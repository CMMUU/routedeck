//! A reversible lease on two TOML keys, not ownership of the whole user's configuration.
use super::{files, RouteDocument, RouteMode};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::Manager;
use toml_edit::{value, DocumentMut, Item, Table};

const PROVIDER: &str = "routedeck";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lease {
    config_path: PathBuf,
    original: Option<String>,
    installed: String,
    backup_name: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatus {
    pub config_revision: String,
    pub attached: bool,
    pub has_backup: bool,
    pub provider: String,
    pub endpoint: Option<String>,
    pub warning: Option<String>,
    pub backup_path: Option<String>,
}
pub fn config_path(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    let home = match std::env::var_os("CODEX_HOME") {
        Some(path) => PathBuf::from(path),
        None => app
            .path()
            .home_dir()
            .map_err(|_| AppError::Io("无法定位 Codex 配置目录".into()))?
            .join(".codex"),
    };
    if !home.is_absolute() {
        return Err(AppError::InvalidInput("CODEX_HOME 必须是绝对路径".into()));
    }
    Ok(home.join("config.toml"))
}
fn parse(text: &Option<String>) -> AppResult<DocumentMut> {
    text.as_deref()
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .parse()
        .map_err(|_| AppError::Config("Codex TOML 配置无法解析，未覆盖".into()))
}
fn revision(text: &Option<String>) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!(
            "{}:{}",
            text.is_some(),
            text.as_deref().unwrap_or("")
        ))
    )
}
fn read_lease(dir: &Path) -> AppResult<Option<Lease>> {
    files::read_optional(&dir.join("codex-lease.json"))?
        .map(|s| {
            serde_json::from_str(&s)
                .map_err(|_| AppError::Config("Codex 恢复记录损坏，请保留备份并手动核对".into()))
        })
        .transpose()
}
pub fn has_lease(dir: &Path) -> AppResult<bool> {
    Ok(read_lease(dir)?.is_some())
}
fn provider_item(doc: &DocumentMut) -> Option<&Item> {
    doc.get("model_providers").and_then(|x| x.get(PROVIDER))
}
fn owns(doc: &DocumentMut, installed: &DocumentMut) -> bool {
    doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER)
        && provider_item(doc).map(ToString::to_string)
            == provider_item(installed).map(ToString::to_string)
}
pub fn status(config: &Path, dir: &Path) -> AppResult<CodexStatus> {
    let text = files::read_optional(config)?;
    let lease = read_lease(dir)?;
    let doc = match parse(&text) {
        Ok(doc) => doc,
        Err(_) => {
            return Ok(CodexStatus {
                config_revision: revision(&text),
                attached: false,
                has_backup: lease.is_some(),
                provider: "配置无法解析".into(),
                endpoint: None,
                warning: Some(
                    "Codex TOML 配置无法解析；接入与恢复会拒绝覆盖，请先修复配置。".into(),
                ),
                backup_path: lease
                    .as_ref()
                    .map(|l| dir.join(&l.backup_name).display().to_string()),
            })
        }
    };
    let provider = doc
        .get("model_provider")
        .and_then(Item::as_str)
        .unwrap_or("openai")
        .to_string();
    let raw = doc
        .get("model_providers")
        .and_then(|v| v.get(&provider))
        .and_then(|v| v.get("base_url"))
        .and_then(Item::as_str);
    let endpoint = raw.and_then(|s| url::Url::parse(s).ok()).map(|u| {
        format!(
            "{}://{}{}",
            u.scheme(),
            u.host_str().unwrap_or("?"),
            u.port().map(|p| format!(":{p}")).unwrap_or_default()
        )
    });
    let attached = lease.as_ref().is_some_and(|l| {
        l.config_path == config && parse(&Some(l.installed.clone())).is_ok_and(|i| owns(&doc, &i))
    });
    let warning = if lease.is_some() && !attached {
        Some("配置已被其他程序修改；Serylane 不会覆盖冲突字段。请查看备份后恢复。".into())
    } else if doc.contains_key("profile") {
        Some("检测到 Codex profile；其设置可能覆盖当前接入，请在新会话核对实际连接。".into())
    } else if provider.contains("cc-switch") {
        Some("当前使用 CC Switch 接入；恢复后仍需其路由服务运行。".into())
    } else {
        None
    };
    Ok(CodexStatus {
        config_revision: revision(&text),
        attached,
        has_backup: lease.is_some(),
        provider,
        endpoint,
        warning,
        backup_path: lease
            .as_ref()
            .map(|l| dir.join(&l.backup_name).display().to_string()),
    })
}
pub fn attach(config: &Path, dir: &Path, route: &RouteDocument, expected: &str) -> AppResult<()> {
    if has_lease(dir)? {
        return Err(AppError::Conflict(
            "已有 Codex 接入备份，请先恢复再更换模式".into(),
        ));
    }
    let before = files::read_optional(config)?;
    if revision(&before) != expected {
        return Err(AppError::Conflict(
            "Codex 配置已变化，请刷新后重新确认".into(),
        ));
    }
    let mut doc = parse(&before)?;
    if provider_item(&doc).is_some()
        || doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER)
    {
        return Err(AppError::Conflict(
            "Codex 已有同名提供方（兼容标识 routedeck），未覆盖".into(),
        ));
    }
    if doc.get("model_providers").is_some_and(|v| !v.is_table()) {
        return Err(AppError::Conflict(
            "model_providers 使用非标准表结构，请先自行检查".into(),
        ));
    }
    let mut provider = Table::new();
    // Display only: existing leases retain their exact installed provider table.
    // Keep PROVIDER and the restore comparison stable across brand changes.
    provider["name"] = value("Serylane 本地路由");
    provider["base_url"] = value(route.endpoint());
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    provider["supports_websockets"] = value(route.settings.mode == RouteMode::Native);
    doc["model_provider"] = value(PROVIDER);
    if !doc.contains_key("model_providers") {
        doc["model_providers"] = Item::Table(Table::new());
    }
    doc["model_providers"][PROVIDER] = Item::Table(provider);
    let installed = doc.to_string();
    files::private_directory(dir)?;
    let parent = config
        .parent()
        .ok_or_else(|| AppError::Io("Codex 配置父目录不存在".into()))?;
    // Never create a Codex installation or touch auth.json on behalf of an absent app.
    if !parent.is_dir() {
        return Err(AppError::NotFound(
            "Codex 配置目录不存在，请先正常启动 Codex".into(),
        ));
    }
    let backup_name = format!("codex-backup-{}.toml", uuid::Uuid::new_v4());
    files::atomic_write(&dir.join(&backup_name), before.as_deref().unwrap_or(""))?;
    let lease = Lease {
        config_path: config.into(),
        original: before.clone(),
        installed: installed.clone(),
        backup_name,
    };
    // Journal first; recovery is safe even if the process dies before the config rename.
    files::atomic_write(
        &dir.join("codex-lease.json"),
        &serde_json::to_string(&lease).map_err(|_| AppError::Config("无法创建恢复记录".into()))?,
    )?;
    files::compare_and_write(config, &before, &installed)
}
pub fn restore(config: &Path, dir: &Path, expected: Option<&str>) -> AppResult<()> {
    let Some(lease) = read_lease(dir)? else {
        return Ok(());
    };
    if lease.config_path != config {
        return Err(AppError::Conflict(
            "Codex 配置目录已变化，未向不同目录恢复".into(),
        ));
    }
    let before = files::read_optional(config)?;
    if expected.is_some_and(|e| revision(&before) != e) {
        return Err(AppError::Conflict("Codex 配置已变化，请刷新后重试".into()));
    }
    if before != lease.original {
        let mut doc = parse(&before)?;
        let installed = parse(&Some(lease.installed.clone()))?;
        if !owns(&doc, &installed) {
            return Err(AppError::Conflict(
                "Codex 接入已被其他程序修改，未覆盖；原始备份仍保留".into(),
            ));
        }
        let original = parse(&lease.original)?;
        if let Some(item) = original.get("model_provider") {
            doc["model_provider"] = item.clone();
        } else {
            doc.remove("model_provider");
        }
        if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
            providers.remove(PROVIDER);
            if providers.is_empty() && !original.contains_key("model_providers") {
                doc.remove("model_providers");
            }
        }
        let after = if before.as_deref() == Some(lease.installed.as_str()) {
            lease.original.clone().unwrap_or_default()
        } else {
            doc.to_string()
        };
        files::compare_and_write(config, &before, &after)?;
    }
    files::reject_links(&dir.join("codex-lease.json"))?;
    std::fs::remove_file(dir.join("codex-lease.json"))?;
    // The timestamped backup is deliberately retained for manual recovery.
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        // Keep the TempDir alive, but do not pass macOS /var aliases to the
        // production I/O helpers that intentionally reject symlink ancestors.
        let root_path = root.path().canonicalize().unwrap();
        let config = root_path.join("config.toml");
        let dir = root_path.join("route");
        std::fs::write(&config, text).unwrap();
        std::fs::write(root_path.join("auth.json"), "unchanged-test-auth").unwrap();
        (root, config, dir)
    }
    #[test]
    fn attach_restore_preserve_auth_comments_and_unrelated_edits() {
        let (root, config, dir) = fixture("# my comment\nmodel = \"sample\"\n");
        let status = status(&config, &dir).unwrap();
        attach(
            &config,
            &dir,
            &RouteDocument::default(),
            &status.config_revision,
        )
        .unwrap();
        assert!(self::status(&config, &dir).unwrap().attached);
        let content = std::fs::read_to_string(&config)
            .unwrap()
            .replace("sample", "changed-model");
        std::fs::write(&config, content).unwrap();
        restore(&config, &dir, None).unwrap();
        let result = std::fs::read_to_string(&config).unwrap();
        assert!(result.contains("# my comment"));
        assert!(result.contains("changed-model"));
        assert!(!result.contains("routedeck"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("auth.json")).unwrap(),
            "unchanged-test-auth"
        );
    }
    #[test]
    fn serylane_recognizes_and_restores_a_routedeck_073_lease() {
        let (root, config, dir) = fixture("# original\nmodel = \"sample\"\n");
        let original = std::fs::read_to_string(&config).unwrap();
        attach(
            &config,
            &dir,
            &RouteDocument::default(),
            &status(&config, &dir).unwrap().config_revision,
        )
        .unwrap();
        let attached = std::fs::read_to_string(&config).unwrap();
        assert!(attached.contains("model_provider = \"routedeck\""));
        assert!(attached.contains("name = \"Serylane 本地路由\""));

        // Recreate the previous release's display name inside an isolated lease;
        // the new application must read it without migrating config or journal.
        let legacy = attached.replace("Serylane 本地路由", "RouteDeck 本地路由");
        let mut lease = read_lease(&dir).unwrap().unwrap();
        lease.installed = legacy.clone();
        let journal = serde_json::to_string(&lease).unwrap();
        std::fs::write(&config, &legacy).unwrap();
        std::fs::write(dir.join("codex-lease.json"), &journal).unwrap();
        assert!(status(&config, &dir).unwrap().attached);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), legacy);
        assert_eq!(
            std::fs::read_to_string(dir.join("codex-lease.json")).unwrap(),
            journal
        );

        // A third-party edit remains a conflict, even when only the name changes.
        let changed = legacy.replace("RouteDeck 本地路由", "another router");
        std::fs::write(&config, &changed).unwrap();
        assert!(restore(&config, &dir, None).is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), changed);
        assert!(has_lease(&dir).unwrap());

        std::fs::write(&config, &legacy).unwrap();
        restore(&config, &dir, None).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
        assert!(dir.join(lease.backup_name).is_file());
        assert_eq!(
            std::fs::read_to_string(root.path().join("auth.json")).unwrap(),
            "unchanged-test-auth"
        );
    }
    #[test]
    fn conflicts_never_overwrite_another_router() {
        let (_root, config, dir) = fixture("model_provider = \"cc-switch-official\"\n");
        attach(
            &config,
            &dir,
            &RouteDocument::default(),
            &status(&config, &dir).unwrap().config_revision,
        )
        .unwrap();
        let changed = std::fs::read_to_string(&config).unwrap().replace(
            "model_provider = \"routedeck\"",
            "model_provider = \"another\"",
        );
        std::fs::write(&config, &changed).unwrap();
        assert!(restore(&config, &dir, None).is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), changed);
        assert!(has_lease(&dir).unwrap());
    }
    #[test]
    fn malformed_config_status_is_visible_but_attach_cannot_overwrite_it() {
        let (_root, config, dir) = fixture("model_provider = [\n");
        let state = status(&config, &dir).unwrap();
        assert!(state.warning.is_some());
        assert!(attach(
            &config,
            &dir,
            &RouteDocument::default(),
            &state.config_revision
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            "model_provider = [\n"
        );
        assert!(!has_lease(&dir).unwrap());
    }
    #[test]
    fn exact_restore_and_stale_revision() {
        let (_root, config, dir) =
            fixture("# preserved\nmodel_provider = \"cc-switch-official\"\n");
        let old = std::fs::read_to_string(&config).unwrap();
        assert!(attach(&config, &dir, &RouteDocument::default(), "stale").is_err());
        assert!(!has_lease(&dir).unwrap());
        attach(
            &config,
            &dir,
            &RouteDocument::default(),
            &status(&config, &dir).unwrap().config_revision,
        )
        .unwrap();
        restore(&config, &dir, None).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), old);
    }
}
