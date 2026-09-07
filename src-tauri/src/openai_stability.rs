//! Sticky, evidence-based failover. Only selects a destination for NEW connections.
//! Never calls /connections DELETE, reloads the core from a timer, or replays model requests.
use crate::{
    error::{AppError, AppErrorDto, AppResult},
    mihomo_api::MihomoApiClient,
    models::RuntimePhase,
    runtime::MihomoRuntime,
    storage::AppStorage,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Manager};
use uuid::Uuid;
pub const GROUP: &str = "🤖 OpenAI 自动灾备";
const WINDOW: u64 = 900;
const COOLDOWN: u64 = 300;
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Copy)]
pub enum Evidence {
    Probe(bool),
    ModelComplete,
    ModelInterrupted,
}
#[derive(Clone, Default)]
struct NodeHealth {
    samples: VecDeque<(u64, bool, bool)>,
    consecutive_failures: u32,
    recovery_passes: u32,
    cooldown_until: u64,
    last_probe: u64,
    model_completed: u64,
    model_interrupted: u64,
}
impl NodeHealth {
    fn prune(&mut self, time: u64) {
        while self
            .samples
            .front()
            .is_some_and(|(t, _, _)| time.saturating_sub(*t) > WINDOW)
        {
            self.samples.pop_front();
        }
        while self.samples.len() > 90 {
            self.samples.pop_front();
        }
    }
    fn record(&mut self, time: u64, event: Evidence) {
        self.prune(time);
        let (ok, model) = match event {
            Evidence::Probe(ok) => {
                self.last_probe = time;
                (ok, false)
            }
            Evidence::ModelComplete => {
                self.model_completed += 1;
                (true, true)
            }
            Evidence::ModelInterrupted => {
                self.model_interrupted += 1;
                (false, true)
            }
        };
        self.samples.push_back((time, ok, model));
        if ok {
            // Probe successes cannot erase a streak of real model-stream failures.
            if model
                || !self
                    .samples
                    .iter()
                    .rev()
                    .take(6)
                    .any(|(_, good, m)| *m && !good)
            {
                self.consecutive_failures = 0;
            }
            if time >= self.cooldown_until {
                self.recovery_passes += 1;
            }
        } else {
            self.consecutive_failures += 1;
            self.recovery_passes = 0;
            if self.consecutive_failures >= 2 {
                self.cooldown_until = time + COOLDOWN;
            }
        }
    }
    fn usable(&self, time: u64) -> bool {
        self.last_probe > 0
            && time.saturating_sub(self.last_probe) <= 150
            && self.cooldown_until <= time
            && (self.cooldown_until == 0 || self.recovery_passes >= 3)
            && self
                .samples
                .iter()
                .rev()
                .find(|(_, _, model)| !model)
                .is_some_and(|(_, ok, _)| *ok)
    }
    fn score(&self) -> f64 {
        let (good, total) =
            self.samples
                .iter()
                .fold((0.0, 0.0), |(good, total), (_, ok, model)| {
                    let weight = if *model { 3.0 } else { 1.0 };
                    (good + if *ok { weight } else { 0.0 }, total + weight)
                });
        if total == 0.0 {
            0.0
        } else {
            good / total
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSnapshot {
    name: String,
    probe_ok: bool,
    success_rate: Option<u64>,
    samples: usize,
    cooldown_seconds: u64,
    recovery_passes: u32,
    model_completed: u64,
    model_interrupted: u64,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StabilitySnapshot {
    pub enabled: bool,
    pub running: bool,
    pub eligible: bool,
    pub profile_id: Option<Uuid>,
    pub revision_id: Option<Uuid>,
    pub current: Option<String>,
    pub last_switch: Option<u64>,
    pub message: String,
    pub nodes: Vec<NodeSnapshot>,
}
#[derive(Default)]
struct HealthState {
    profile: Option<Uuid>,
    revision: Option<Uuid>,
    current: Option<String>,
    epoch: u64,
    nodes: BTreeMap<String, NodeHealth>,
    snapshot: StabilitySnapshot,
}
#[derive(Clone, Default)]
pub struct StabilityManager {
    inner: Arc<Mutex<HealthState>>,
    stopped: Arc<AtomicBool>,
}
/// A request contributes model evidence only when its outbound is the default RouteDeck
/// proxy and the observed group selection stays unchanged throughout the request.
#[derive(Clone)]
pub struct Observation {
    manager: StabilityManager,
    node: String,
    epoch: u64,
}
impl Observation {
    pub fn matches_connection(
        &self,
        data: &serde_json::Value,
        source: std::net::SocketAddr,
        host: &str,
    ) -> bool {
        if !source.ip().is_loopback() {
            return false;
        }
        data["connections"].as_array().is_some_and(|connections| {
            connections.iter().any(|connection| {
                let meta = &connection["metadata"];
                let port = meta["sourcePort"]
                    .as_u64()
                    .or_else(|| meta["sourcePort"].as_str().and_then(|p| p.parse().ok()));
                let ip = meta["sourceIP"]
                    .as_str()
                    .and_then(|ip| ip.parse::<std::net::IpAddr>().ok());
                let chains = connection["chains"].as_array();
                port == Some(u64::from(source.port()))
                    && ip == Some(source.ip())
                    && meta["host"].as_str() == Some(host)
                    && chains.is_some_and(|v| {
                        v.iter().any(|c| c.as_str() == Some(GROUP))
                            && v.iter().any(|c| c.as_str() == Some(&self.node))
                    })
            })
        })
    }
    pub fn finish(&self, event: Evidence) {
        if let Ok(mut s) = self.manager.inner.lock() {
            if s.epoch == self.epoch && s.current.as_ref() == Some(&self.node) && s.snapshot.running
            {
                s.nodes
                    .entry(self.node.clone())
                    .or_default()
                    .record(now(), event);
            }
        }
    }
}
impl StabilityManager {
    pub fn invalidate_observations(&self) {
        if let Ok(mut s) = self.inner.lock() {
            s.epoch += 1;
            s.snapshot.running = false;
        }
    }
    pub fn update_policy_status(&self, app: &AppHandle) {
        let Ok(storage) = AppStorage::from_app(app) else {
            return;
        };
        let Ok(persistent) = storage.state() else {
            return;
        };
        let profile = persistent
            .active_profile_id
            .and_then(|id| storage.load_profile(id).ok());
        if let Ok(mut s) = self.inner.lock() {
            s.epoch += 1;
            s.snapshot.running = false;
            s.snapshot.profile_id = persistent.active_profile_id;
            s.snapshot.revision_id = profile.as_ref().and_then(|p| p.active_revision_id);
            s.snapshot.eligible = profile.as_ref().is_some_and(|p| {
                p.openai_policy.enabled && p.openai_policy.selected_nodes.len() >= 2
            });
            s.snapshot.enabled = profile
                .as_ref()
                .is_some_and(|p| p.openai_policy.enabled && p.openai_policy.stability_enabled);
            s.snapshot.message = "策略已应用，等待下一轮稳定性检查".into();
        }
    }
    pub fn snapshot(&self) -> StabilitySnapshot {
        self.inner
            .lock()
            .map(|mut s| {
                let mut out = s.snapshot.clone();
                let time = now();
                out.current = s.current.clone();
                for node in s.nodes.values_mut() {
                    node.prune(time);
                }
                out.nodes = s
                    .nodes
                    .iter()
                    .map(|(name, n)| NodeSnapshot {
                        name: name.clone(),
                        probe_ok: n.usable(time),
                        success_rate: (!n.samples.is_empty())
                            .then(|| (n.score() * 100.0).round() as u64),
                        samples: n.samples.len(),
                        cooldown_seconds: n.cooldown_until.saturating_sub(time),
                        recovery_passes: n.recovery_passes,
                        model_completed: n.model_completed,
                        model_interrupted: n.model_interrupted,
                    })
                    .collect();
                out
            })
            .unwrap_or_default()
    }
    pub fn observe(&self) -> Option<Observation> {
        let s = self.inner.lock().ok()?;
        if !s.snapshot.running {
            return None;
        }
        Some(Observation {
            manager: self.clone(),
            node: s.current.clone().filter(|s| s != "REJECT")?,
            epoch: s.epoch,
        })
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn start(&self, app: AppHandle) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while !manager.stopped.load(Ordering::Acquire) {
                timer.tick().await;
                if manager.stopped.load(Ordering::Acquire) {
                    break;
                }
                if manager.tick(&app).await.is_err() {
                    if let Ok(mut s) = manager.inner.lock() {
                        s.snapshot.running = false;
                        s.snapshot.message =
                            "稳定性检查暂不可用，保留当前节点；没有关闭连接".into();
                    }
                }
            }
        });
    }
    async fn tick(&self, app: &AppHandle) -> AppResult<()> {
        let storage = AppStorage::from_app(app)?;
        let active = storage.state()?.active_profile_id;
        let profile = active.map(|id| storage.load_profile(id)).transpose()?;
        let policy = profile.as_ref().map(|p| &p.openai_policy);
        {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            let revision = profile.as_ref().and_then(|p| p.active_revision_id);
            if s.profile != active || s.revision != revision {
                s.nodes.clear();
                s.current = None;
                s.epoch += 1;
                s.profile = active;
                s.revision = revision;
            }
            s.snapshot.profile_id = active;
            s.snapshot.revision_id = revision;
            s.snapshot.eligible = policy.is_some_and(|p| p.enabled && p.selected_nodes.len() >= 2);
            s.snapshot.enabled = policy.is_some_and(|p| p.enabled && p.stability_enabled);
            s.snapshot.running = false;
            s.snapshot.message = "请先生成 OpenAI 灾备并启用稳定策略".into();
        }
        let Some(policy) = policy.filter(|p| p.enabled && p.stability_enabled) else {
            return Ok(());
        };
        if app.state::<MihomoRuntime>().status(Some(app)).phase != RuntimePhase::Running {
            return Ok(());
        }
        let settings = storage.settings()?;
        let api = MihomoApiClient::new(&settings)?;
        let proxies = api.proxies().await?;
        let group = &proxies["proxies"][GROUP];
        if group["type"].as_str() != Some("Selector") {
            return Err(AppError::Conflict("核心尚未加载稳定策略".into()));
        }
        let current = group["now"].as_str().unwrap_or("REJECT").to_string();
        let allowed: Vec<String> = policy
            .selected_nodes
            .iter()
            .filter(|n| {
                group["all"]
                    .as_array()
                    .is_some_and(|all| all.iter().any(|x| x.as_str() == Some(&n.name)))
            })
            .take(10)
            .map(|n| n.name.clone())
            .collect();
        {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            if s.current.as_ref() != Some(&current) {
                s.epoch += 1;
                s.current = Some(current.clone());
            }
            s.snapshot.running = true;
            s.snapshot.message =
                "基础连通性与真实模型流分开统计；保持健康节点，不主动清空连接".into();
        }
        use futures_util::{stream, StreamExt};
        let checks = stream::iter(allowed.iter().cloned().map(|name| {
            let api = api.clone();
            async move {
                // No login credentials are used for probes. A 401 only means API reachability.
                let result = api
                    .delay_expected(
                        &name,
                        "https://api.openai.com/v1/models",
                        8_000,
                        Some("401"),
                    )
                    .await;
                let ok = result
                    .ok()
                    .and_then(|v| v["delay"].as_u64())
                    .is_some_and(|n| n > 0);
                (name, ok)
            }
        }))
        .buffer_unordered(2)
        .collect::<Vec<_>>()
        .await;
        // A stopped/reloaded core is not evidence that every candidate is broken.
        // Gate both recording and selection, not just the eventual controller write.
        let _permit = crate::user_rules::acquire_configuration(app)?;
        if self.stopped.load(Ordering::Acquire)
            || app.state::<MihomoRuntime>().status(Some(app)).phase != RuntimePhase::Running
            || storage.state()?.active_profile_id != active
        {
            self.invalidate_observations();
            return Ok(());
        }
        let latest = active.map(|id| storage.load_profile(id)).transpose()?;
        if latest.as_ref().and_then(|p| p.active_revision_id)
            != profile.as_ref().and_then(|p| p.active_revision_id)
            || !latest
                .as_ref()
                .is_some_and(|p| p.openai_policy.stability_enabled)
        {
            self.invalidate_observations();
            return Ok(());
        }
        let live = api.proxies().await?;
        if live["proxies"][GROUP]["type"].as_str() != Some("Selector")
            || live["proxies"][GROUP]["now"].as_str() != Some(&current)
        {
            self.invalidate_observations();
            return Ok(());
        }
        let candidate = {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            for (name, ok) in checks {
                s.nodes
                    .entry(name)
                    .or_default()
                    .record(now(), Evidence::Probe(ok));
            }
            choose(&s.nodes, &current, &allowed, now())
        };
        if candidate == current {
            return Ok(());
        }
        api.select_proxy(GROUP, &candidate).await?;
        let mut s = self
            .inner
            .lock()
            .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
        s.epoch += 1;
        s.current = Some(candidate);
        s.snapshot.last_switch = Some(now());
        s.snapshot.message = "原节点持续失败，已为后续新连接切换出口；原有连接未被主动关闭".into();
        Ok(())
    }
}
fn choose(
    nodes: &BTreeMap<String, NodeHealth>,
    current: &str,
    allowed: &[String],
    time: u64,
) -> String {
    // One failed probe must not flap a live connection. Recent model failures count too.
    if allowed.iter().any(|n| n == current)
        && nodes
            .get(current)
            .is_some_and(|n| n.consecutive_failures < 2 && n.cooldown_until <= time)
    {
        return current.into();
    }
    allowed
        .iter()
        .filter(|name| nodes.get(*name).is_some_and(|n| n.usable(time)))
        .max_by(|a, b| {
            nodes[*a]
                .score()
                .total_cmp(&nodes[*b].score())
                .then_with(|| b.cmp(a))
        })
        .cloned()
        .unwrap_or_else(|| "REJECT".into())
}
#[tauri::command]
pub async fn set_openai_stability(
    app: AppHandle,
    enabled: bool,
    profile_id: Uuid,
    revision_id: Uuid,
    confirmed: bool,
) -> Result<(), AppErrorDto> {
    let work = async {
        if !confirmed {
            return Err(AppError::InvalidInput("更新稳定灾备需要确认".into()));
        }
        let storage = AppStorage::from_app(&app)?;
        if storage.state()?.active_profile_id != Some(profile_id) {
            return Err(AppError::Conflict("活动配置已变化，请刷新".into()));
        }
        let profile = storage.load_profile(profile_id)?;
        if profile.active_revision_id != Some(revision_id) {
            return Err(AppError::Conflict("配置版本已变化，请刷新".into()));
        }
        let mut policy = profile.openai_policy;
        if !policy.enabled || policy.selected_nodes.len() < 2 {
            return Err(AppError::InvalidInput(
                "请先在代理页生成 OpenAI 灾备".into(),
            ));
        }
        policy.stability_enabled = enabled;
        crate::openai_policy::apply_policy_revision(&app, profile_id, &policy).await?;
        app.state::<StabilityManager>().update_policy_status(&app);
        Ok(())
    };
    work.await.map_err(|e| e.dto())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn healthy_current_is_sticky_and_one_failure_does_not_switch() {
        let mut nodes = BTreeMap::new();
        let mut a = NodeHealth::default();
        a.record(100, Evidence::Probe(true));
        a.record(110, Evidence::Probe(false));
        let mut b = NodeHealth::default();
        b.record(110, Evidence::Probe(true));
        nodes.insert("a".into(), a);
        nodes.insert("b".into(), b);
        assert_eq!(choose(&nodes, "a", &["a".into(), "b".into()], 110), "a");
        nodes
            .get_mut("a")
            .unwrap()
            .record(120, Evidence::Probe(false));
        assert_eq!(choose(&nodes, "a", &["a".into(), "b".into()], 120), "b");
    }
    #[test]
    fn failed_nodes_need_cooldown_and_three_recovery_probes() {
        let mut a = NodeHealth::default();
        a.record(10, Evidence::Probe(false));
        a.record(20, Evidence::Probe(false));
        a.record(100, Evidence::Probe(true));
        assert!(!a.usable(100));
        for time in [330, 390] {
            a.record(time, Evidence::Probe(true));
            assert!(!a.usable(time));
        }
        a.record(450, Evidence::Probe(true));
        assert!(a.usable(450));
    }
    #[test]
    fn model_interruptions_are_not_erased_by_a_401_probe() {
        let mut a = NodeHealth::default();
        a.record(100, Evidence::Probe(true));
        a.record(101, Evidence::ModelInterrupted);
        a.record(102, Evidence::Probe(true));
        a.record(103, Evidence::ModelInterrupted);
        assert!(!a.usable(103));
        assert_eq!(a.model_completed, 0);
        assert_eq!(a.model_interrupted, 2);
        assert!(a.score() < 0.5);
    }
    #[test]
    fn only_unchanged_observations_contribute_model_evidence() {
        let manager = StabilityManager::default();
        {
            let mut s = manager.inner.lock().unwrap();
            s.current = Some("a".into());
            s.snapshot.running = true;
        }
        let valid = manager.observe().unwrap();
        let connections = serde_json::json!({"connections":[{"metadata":{"sourceIP":"127.0.0.1","sourcePort":"45678","host":"chatgpt.com"},"chains":["a",GROUP]}]});
        assert!(valid.matches_connection(
            &connections,
            "127.0.0.1:45678".parse().unwrap(),
            "chatgpt.com"
        ));
        assert!(!valid.matches_connection(
            &connections,
            "127.0.0.1:45679".parse().unwrap(),
            "chatgpt.com"
        ));
        assert!(!valid.matches_connection(
            &connections,
            "127.0.0.1:45678".parse().unwrap(),
            "api.openai.com"
        ));
        let direct = serde_json::json!({"connections":[{"metadata":{"sourceIP":"127.0.0.1","sourcePort":45678,"host":"chatgpt.com"},"chains":["DIRECT"]}]});
        assert!(!valid.matches_connection(
            &direct,
            "127.0.0.1:45678".parse().unwrap(),
            "chatgpt.com"
        ));
        valid.finish(Evidence::ModelComplete);
        assert_eq!(manager.inner.lock().unwrap().nodes["a"].model_completed, 1);
        let stale = manager.observe().unwrap();
        manager.invalidate_observations();
        stale.finish(Evidence::ModelInterrupted);
        assert_eq!(
            manager.inner.lock().unwrap().nodes["a"].model_interrupted,
            0
        );
        assert!(manager.observe().is_none());
    }
    #[test]
    fn no_healthy_candidate_fails_closed_and_stale_evidence_expires() {
        assert_eq!(choose(&BTreeMap::new(), "a", &["a".into()], 100), "REJECT");
        let mut a = NodeHealth::default();
        a.record(10, Evidence::Probe(true));
        assert!(!a.usable(200));
        a.prune(1000);
        assert!(a.samples.is_empty());
    }
}
