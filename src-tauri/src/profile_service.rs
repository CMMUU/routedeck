use crate::config::{inspect_profile, ProfileSummary};
use crate::effective::build_effective_config_with_policy;
use crate::error::{AppError, AppResult};
use crate::models::{
    ConfigRevision, ProfileRecord, ProfileSource, PublicProfileRecord, RoutingMode,
    SubscriptionMetadata, SubscriptionStatus, SubscriptionUsage, ValidationReport,
};
use crate::storage::AppStorage;
use crate::subscription::SubscriptionFetcher;
use crate::user_rules;
use chrono::Utc;
use serde::Serialize;
use tauri::AppHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOperationResult {
    pub profile: PublicProfileRecord,
    pub revision: ConfigRevision,
    pub summary: ProfileSummary,
    pub updated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDetails {
    pub profile: PublicProfileRecord,
    pub revisions: Vec<ConfigRevision>,
    pub summary: Option<ProfileSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionOverview {
    pub profile: PublicProfileRecord,
    pub summary: Option<ProfileSummary>,
    pub revision_count: usize,
    pub latest_fetched_at: Option<chrono::DateTime<Utc>>,
    pub latest_metadata: Option<SubscriptionMetadata>,
    pub latest_validation: Option<ValidationReport>,
    pub active: bool,
    pub status: Option<SubscriptionStatus>,
}

pub fn list_profiles(app: &AppHandle) -> AppResult<Vec<PublicProfileRecord>> {
    Ok(AppStorage::from_app(app)?
        .list_profiles()?
        .iter()
        .map(PublicProfileRecord::from)
        .collect())
}

pub fn list_subscriptions(app: &AppHandle) -> AppResult<Vec<SubscriptionOverview>> {
    let storage = AppStorage::from_app(app)?;
    let active_profile_id = storage.state()?.active_profile_id;
    storage
        .list_profiles()?
        .into_iter()
        .filter(|profile| matches!(profile.source, ProfileSource::RemoteSubscription { .. }))
        .map(|profile| {
            let revisions = storage.list_revisions(profile.id)?;
            let latest = revisions.first();
            let summary = match profile.active_revision_id {
                Some(revision_id) => storage
                    .load_revision_source(profile.id, revision_id)
                    .ok()
                    .and_then(|source| inspect_profile(&source).ok()),
                None => None,
            };
            Ok(SubscriptionOverview {
                profile: PublicProfileRecord::from(&profile),
                summary,
                revision_count: revisions.len(),
                latest_fetched_at: latest.map(|revision| revision.fetched_at),
                latest_metadata: latest.and_then(|revision| revision.subscription.clone()),
                latest_validation: latest.map(|revision| revision.validation.clone()),
                active: active_profile_id == Some(profile.id),
                status: storage.subscription_status(profile.id),
            })
        })
        .collect()
}

pub fn profile_details(app: &AppHandle, profile_id: Uuid) -> AppResult<ProfileDetails> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let revisions = storage.list_revisions(profile_id)?;
    let summary = match profile.active_revision_id {
        Some(revision_id) => {
            let source = storage.load_revision_source(profile_id, revision_id)?;
            Some(inspect_profile(&source).map_err(AppError::Config)?)
        }
        None => None,
    };
    Ok(ProfileDetails {
        profile: PublicProfileRecord::from(&profile),
        revisions,
        summary,
    })
}

pub async fn create_inline_profile(
    app: &AppHandle,
    display_name: String,
    source: String,
) -> AppResult<ProfileOperationResult> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.create_profile(
        display_name.clone(),
        ProfileSource::Inline {
            label: display_name,
        },
    )?;
    match persist_candidate(app, &storage, profile.clone(), source, None, true).await {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = storage.delete_profile(profile.id);
            Err(error)
        }
    }
}

pub async fn create_subscription_profile(
    app: &AppHandle,
    display_name: String,
    url: String,
    user_agent: String,
) -> AppResult<ProfileOperationResult> {
    let storage = AppStorage::from_app(app)?;
    if let Some(existing) = storage.list_profiles()?.into_iter().find(|profile| {
        matches!(
            &profile.source,
            ProfileSource::RemoteSubscription {
                url: existing_url,
                ..
            } if existing_url == &url
        )
    }) {
        let mut result = refresh_profile(app, existing.id).await?;
        result.profile = activate_profile(app, existing.id, Some(result.revision.id))
            .await?
            .profile;
        return Ok(result);
    }
    let fetcher = SubscriptionFetcher::new()?;
    let fetched = fetcher.fetch(&url, &user_agent, None, None).await?;
    let usage = fetched.metadata.usage.clone();
    let source = fetched
        .content
        .ok_or_else(|| AppError::Subscription("订阅没有返回内容".to_string()))?;
    let profile = storage.create_profile(
        display_name,
        ProfileSource::RemoteSubscription { url, user_agent },
    )?;
    match persist_candidate(
        app,
        &storage,
        profile.clone(),
        source,
        Some(fetched.metadata),
        true,
    )
    .await
    {
        Ok(result) => {
            save_subscription_observation(&storage, profile.id, usage.as_ref(), None)?;
            Ok(result)
        }
        Err(error) => {
            let _ = storage.delete_profile(profile.id);
            Err(error)
        }
    }
}

fn unchanged_operation_result(
    profile: &ProfileRecord,
    revision: ConfigRevision,
    source: &str,
) -> AppResult<ProfileOperationResult> {
    Ok(ProfileOperationResult {
        profile: PublicProfileRecord::from(profile),
        revision,
        summary: inspect_profile(source).map_err(AppError::Config)?,
        updated: false,
    })
}

fn unchanged_operation_if_source_matches(
    storage: &AppStorage,
    profile: &ProfileRecord,
    revision: &ConfigRevision,
    candidate_source: &str,
) -> AppResult<Option<ProfileOperationResult>> {
    let previous_source = storage.load_revision_source(profile.id, revision.id)?;
    if previous_source != candidate_source {
        return Ok(None);
    }
    unchanged_operation_result(profile, revision.clone(), &previous_source).map(Some)
}

pub async fn refresh_profile(
    app: &AppHandle,
    profile_id: Uuid,
) -> AppResult<ProfileOperationResult> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let ProfileSource::RemoteSubscription { url, user_agent } = &profile.source else {
        return Err(AppError::Conflict("该配置不是远程订阅".to_string()));
    };
    match refresh_subscription_candidate(app, &storage, &profile, url, user_agent).await {
        Ok((result, usage)) => {
            save_subscription_observation(&storage, profile_id, usage.as_ref(), None)?;
            Ok(result)
        }
        Err(error) => {
            save_subscription_observation(&storage, profile_id, None, Some(&error))?;
            Err(error)
        }
    }
}

fn save_subscription_observation(
    storage: &AppStorage,
    profile_id: Uuid,
    usage: Option<&SubscriptionUsage>,
    error: Option<&AppError>,
) -> AppResult<()> {
    storage
        .record_subscription_check(profile_id, usage, error)
        .map_err(|_| {
            AppError::Io(
                if error.is_some() {
                    "订阅更新失败，且检查记录未能保存；请检查本地存储后重试"
                } else {
                    "订阅已获取，但检查记录未能保存；已保留配置结果，请检查本地存储"
                }
                .into(),
            )
        })
}

async fn refresh_subscription_candidate(
    app: &AppHandle,
    storage: &AppStorage,
    profile: &ProfileRecord,
    url: &str,
    user_agent: &str,
) -> AppResult<(ProfileOperationResult, Option<SubscriptionUsage>)> {
    let profile_id = profile.id;
    let latest = storage.list_revisions(profile_id)?.into_iter().next();
    let etag = latest
        .as_ref()
        .and_then(|revision| revision.subscription.as_ref())
        .and_then(|metadata| metadata.etag.as_deref());
    let last_modified = latest
        .as_ref()
        .and_then(|revision| revision.subscription.as_ref())
        .and_then(|metadata| metadata.last_modified.as_deref());
    let fetched = SubscriptionFetcher::new()?
        .fetch(url, user_agent, etag, last_modified)
        .await?;
    let usage = fetched.metadata.usage.clone();
    if fetched.not_modified {
        let revision = latest.ok_or_else(|| AppError::NotFound("当前订阅版本".to_string()))?;
        let source = storage.load_revision_source(profile_id, revision.id)?;
        return unchanged_operation_result(profile, revision, &source)
            .map(|result| (result, usage));
    }
    let source = fetched
        .content
        .ok_or_else(|| AppError::Subscription("订阅内容为空".to_string()))?;
    // The fetch stays outside the configuration transaction. Once mutation is
    // serialized, re-read the latest revision so concurrent refreshes cannot
    // both persist and hot-reload the same response body.
    let permit = user_rules::acquire_configuration(app)?;
    let profile = storage.load_profile(profile_id)?;
    if let Some(revision) = storage.list_revisions(profile_id)?.into_iter().next() {
        if let Some(result) =
            unchanged_operation_if_source_matches(storage, &profile, &revision, &source)?
        {
            return Ok((result, usage));
        }
    }
    persist_candidate_with_permit(
        app,
        storage,
        profile,
        source,
        Some(fetched.metadata),
        false,
        &permit,
    )
    .await
    .map(|result| (result, usage))
}

pub async fn activate_profile(
    app: &AppHandle,
    profile_id: Uuid,
    revision_id: Option<Uuid>,
) -> AppResult<ProfileDetails> {
    let permit = user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let revision_id = revision_id
        .or(profile.active_revision_id)
        .ok_or_else(|| AppError::NotFound("配置没有可激活版本".to_string()))?;
    let effective = activation_candidate(&storage, profile_id, revision_id)?;
    user_rules::apply_profile_config(
        app,
        &storage,
        &effective.yaml,
        || commit_active_selection(&storage, profile_id, revision_id, &effective.yaml).map(|_| ()),
        &permit,
    )
    .await?;
    profile_details(app, profile_id)
}

fn activation_candidate(
    storage: &AppStorage,
    profile_id: Uuid,
    revision_id: Uuid,
) -> AppResult<crate::effective::EffectiveConfig> {
    let profile = storage.load_profile(profile_id)?;
    let revision = storage.load_revision(profile_id, revision_id)?;
    let source = storage.load_revision_source(profile_id, revision_id)?;
    build_effective_config_with_policy(
        &source,
        &storage.settings()?,
        profile.routing_mode,
        Some(&revision.openai_policy),
    )
}

pub async fn rollback_profile(app: &AppHandle, profile_id: Uuid) -> AppResult<ProfileDetails> {
    let profile = AppStorage::from_app(app)?.load_profile(profile_id)?;
    let revision_id = profile
        .last_known_good_revision_id
        .ok_or_else(|| AppError::Conflict("没有可回滚的稳定版本".to_string()))?;
    activate_profile(app, profile_id, Some(revision_id)).await
}

pub fn delete_profile(app: &AppHandle, profile_id: Uuid) -> AppResult<()> {
    AppStorage::from_app(app)?.delete_profile(profile_id)
}

pub fn set_routing_mode(
    app: &AppHandle,
    profile_id: Uuid,
    mode: RoutingMode,
) -> AppResult<ProfileDetails> {
    let storage = AppStorage::from_app(app)?;
    let mut profile = storage.load_profile(profile_id)?;
    profile.routing_mode = mode;
    profile.updated_at = Utc::now();
    storage.save_profile(&profile)?;
    profile_details(app, profile_id)
}

async fn persist_candidate(
    app: &AppHandle,
    storage: &AppStorage,
    profile: ProfileRecord,
    source: String,
    metadata: Option<crate::models::SubscriptionMetadata>,
    force_activate: bool,
) -> AppResult<ProfileOperationResult> {
    let permit = user_rules::acquire_configuration(app)?;
    persist_candidate_with_permit(
        app,
        storage,
        profile,
        source,
        metadata,
        force_activate,
        &permit,
    )
    .await
}

async fn persist_candidate_with_permit(
    app: &AppHandle,
    storage: &AppStorage,
    profile: ProfileRecord,
    source: String,
    metadata: Option<crate::models::SubscriptionMetadata>,
    force_activate: bool,
    permit: &user_rules::ConfigurationMutationPermit,
) -> AppResult<ProfileOperationResult> {
    // Fetching can overlap unrelated work; take profile policy/settings only
    // after entering the short mutation transaction, never from the fetch start.
    let profile = storage.load_profile(profile.id)?;
    let activate_app = force_activate || storage.state()?.active_profile_id == Some(profile.id);
    let settings = storage.settings()?;
    let effective = build_effective_config_with_policy(
        &source,
        &settings,
        profile.routing_mode,
        Some(&profile.openai_policy),
    )?;
    let validation = ValidationReport {
        valid: true,
        warnings: effective.summary.warnings.clone(),
        errors: Vec::new(),
        native_core_validated: true,
    };
    let mut revision = None;
    if activate_app {
        user_rules::apply_profile_config(
            app,
            storage,
            &effective.yaml,
            || {
                let saved = storage.save_revision(
                    profile.id,
                    &source,
                    &effective.yaml,
                    metadata,
                    validation,
                    profile.openai_policy.clone(),
                )?;
                commit_active_selection(storage, profile.id, saved.id, &effective.yaml)?;
                revision = Some(saved);
                Ok(())
            },
            permit,
        )
        .await?;
    } else {
        user_rules::validate_config(app, &effective.yaml).await?;
        let saved = storage.save_revision(
            profile.id,
            &source,
            &effective.yaml,
            metadata,
            validation,
            profile.openai_policy.clone(),
        )?;
        if let Err(error) = storage.update_profile_revision(profile.id, saved.id) {
            storage.save_profile(&profile)?;
            return Err(error);
        }
        revision = Some(saved);
    }
    let revision = revision.ok_or_else(|| AppError::Runtime("配置事务未生成版本".to_string()))?;
    let profile = storage.load_profile(profile.id)?;
    Ok(ProfileOperationResult {
        profile: PublicProfileRecord::from(&profile),
        revision,
        summary: effective.summary,
        updated: true,
    })
}

/// Roll back all selection files if any atomic file replacement fails. Source
/// revisions stay immutable; active.yaml uses a freshly merged local overlay.
pub(crate) fn commit_active_selection(
    storage: &AppStorage,
    profile_id: Uuid,
    revision_id: Uuid,
    effective: &str,
) -> AppResult<ProfileRecord> {
    let previous_profile = storage.load_profile(profile_id)?;
    let previous_state = storage.state()?;
    let previous_config = storage.active_runtime_config()?;
    match storage.activate_revision_with_config(profile_id, revision_id, effective) {
        Ok(profile) => Ok(profile),
        Err(error) => {
            let rollback_errors = [
                storage.save_profile(&previous_profile),
                storage.save_state(&previous_state),
                storage.restore_active_runtime_config(previous_config.as_deref()),
            ]
            .into_iter()
            .filter_map(Result::err)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
            if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(AppError::Io(format!(
                    "配置选择失败且部分文件恢复失败：{error}；{}",
                    rollback_errors.join("；")
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OpenAiPolicy;
    use crate::user_rules::{UserRule, UserRulesDocument};

    #[test]
    fn subscription_observation_write_failure_is_explicit_and_preserves_saved_data() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://subscription.example.invalid/config".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .expect("profile");
        save_subscription_observation(&storage, profile.id, None, None).expect("initial record");
        let directory = root.path().join("profiles").join(profile.id.to_string());
        let status = directory.join("subscription-status.json");
        let before = std::fs::read(&status).expect("saved status");
        let before_profile = std::fs::read(directory.join("metadata.json")).expect("profile bytes");
        std::fs::create_dir(directory.join(".subscription-status.json.backup"))
            .expect("block atomic replacement");
        let error = save_subscription_observation(&storage, profile.id, None, None)
            .expect_err("must report cache write failure");
        assert!(error.to_string().contains("订阅已获取，但检查记录未能保存"));
        assert!(!error
            .to_string()
            .contains(&root.path().to_string_lossy().to_string()));
        assert_eq!(std::fs::read(&status).expect("unchanged status"), before);
        assert_eq!(
            std::fs::read(directory.join("metadata.json")).expect("unchanged profile"),
            before_profile
        );
    }

    #[test]
    fn activating_old_revision_rebuilds_current_global_overrides() {
        let root = std::env::temp_dir().join(format!("routedeck-activation-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::Inline {
                    label: "fixture".into(),
                },
            )
            .expect("profile");
        let source = "proxies: []\nproxy-groups: []\nrules: [MATCH,DIRECT]\n";
        // Use a quoted one-line sequence: each rule is a single YAML scalar.
        let source = source.replace("[MATCH,DIRECT]", "['MATCH,DIRECT']");
        let revision = storage
            .save_revision(
                profile.id,
                &source,
                "stale-cache: true\n",
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy::default(),
            )
            .expect("revision");
        let mut document = UserRulesDocument::default();
        document.rules.push(UserRule {
            id: "current".into(),
            enabled: true,
            rule: "DOMAIN,example.com,REJECT".into(),
            note: String::new(),
        });
        storage
            .save_user_rules(&document)
            .expect("current global rule");
        let candidate = activation_candidate(&storage, profile.id, revision.id).expect("candidate");
        assert!(!candidate.yaml.contains("stale-cache"));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&candidate.yaml).expect("yaml");
        assert_eq!(yaml["rules"][0].as_str(), Some("DOMAIN,example.com,REJECT"));
        commit_active_selection(&storage, profile.id, revision.id, &candidate.yaml)
            .expect("select");
        assert_eq!(
            storage.active_runtime_config().expect("snapshot"),
            Some(candidate.yaml)
        );
        assert_eq!(
            storage
                .load_revision_effective(profile.id, revision.id)
                .expect("immutable cache"),
            "stale-cache: true\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn identical_refresh_source_reuses_the_existing_revision() {
        let root = std::env::temp_dir().join(format!("routedeck-refresh-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://subscription.example.invalid/config".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .expect("profile");
        let source = "proxies: []\nproxy-groups: []\nrules: ['MATCH,DIRECT']\n";
        let revision = storage
            .save_revision(
                profile.id,
                source,
                source,
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy::default(),
            )
            .expect("revision");

        let result = unchanged_operation_if_source_matches(&storage, &profile, &revision, source)
            .expect("comparison")
            .expect("unchanged result");
        assert!(!result.updated);
        assert_eq!(result.revision.id, revision.id);
        assert_eq!(
            storage.list_revisions(profile.id).expect("revisions").len(),
            1
        );
        assert!(unchanged_operation_if_source_matches(
            &storage,
            &profile,
            &revision,
            "proxies: []\nrules: []\n",
        )
        .expect("comparison")
        .is_none());
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
