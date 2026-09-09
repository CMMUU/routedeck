use serde::Serialize;
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;

const SUPPORTED_GROUP_TYPES: &[&str] = &["select", "url-test", "fallback"];

pub fn is_subscription_metadata_node_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    [
        "剩余流量",
        "距离下次重置",
        "下次重置",
        "套餐到期",
        "订阅到期",
        "建议：感到卡顿",
        "建议:感到卡顿",
        "到期时间",
        "官网:",
        "官网：",
        "traffic remaining",
        "reset in",
        "expire date",
        "expiration",
        "subscription info",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub format: String,
    pub node_count: usize,
    pub proxy_group_count: usize,
    pub proxy_provider_count: usize,
    pub rule_count: usize,
    pub rule_provider_count: usize,
    pub dns_configured: bool,
    pub tun_configured: bool,
    pub node_protocols: Vec<String>,
    pub proxy_group_types: Vec<String>,
    pub unsupported_group_types: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn inspect_profile(source: &str) -> Result<ProfileSummary, String> {
    let document: Value =
        serde_yaml::from_str(source).map_err(|error| format!("Mihomo YAML 解析失败: {error}"))?;
    let root = document
        .as_mapping()
        .ok_or_else(|| "Mihomo 配置根节点必须是 YAML 对象".to_string())?;

    let proxies = sequence_field(root, "proxies");
    let proxy_groups = sequence_field(root, "proxy-groups");
    let proxy_providers = mapping_field(root, "proxy-providers");
    let rules = sequence_field(root, "rules");
    let rule_providers = mapping_field(root, "rule-providers");

    let mut warnings = Vec::new();
    let mut node_protocols = BTreeSet::new();

    for (index, proxy) in proxies.iter().enumerate() {
        let Some(proxy_map) = proxy.as_mapping() else {
            warnings.push(format!("节点 {} 不是对象", index + 1));
            continue;
        };

        if let Some(protocol) = string_field(proxy_map, "type") {
            node_protocols.insert(protocol.to_string());
        } else {
            warnings.push(format!("节点 {} 缺少 type", index + 1));
        }

        for required in ["name", "server", "port"] {
            if !proxy_map.contains_key(Value::String(required.to_string())) {
                warnings.push(format!("节点 {} 缺少 {required}", index + 1));
            }
        }
    }

    let mut proxy_group_types = BTreeSet::new();
    let mut unsupported_group_types = BTreeSet::new();
    for (index, group) in proxy_groups.iter().enumerate() {
        let Some(group_map) = group.as_mapping() else {
            warnings.push(format!("代理组 {} 不是对象", index + 1));
            continue;
        };

        let Some(group_type) = string_field(group_map, "type") else {
            warnings.push(format!("代理组 {} 缺少 type", index + 1));
            continue;
        };

        proxy_group_types.insert(group_type.to_string());
        if !SUPPORTED_GROUP_TYPES.contains(&group_type) {
            unsupported_group_types.insert(group_type.to_string());
        }
    }

    if proxies.is_empty() && proxy_providers.is_empty() {
        warnings.push("配置中没有 proxies 或 proxy-providers".to_string());
    }
    if !unsupported_group_types.is_empty() {
        warnings.push(format!(
            "存在尚未纳入第一版 UI 的代理组类型: {}",
            unsupported_group_types
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if root.contains_key(Value::String("script".to_string())) {
        warnings.push("检测到 script，第一版仅保留原始配置，不执行脚本".to_string());
    }
    if root.contains_key(Value::String("external-controller".to_string())) {
        warnings.push(
            "订阅包含 external-controller，应用会覆盖为本机回环地址并使用独立 secret".to_string(),
        );
    }

    Ok(ProfileSummary {
        format: "clash-meta/mihomo-yaml".to_string(),
        node_count: proxies.len(),
        proxy_group_count: proxy_groups.len(),
        proxy_provider_count: proxy_providers.len(),
        rule_count: rules.len(),
        rule_provider_count: rule_providers.len(),
        dns_configured: root.contains_key(Value::String("dns".to_string())),
        tun_configured: root.contains_key(Value::String("tun".to_string())),
        node_protocols: node_protocols.into_iter().collect(),
        proxy_group_types: proxy_group_types.into_iter().collect(),
        unsupported_group_types: unsupported_group_types.into_iter().collect(),
        warnings,
    })
}

fn sequence_field<'a>(root: &'a Mapping, name: &str) -> Vec<&'a Value> {
    root.get(Value::String(name.to_string()))
        .and_then(Value::as_sequence)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn mapping_field<'a>(root: &'a Mapping, name: &str) -> &'a Mapping {
    root.get(Value::String(name.to_string()))
        .and_then(Value::as_mapping)
        .unwrap_or_else(|| {
            static EMPTY: std::sync::OnceLock<Mapping> = std::sync::OnceLock::new();
            EMPTY.get_or_init(Mapping::new)
        })
}

fn string_field<'a>(map: &'a Mapping, name: &str) -> Option<&'a str> {
    map.get(Value::String(name.to_string()))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::{inspect_profile, is_subscription_metadata_node_name};

    #[test]
    fn inspects_a_minimal_mihomo_profile() {
        let summary = inspect_profile(
            r#"
proxies:
  - name: sample
    type: socks5
    server: 127.0.0.1
    port: 1080
proxy-groups:
  - name: PROXY
    type: select
    proxies: [sample]
rules:
  - MATCH,PROXY
dns:
  enable: true
"#,
        )
        .expect("profile should parse");

        assert_eq!(summary.node_count, 1);
        assert_eq!(summary.proxy_group_count, 1);
        assert_eq!(summary.rule_count, 1);
        assert!(summary.dns_configured);
        assert_eq!(summary.node_protocols, vec!["socks5"]);
    }

    #[test]
    fn reports_unsupported_group_types_without_failing_parse() {
        let summary = inspect_profile(
            r#"
proxies:
  - name: sample
    type: socks5
    server: 127.0.0.1
    port: 1080
proxy-groups:
  - name: BALANCE
    type: load-balance
    proxies: [sample]
"#,
        )
        .expect("profile should parse");

        assert_eq!(summary.unsupported_group_types, vec!["load-balance"]);
        assert!(!summary.warnings.is_empty());
    }

    #[test]
    fn rejects_non_mapping_root() {
        let error = inspect_profile("- plain-list-item").expect_err("root must be a mapping");
        assert!(error.contains("根节点"));
    }

    #[test]
    fn recognizes_subscription_metadata_nodes() {
        for name in [
            "剩余流量：15.61 GB",
            "距离下次重置剩余：29 天",
            "套餐到期：2026-09-17",
            "放丢失官网:https://example.com",
            "Traffic Remaining: 20 GB",
            "建议：感到卡顿请切换到专线节点",
        ] {
            assert!(is_subscription_metadata_node_name(name), "{name}");
        }
        assert!(!is_subscription_metadata_node_name(
            "🇸🇬【亚洲】新加坡02丨Vless"
        ));
    }
}
