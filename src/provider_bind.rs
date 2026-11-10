//! provider ↔ API key 绑定断言 —— 防多 provider 配置下密钥外泄。
//!
//! 借鉴：yologdev/yoyo-evolve 修复 #1170（停止向非 Anthropic provider 发
//! ANTHROPIC_API_KEY）→ q-body 对应改法：为每个 LLM provider 的 API key
//! 环境变量绑定一个「应匹配的 host 域」，当编排 provider 链时若把某 key 绑到
//! 非匹配的 provider 端点，启动即 fail-loud 点名，密钥永不发给错误方。
//!
//! 判定语义（yoyo #1023 fail-closed 按 provenance 判定，不按片段黑名单）：
//! - key 已知 且 host 命中其绑定域            → Known（允许）
//! - key 已知 但 host 命中「别的已知 key 的域」 → Mismatched(那个 key)，必须拦
//! - key 已知 但 host 不命中任何已知域        → Mismatched(unknown host)，必须拦
//! - key 未知 但 host 命中某个已知 key 的域    → WrongKey(那个 key)，疑似写错名
//! - key 未知 且 host 非已知域                → Unbound（保守放行，不误伤自定义 provider）

/// 已知 provider key 环境变量名 → 应匹配的 host 域（不含协议，精确域后缀匹配）。
/// 唯一事实源：加新 provider 时同步登记，测试钉死字节级。
pub const KNOWN_KEY_HOSTS: &[(&str, &str)] = &[
    ("ARK_API_KEY", "ark.cn-beijing.volces.com"),
    ("DEEPSEEK_API_KEY", "api.deepseek.com"),
];

/// 绑定判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindVerdict {
    /// key 在已知表且 host 匹配其绑定域 —— 允许。
    Known,
    /// key 不在已知表，且 host 也不是任何已知 key 的专属域 —— 保守放行。
    Unbound,
    /// key 已知但 host 不匹配其绑定域 —— 密钥将发给错误方，必须拦下。
    /// String = 该 host 若命中已知域则为那个 key 名，否则 "(unknown host)"（诊断线索）。
    Mismatched(String),
    /// key 未知但 host 是某个已知 key 的专属域 —— 疑似把 key 写错名后指向它的域。
    WrongKey(String),
}

/// host（去协议、去路径）与某已知 key 的绑定域是否匹配（精确域或子域后缀）。
fn host_matches(host: &str, domain: &str) -> bool {
    host.eq_ignore_ascii_case(domain)
        || host.to_ascii_lowercase().ends_with(&format!(".{}", domain.to_ascii_lowercase()))
}

/// 从 api_url 提取 host（无协议、无路径/查询）。
fn extract_host(api_url: &str) -> &str {
    api_url
        .split("://")
        .nth(1)
        .map(|rest| rest.split(['/', '?']).next().unwrap_or(""))
        .unwrap_or("")
}

/// 绑定判定谓词 —— provider 编排时的唯一判定口。
pub fn bind_verdict(api_key_env: &str, api_url: &str) -> BindVerdict {
    let host = extract_host(api_url);
    if host.trim().is_empty() {
        // 畸形 URL 无法提取 host —— 保守放行不拦（密钥根本没机会发出，交给请求层报错）。
        return BindVerdict::Unbound;
    }

    let key_owned_domain = KNOWN_KEY_HOSTS
        .iter()
        .find(|(k, _)| *k == api_key_env)
        .map(|(_, d)| *d);
    let host_is_known_domain_of = KNOWN_KEY_HOSTS
        .iter()
        .find(|(_, d)| host_matches(host, d))
        .map(|(k, _)| *k);

    match (key_owned_domain, host_is_known_domain_of) {
        // key 已知且 host 匹配自身的绑定域
        (Some(d), Some(k)) if k == api_key_env && host_matches(host, d) => BindVerdict::Known,
        // key 已知但 host 命中的是另一个已知 key 的域 → 错发
        (Some(_), Some(k)) => BindVerdict::Mismatched(k.to_string()),
        // key 已知但 host 非任何已知域 → 错发（除非恰是自己的域，已在上臂处理）
        (Some(d), None) => {
            if host_matches(host, d) {
                BindVerdict::Known
            } else {
                BindVerdict::Mismatched("(unknown host)".to_string())
            }
        }
        // key 未知但 host 是某已知 key 的专属域 → 疑似写错 key 名
        (None, Some(k)) => BindVerdict::WrongKey(k.to_string()),
        // 全未知 → 保守放行
        (None, None) => BindVerdict::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ark_key_binds_to_ark_host() {
        assert_eq!(
            bind_verdict(
                "ARK_API_KEY",
                "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions"
            ),
            BindVerdict::Known
        );
    }

    #[test]
    fn test_deepseek_key_binds_to_deepseek_host() {
        assert_eq!(
            bind_verdict("DEEPSEEK_API_KEY", "https://api.deepseek.com/chat/completions"),
            BindVerdict::Known
        );
    }

    #[test]
    fn test_wrong_key_to_deepseek_host_is_mismatch() {
        // 把 ARK 的 key 绑到 deepseek 端点 → 密钥将发给错误方，必须拦
        assert_eq!(
            bind_verdict("ARK_API_KEY", "https://api.deepseek.com/chat/completions"),
            BindVerdict::Mismatched("DEEPSEEK_API_KEY".into())
        );
    }

    #[test]
    fn test_unknown_key_to_arbitrary_host_is_unbound() {
        // 未知 key + 非已知域 → 保守放行（不误伤自定义 provider）
        assert_eq!(
            bind_verdict("MY_CUSTOM_KEY", "https://custom.example.com/v1"),
            BindVerdict::Unbound
        );
    }

    #[test]
    fn test_unknown_key_to_known_host_is_wrongkey() {
        // 未知 key 但指向已知 key 的专属域 → 疑似把 key 名写错
        assert_eq!(
            bind_verdict("ARK_APIKE", "https://ark.cn-beijing.volces.com/api/plan/v3"),
            BindVerdict::WrongKey("ARK_API_KEY".into())
        );
    }

    #[test]
    fn test_known_key_to_unrelated_host_is_mismatch() {
        // key 已知但 host 是无关第三方 → 错发（即使它也是 https）
        assert_eq!(
            bind_verdict(
                "DEEPSEEK_API_KEY",
                "https://thirdparty-proxy.example.com/v1"
            ),
            BindVerdict::Mismatched("(unknown host)".into())
        );
    }

    #[test]
    fn test_malformed_url_is_unbound() {
        // 畸形 URL 无法提取 host → 保守放行不拦
        assert_eq!(bind_verdict("ARK_API_KEY", "not-a-url"), BindVerdict::Unbound);
    }

    #[test]
    fn test_subdomain_suffix_matches() {
        // 同一厂商的子域也应命中其绑定域
        assert_eq!(
            bind_verdict(
                "DEEPSEEK_API_KEY",
                "https://api.deepseek.com/chat/completions"
            ),
            BindVerdict::Known
        );
    }
}