use crate::error::{AppError, AppErrorDto, AppResult};
use crate::models::RuntimePhase;
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

const GITHUB_MANIFEST: &str =
    "https://github.com/CMMUU/serylane/releases/latest/download/latest.json";
// Published <= 0.7.6 clients require this exact URL in latest.json. Keep the
// signed compatibility alias while all visible release links use Serylane.
const LEGACY_GITHUB_RELEASES: &str = "https://github.com/CMMUU/routedeck/releases";
const GITEE_RELEASE: &str = "https://gitee.com/api/v5/repos/cmmuu/routedeck/releases/latest";
const MAX_METADATA_BYTES: usize = 512 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    #[default]
    Auto,
    Github,
    Gitee,
}

impl UpdateSource {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Github => "GitHub",
            Self::Gitee => "Gitee",
        }
    }
    fn release_base(self) -> AppResult<&'static str> {
        match self {
            Self::Github => Ok("https://github.com/CMMUU/serylane/releases"),
            Self::Gitee => Ok("https://gitee.com/cmmuu/routedeck/releases"),
            Self::Auto => Err(failure("请指定实际发布渠道")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StableVersion(u64, u64, u64);

fn parse_version(value: &str) -> AppResult<StableVersion> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || (p.len() > 1 && p.starts_with('0'))
                || !p.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return Err(failure("版本号必须是严格的 X.Y.Z 稳定版本"));
    }
    let number = |p: &str| p.parse::<u64>().map_err(|_| failure("版本号超出范围"));
    Ok(StableVersion(
        number(parts[0])?,
        number(parts[1])?,
        number(parts[2])?,
    ))
}

fn official_release_url(source: UpdateSource, tag: &str) -> AppResult<String> {
    parse_version(
        tag.strip_prefix('v')
            .ok_or_else(|| failure("发布标签必须以 v 开头"))?,
    )?;
    Ok(format!("{}/tag/{tag}", source.release_base()?))
}

fn failure(message: impl Into<String>) -> AppError {
    AppError::Update(message.into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannelStatus {
    source: UpdateSource,
    version: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub ahead: bool,
    pub notes: String,
    pub published_at: Option<String>,
    pub release_url: String,
    pub source: UpdateSource,
    pub channels: Vec<UpdateChannelStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    pub phase: String,
    pub info: Option<AppUpdateInfo>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

impl Default for AppUpdateStatus {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            info: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            error: None,
        }
    }
}

#[derive(Clone)]
struct Candidate {
    update: Update,
    source: UpdateSource,
    version: StableVersion,
    sha256: String,
    size: u64,
}

#[derive(Default)]
struct Session {
    status: AppUpdateStatus,
    candidates: Vec<Candidate>,
    // Only the native, signature-verified download path can populate this.
    // The frontend never supplies installation bytes, a path, or a download URL.
    ready: Option<(Candidate, Vec<u8>)>,
}

#[derive(Default)]
pub struct AppUpdateManager {
    session: Mutex<Session>,
    busy: AtomicBool,
    cancelled: AtomicBool,
}

struct Operation<'a>(&'a AppUpdateManager);
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if let Ok(mut session) = self.0.session.lock() {
            if matches!(
                session.status.phase.as_str(),
                "checking" | "downloading" | "installing"
            ) {
                session.status.phase = "failed".into();
                session.status.error = Some("操作已中断，请重试".into());
            }
        }
        self.0.busy.store(false, Ordering::Release);
    }
}

impl AppUpdateManager {
    fn lock(&self) -> AppResult<MutexGuard<'_, Session>> {
        self.session.lock().map_err(|_| failure("更新状态锁不可用"))
    }
    fn begin(&self) -> AppResult<Operation<'_>> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::Conflict("已有更新操作正在进行".into()))?;
        self.cancelled.store(false, Ordering::Release);
        Ok(Operation(self))
    }
    fn finish_error(&self, error: &AppError) -> AppErrorDto {
        if let Ok(mut session) = self.lock() {
            session.status.phase = if self.cancelled.load(Ordering::Acquire) {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            session.status.error = Some(error.to_string());
        }
        error.dto()
    }
}

fn artifact_name(target: &str, version: &str) -> AppResult<String> {
    parse_version(version)?;
    let suffix = match target {
        "windows-x86_64" | "windows-x86_64-nsis" => "x64-setup.exe",
        "windows-aarch64" | "windows-aarch64-nsis" => "arm64-setup.exe",
        "windows-x86_64-msi" => "x64_en-US.msi",
        "windows-aarch64-msi" => "arm64_en-US.msi",
        "darwin-x86_64" => "x64.app.tar.gz",
        "darwin-aarch64" => "aarch64.app.tar.gz",
        "linux-x86_64" => "amd64.AppImage",
        "linux-aarch64" => "aarch64.AppImage",
        _ => return Err(failure("此系统架构暂不支持内置更新")),
    };
    Ok(format!("RouteDeck_{version}_{suffix}"))
}

fn validate_asset(
    source: UpdateSource,
    target: &str,
    version: &str,
    url: &Url,
    raw: &serde_json::Value,
) -> AppResult<(String, u64)> {
    let expected = format!(
        "{}/download/v{version}/{}",
        source.release_base()?,
        artifact_name(target, version)?
    );
    let legacy_github = matches!(source, UpdateSource::Github)
        && url.as_str()
            == format!(
                "{LEGACY_GITHUB_RELEASES}/download/v{version}/{}",
                artifact_name(target, version)?
            );
    if url.as_str() != expected && !legacy_github {
        return Err(failure("更新包地址与官方渠道、版本或架构不一致"));
    }
    let platform = &raw["platforms"][target];
    let hash = platform["sha256"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| failure("更新清单缺少有效 SHA-256 摘要"))?;
    let size = platform["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_DOWNLOAD_BYTES)
        .ok_or_else(|| failure("更新包大小无效或超过 256 MiB 限制"))?;
    Ok((hash.to_ascii_lowercase(), size))
}

fn proxy_for_update(app: &AppHandle) -> AppResult<Option<Url>> {
    let settings = AppStorage::from_app(app)?.settings()?;
    if app.state::<MihomoRuntime>().status(Some(app)).phase == RuntimePhase::Running {
        Ok(Some(
            Url::parse(&format!("http://127.0.0.1:{}", settings.mixed_port))
                .map_err(|_| failure("本地代理地址无效"))?,
        ))
    } else {
        Ok(None)
    }
}

async fn gitee_manifest(proxy: Option<&Url>) -> AppResult<Url> {
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8));
    if let Some(proxy) = proxy {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy.as_str()).map_err(|_| failure("更新代理无效"))?);
    }
    let response = builder
        .build()
        .map_err(|_| failure("无法创建 Gitee 更新客户端"))?
        .get(GITEE_RELEASE)
        .header(
            "User-Agent",
            concat!("Serylane/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(|_| failure("无法连接 Gitee 发布服务"))?;
    if !response.status().is_success() {
        return Err(failure(format!(
            "Gitee 发布服务返回 HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| failure("读取 Gitee 发布信息失败"))?;
        if body.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(failure("Gitee 发布信息过大"));
        }
        body.extend_from_slice(&chunk);
    }
    let release: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| failure("Gitee 发布信息格式无效"))?;
    if release["prerelease"].as_bool() == Some(true) || release["draft"].as_bool() == Some(true) {
        return Err(failure("Gitee 尚无稳定正式版"));
    }
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| failure("Gitee 缺少发布标签"))?;
    official_release_url(UpdateSource::Gitee, tag)?;
    Url::parse(&format!(
        "{}/download/{tag}/latest-gitee.json",
        UpdateSource::Gitee.release_base()?
    ))
    .map_err(|_| failure("Gitee 清单地址无效"))
}

async fn check_source(
    app: &AppHandle,
    source: UpdateSource,
    proxy: Option<&Url>,
) -> AppResult<Candidate> {
    #[cfg(target_os = "linux")]
    if tauri::utils::platform::bundle_type() != Some(tauri::utils::config::BundleType::AppImage)
        || app.env().appimage.is_none()
    {
        return Err(failure(
            "此 Linux 安装不是 AppImage，请通过原 deb/rpm 包管理方式升级",
        ));
    }
    let endpoint = if source == UpdateSource::Github {
        Url::parse(GITHUB_MANIFEST).map_err(|_| failure("GitHub 清单地址无效"))?
    } else {
        gitee_manifest(proxy).await?
    };
    let target = tauri_plugin_updater::target().ok_or_else(|| failure("此平台不支持更新"))?;
    let mut builder = app
        .updater_builder()
        .timeout(Duration::from_secs(10))
        // Inspect equal/older versions to report unpublished local builds, but
        // keep installation strictly newer-only in the native command below.
        .version_comparator(|_, _| true)
        .endpoints(vec![endpoint.clone()])
        .map_err(|_| failure("更新清单地址无效"))?
        .configure_client(|client| {
            client
                .https_only(true)
                .connect_timeout(Duration::from_secs(3))
        });
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    let mut update = builder
        .build()
        .map_err(|_| failure("此安装方式不支持内置更新；Linux 请使用 AppImage"))?
        .check()
        .await
        .map_err(|e| failure(format!("{} 更新清单不可用：{e}", source.label())))?
        .ok_or_else(|| failure("发布服务没有返回更新清单"))?;
    let version = parse_version(&update.version)?;
    if update.signature.len() > 4096
        || update.signature.trim().is_empty()
        || update.raw_json.to_string().len() > MAX_METADATA_BYTES
    {
        return Err(failure("更新清单或签名无效"));
    }
    let manifest_target = [
        format!("{target}-msi"),
        format!("{target}-nsis"),
        target.clone(),
    ]
    .into_iter()
    .find(|key| {
        update.raw_json["platforms"][key]["url"].as_str() == Some(update.download_url.as_str())
    })
    .ok_or_else(|| failure("更新包不属于当前系统架构"))?;
    let (sha256, size) = validate_asset(
        source,
        &manifest_target,
        &update.version,
        &update.download_url,
        &update.raw_json,
    )?;
    if source == UpdateSource::Gitee
        && endpoint.path()
            != format!(
                "/cmmuu/routedeck/releases/download/v{}/latest-gitee.json",
                update.version
            )
    {
        return Err(failure("Gitee 发布标签与更新清单版本不一致"));
    }
    update.timeout = Some(Duration::from_secs(600));
    Ok(Candidate {
        update,
        source,
        version,
        sha256,
        size,
    })
}

fn same_build(a: &Candidate, b: &Candidate) -> bool {
    a.version == b.version
        && a.sha256 == b.sha256
        && a.size == b.size
        && a.update.signature == b.update.signature
}

fn candidate_priority(
    version: StableVersion,
    source: UpdateSource,
) -> (std::cmp::Reverse<StableVersion>, u8) {
    // Prefer the domestic mirror for the newest stable build. A lagging mirror
    // must not hide a newer GitHub release or become a downgrade fallback.
    let priority = match source {
        UpdateSource::Gitee => 0,
        UpdateSource::Github => 1,
        UpdateSource::Auto => 2,
    };
    (std::cmp::Reverse(version), priority)
}

#[tauri::command]
pub fn save_update_preferences(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
    source: UpdateSource,
    auto_check: bool,
    auto_download: bool,
) -> Result<crate::models::PublicAppSettings, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    let _configuration = crate::user_rules::acquire_configuration(&app).map_err(|e| e.dto())?;
    let storage = AppStorage::from_app(&app).map_err(|e| e.dto())?;
    let mut settings = storage.settings().map_err(|e| e.dto())?;
    let source_changed = settings.update_source != source;
    settings.update_source = source;
    settings.auto_check_updates = auto_check;
    settings.auto_download_updates = auto_download;
    storage.save_settings(&settings).map_err(|e| e.dto())?;
    if source_changed {
        *state.lock().map_err(|e| e.dto())? = Session::default();
    }
    Ok(crate::models::PublicAppSettings::from(&settings))
}

#[tauri::command]
pub fn app_update_status(
    state: State<'_, AppUpdateManager>,
) -> Result<AppUpdateStatus, AppErrorDto> {
    state
        .lock()
        .map(|session| session.status.clone())
        .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn check_app_update(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
) -> Result<AppUpdateStatus, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    {
        let mut session = state.lock().map_err(|e| e.dto())?;
        *session = Session::default();
        session.status.phase = "checking".into();
    }
    let result = async {
        let source = AppStorage::from_app(&app)?.settings()?.update_source;
        let proxy = proxy_for_update(&app)?;
        let results = match source {
            UpdateSource::Auto => {
                let (gitee, github) = tokio::join!(
                    check_source(&app, UpdateSource::Gitee, proxy.as_ref()),
                    check_source(&app, UpdateSource::Github, proxy.as_ref())
                );
                vec![(UpdateSource::Gitee, gitee), (UpdateSource::Github, github)]
            }
            single => vec![(single, check_source(&app, single, proxy.as_ref()).await)],
        };
        let mut channels = Vec::new();
        let mut candidates = Vec::new();
        for (source, result) in results {
            match result {
                Ok(candidate) => {
                    channels.push(UpdateChannelStatus {
                        source,
                        version: Some(format!("v{}", candidate.update.version)),
                        error: None,
                    });
                    candidates.push(candidate);
                }
                Err(error) => channels.push(UpdateChannelStatus {
                    source,
                    version: None,
                    error: Some(error.to_string()),
                }),
            }
        }
        candidates.sort_by_key(|candidate| candidate_priority(candidate.version, candidate.source));
        let latest = candidates.first().ok_or_else(|| {
            failure(
                channels
                    .iter()
                    .filter_map(|c| c.error.clone())
                    .collect::<Vec<_>>()
                    .join("；"),
            )
        })?;
        let current = parse_version(env!("CARGO_PKG_VERSION"))?;
        for candidate in &candidates {
            if candidate.version == latest.version && !same_build(latest, candidate) {
                if let Some(channel) = channels
                    .iter_mut()
                    .find(|channel| channel.source == candidate.source)
                {
                    channel.error =
                        Some("此渠道同版本的安装包或签名不一致，已排除跨渠道下载回退".into());
                }
            }
        }
        let info = AppUpdateInfo {
            current_version: env!("CARGO_PKG_VERSION").into(),
            latest_version: format!("v{}", latest.update.version),
            available: latest.version > current,
            ahead: latest.version < current,
            notes: latest
                .update
                .body
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(20_000)
                .collect(),
            published_at: latest.update.raw_json["pub_date"]
                .as_str()
                .map(str::to_owned),
            release_url: official_release_url(
                latest.source,
                &format!("v{}", latest.update.version),
            )?,
            source: latest.source,
            channels,
        };
        let selected = latest.clone();
        // Never silently substitute a different build/version on a mirror.
        candidates.retain(|candidate| same_build(&selected, candidate));
        let mut session = state.lock()?;
        session.status.phase = if info.available {
            "available"
        } else if info.ahead {
            "ahead"
        } else {
            "current"
        }
        .into();
        session.status.info = Some(info);
        session.candidates = candidates;
        Ok(session.status.clone())
    }
    .await;
    result.map_err(|e| state.finish_error(&e))
}

async fn download_candidate(candidate: &Candidate, state: &AppUpdateManager) -> AppResult<Vec<u8>> {
    let oversized = AtomicBool::new(false);
    let mut received = 0_u64;
    let download = candidate.update.download(
        |length, total| {
            received = received.saturating_add(length as u64);
            if received > candidate.size || total.is_some_and(|size| size != candidate.size) {
                oversized.store(true, Ordering::Release);
            }
            if let Ok(mut session) = state.lock() {
                session.status.downloaded_bytes = received;
            }
        },
        || {},
    );
    tokio::pin!(download);
    let bytes = loop {
        tokio::select! {
            result = &mut download => break result.map_err(|e| failure(format!("下载或签名校验失败：{e}")))?,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if state.cancelled.load(Ordering::Acquire) { return Err(failure("下载已取消")); }
                if oversized.load(Ordering::Acquire) { return Err(failure("更新包大小与发布清单不符")); }
            }
        }
    };
    if state.cancelled.load(Ordering::Acquire) {
        return Err(failure("下载已取消"));
    }
    if oversized.load(Ordering::Acquire)
        || bytes.len() as u64 != candidate.size
        || format!("{:x}", Sha256::digest(&bytes)) != candidate.sha256
    {
        return Err(failure("更新包大小或 SHA-256 校验失败，已拒绝安装"));
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn download_app_update(
    state: State<'_, AppUpdateManager>,
    version_tag: String,
) -> Result<AppUpdateStatus, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    let candidates = {
        let mut session = state.lock().map_err(|e| e.dto())?;
        if !session
            .status
            .info
            .as_ref()
            .is_some_and(|info| info.available && info.latest_version == version_tag)
        {
            return Err(failure("更新版本已变化或不是新版本，请重新检查").dto());
        }
        session.ready = None;
        session.status.phase = "downloading".into();
        session.status.error = None;
        session.candidates.clone()
    };
    let mut errors = Vec::new();
    for candidate in candidates {
        {
            let mut session = state.lock().map_err(|e| e.dto())?;
            session.status.downloaded_bytes = 0;
            session.status.total_bytes = candidate.size;
            if let Some(info) = &mut session.status.info {
                info.source = candidate.source;
                info.release_url =
                    official_release_url(candidate.source, &version_tag).map_err(|e| e.dto())?;
            }
        }
        match download_candidate(&candidate, &state).await {
            Ok(bytes) => {
                let mut session = state.lock().map_err(|e| e.dto())?;
                session.status.phase = "ready".into();
                session.ready = Some((candidate, bytes));
                return Ok(session.status.clone());
            }
            Err(error) => {
                errors.push(format!("{}：{error}", candidate.source.label()));
                if state.cancelled.load(Ordering::Acquire) {
                    break;
                }
            }
        }
    }
    Err(state.finish_error(&failure(errors.join("；"))))
}

#[tauri::command]
pub fn cancel_app_update(state: State<'_, AppUpdateManager>) -> Result<(), AppErrorDto> {
    if state.lock().map_err(|e| e.dto())?.status.phase == "downloading" {
        state.cancelled.store(true, Ordering::Release);
    }
    Ok(())
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
    version_tag: String,
    confirmed: bool,
) -> Result<(), AppErrorDto> {
    if !confirmed {
        return Err(failure("安装需要用户确认；安装期间代理会暂时停止").dto());
    }
    let _operation = state.begin().map_err(|e| e.dto())?;
    let _configuration = crate::user_rules::acquire_configuration(&app).map_err(|e| e.dto())?;
    let result = (|| -> AppResult<()> {
        let mut session = state.lock()?;
        if session.status.phase != "ready"
            || !session
                .status
                .info
                .as_ref()
                .is_some_and(|info| info.available && info.latest_version == version_tag)
        {
            return Err(failure("请先下载并验证当前选定的新版本"));
        }
        let (candidate, bytes) = session
            .ready
            .as_ref()
            .ok_or_else(|| failure("已验证的安装包不可用，请重新下载"))?;
        if candidate.version <= parse_version(env!("CARGO_PKG_VERSION"))?
            || format!("{:x}", Sha256::digest(bytes)) != candidate.sha256
        {
            return Err(failure("安装包版本或摘要校验失败"));
        }
        // No disruptive work before explicit confirmation and a verified package.
        app.state::<crate::local_routing::LocalRoutingManager>()
            .shutdown(&app)?;
        app.state::<crate::openai_stability::StabilityManager>()
            .stop();
        crate::platform::restore_system_proxy(&app)?;
        app.state::<MihomoRuntime>().stop(Some(&app))?;
        app.state::<crate::OpenAiPolicyTaskManager>().cancel()?;
        app.state::<crate::GlobalTrafficMonitor>().stop();
        AppStorage::from_app(&app)?.mark_clean_shutdown(true)?;
        let (candidate, bytes) = session
            .ready
            .take()
            .ok_or_else(|| failure("安装包已失效"))?;
        session.status.phase = "installing".into();
        drop(session);
        candidate.update.install(bytes).map_err(|e| {
            failure(format!(
                "安装启动失败：{e}。代理已安全停止，可重新启动后重试。"
            ))
        })?;
        // Windows exits from install(); its installer relaunches Serylane.
        #[cfg(not(windows))]
        {
            app.restart()
        }
        #[cfg(windows)]
        {
            Ok(())
        }
    })();
    result.map_err(|e| state.finish_error(&e))
}

#[tauri::command]
pub fn open_official_release(
    app: AppHandle,
    source: UpdateSource,
    version_tag: String,
) -> Result<(), AppErrorDto> {
    let url = official_release_url(source, &version_tag).map_err(|e| e.dto())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| failure(format!("无法打开浏览器：{e}")).dto())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newest_build_prefers_gitee_and_keeps_github_as_download_fallback() {
        let version = StableVersion(1, 2, 3);
        let mut sources = [UpdateSource::Github, UpdateSource::Gitee];
        sources.sort_by_key(|source| candidate_priority(version, *source));
        assert_eq!(sources, [UpdateSource::Gitee, UpdateSource::Github]);
        sources.reverse();
        sources.sort_by_key(|source| candidate_priority(version, *source));
        assert_eq!(sources, [UpdateSource::Gitee, UpdateSource::Github]);
    }
    #[test]
    fn lagging_domestic_mirror_never_hides_a_newer_official_release() {
        let mut releases = [
            (StableVersion(1, 2, 3), UpdateSource::Gitee),
            (StableVersion(1, 2, 4), UpdateSource::Github),
        ];
        releases.sort_by_key(|(version, source)| candidate_priority(*version, *source));
        assert_eq!(releases[0], (StableVersion(1, 2, 4), UpdateSource::Github));
    }
    #[test]
    fn automatic_updates_remain_the_default_and_either_single_source_is_usable() {
        assert_eq!(UpdateSource::default(), UpdateSource::Auto);
        for source in [UpdateSource::Gitee, UpdateSource::Github] {
            let mut sources = [source];
            sources.sort_by_key(|source| candidate_priority(StableVersion(1, 2, 3), *source));
            assert_eq!(sources, [source]);
        }
    }
    #[test]
    fn versions_are_strict_numeric_and_never_allow_prereleases_or_paths() {
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.9").unwrap());
        for version in [
            "v1.2.3",
            "1.02.3",
            "1.2.03",
            "1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "1.2.3/other",
            "1.2.18446744073709551616",
        ] {
            assert!(parse_version(version).is_err(), "{version}");
        }
    }
    #[test]
    fn official_urls_are_derived_from_source_and_strict_tag() {
        assert_eq!(
            official_release_url(UpdateSource::Github, "v0.7.7").unwrap(),
            "https://github.com/CMMUU/serylane/releases/tag/v0.7.7"
        );
        assert_eq!(
            official_release_url(UpdateSource::Gitee, "v1.2.3").unwrap(),
            "https://gitee.com/cmmuu/routedeck/releases/tag/v1.2.3"
        );
        assert!(official_release_url(UpdateSource::Auto, "v1.2.3").is_err());
        assert!(official_release_url(UpdateSource::Github, "v1.2.3?redirect=evil").is_err());
    }
    #[test]
    fn github_rename_accepts_only_exact_current_and_legacy_update_assets() {
        let raw = serde_json::json!({ "platforms": { "windows-x86_64": { "sha256": "a".repeat(64), "size": 100 } } });
        for repo in ["serylane", "routedeck"] {
            let url = format!("https://github.com/CMMUU/{repo}/releases/download/v0.7.7/RouteDeck_0.7.7_x64-setup.exe");
            assert!(validate_asset(
                UpdateSource::Github,
                "windows-x86_64",
                "0.7.7",
                &Url::parse(&url).unwrap(),
                &raw
            )
            .is_ok());
            for invalid in [
                url.replace("CMMUU", "other"),
                url.replace("github.com", "github.com.attacker"),
                url.replace(repo, "other-repository"),
                url.replace("v0.7.7/", "v0.7.8/"),
                url.replace("_x64-", "_arm64-"),
                format!("{url}?redirect=other"),
            ] {
                assert!(validate_asset(
                    UpdateSource::Github,
                    "windows-x86_64",
                    "0.7.7",
                    &Url::parse(&invalid).unwrap(),
                    &raw
                )
                .is_err());
            }
        }
    }
    #[test]
    fn asset_validation_binds_channel_version_arch_hash_and_size() {
        let raw = serde_json::json!({ "platforms": { "windows-x86_64": { "sha256": "a".repeat(64), "size": 100 } } });
        let url = Url::parse("https://gitee.com/cmmuu/routedeck/releases/download/v1.2.3/RouteDeck_1.2.3_x64-setup.exe").unwrap();
        assert!(validate_asset(UpdateSource::Gitee, "windows-x86_64", "1.2.3", &url, &raw).is_ok());
        assert!(
            validate_asset(UpdateSource::Github, "windows-x86_64", "1.2.3", &url, &raw).is_err()
        );
        assert!(
            validate_asset(UpdateSource::Gitee, "windows-aarch64", "1.2.3", &url, &raw).is_err()
        );
        assert!(
            validate_asset(UpdateSource::Gitee, "windows-x86_64", "1.2.4", &url, &raw).is_err()
        );
        let mut altered = raw.clone();
        altered["platforms"]["windows-x86_64"]["size"] = (MAX_DOWNLOAD_BYTES + 1).into();
        assert!(validate_asset(
            UpdateSource::Gitee,
            "windows-x86_64",
            "1.2.3",
            &url,
            &altered
        )
        .is_err());
        assert!(validate_asset(
            UpdateSource::Gitee,
            "windows-x86_64",
            "1.2.3",
            &url,
            &serde_json::json!({})
        )
        .is_err());
    }
    #[test]
    fn updater_operations_are_exclusive_and_cancellation_resets_on_retry() {
        let manager = AppUpdateManager::default();
        let operation = manager.begin().unwrap();
        assert!(manager.begin().is_err());
        manager.cancelled.store(true, Ordering::Release);
        manager.lock().unwrap().status.phase = "downloading".into();
        drop(operation);
        assert_eq!(manager.lock().unwrap().status.phase, "failed");
        let _retry = manager.begin().unwrap();
        assert!(!manager.cancelled.load(Ordering::Acquire));
    }
}
