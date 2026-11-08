//! 配置遮蔽启动审计 — 借鉴 yoyo-evolve `config get` 命名被遮蔽的配置文件
//!
//! 背景：q-body 无 config.rs/配置文件树，配置事实是环境变量 + 内置默认。
//! 「同 key 多来源遮蔽 → 静默回退」在 q-body 的等价物：用户显式设置了某个
//! `QBODY_*`/provider key env，但值非法（非数字 / ≤0）或为空，
//! 被 `price_per_mtok` / `cost_warn_threshold_usd` / provider key 读取
//! 静默回落默认或静默跳过 —— 用户以为配了 0.1，实际跑的是 0.44 默认。
//!
//! 本模块在启动时 scan 已知可遮蔽配置键，把「用户设置了但被忽略」的项
//! 一次性点名打 stderr 警告，防静默回退（对齐 SOUL §22 自愈核验：
//! 配置冲突可见化，修复/回退都有出勤钩子）。

/// 遮蔽问题分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowReason {
    /// 设置了但值全空白 → 被当作未设置（provider key 跳过 / 数值回落默认）
    Empty,
    /// 设置了但值非法（非数字 / ≤0）→ 数值回落默认
    InvalidValue,
}

/// 一次遮蔽审计发现
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigShadowIssue {
    /// 被遮蔽的环境变量名
    pub key: String,
    /// 取到的原始值（sensitive 键做长度打码预览，不泄完整值）
    pub value_preview: String,
    /// 遮蔽原因
    pub reason: ShadowReason,
}

/// 已知「数值型」可遮蔽配置键（单一事实源表：新增配置键在此登记）
/// value = (key, 是否为敏感凭据)
pub const NUMERIC_CONFIG_KEYS: &[(&str, bool)] = &[
    ("QBODY_COST_WARN_USD", false),
    ("QBODY_COST_INPUT_USD_PER_MTOK", false),
    ("QBODY_COST_OUTPUT_USD_PER_MTOK", false),
];

/// 已知「凭据型」provider key 配置键（set-but-empty → provider 被跳过）
pub const CREDENTIAL_CONFIG_KEYS: &[(&str, bool)] = &[
    ("ARK_API_KEY", true),
    ("DEEPSEEK_API_KEY", true),
];

/// 打码预览：非敏感键显示原始值；敏感键只显示长度（`<redacted:N>`）
pub fn preview(value: &str, is_sensitive: bool) -> String {
    if !is_sensitive {
        return value.to_owned();
    }
    format!("<redacted:{}>", value.len())
}

/// 判定单个数值键是否被遮蔽：设置了但非法（非数字 / ≤0）→ InvalidValue；
/// 设置了但全空白 → Empty；未设置或合法 → None
fn numeric_issue(key: &str, is_sensitive: bool) -> Option<ConfigShadowIssue> {
    let raw = std::env::var(key).ok()?;
    if raw.trim().is_empty() {
        return Some(ConfigShadowIssue {
            key: key.to_owned(),
            value_preview: preview(&raw, is_sensitive),
            reason: ShadowReason::Empty,
        });
    }
    let valid = raw
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| v > 0.0)
        .unwrap_or(false);
    if !valid {
        return Some(ConfigShadowIssue {
            key: key.to_owned(),
            value_preview: preview(&raw, is_sensitive),
            reason: ShadowReason::InvalidValue,
        });
    }
    None
}

/// 判定单个凭据键是否被遮蔽：设置了但全空白 → 被当作未设置（provider 跳过）
fn credential_issue(key: &str, is_sensitive: bool) -> Option<ConfigShadowIssue> {
    let raw = std::env::var(key).ok()?;
    if raw.trim().is_empty() {
        return Some(ConfigShadowIssue {
            key: key.to_owned(),
            value_preview: preview(&raw, is_sensitive),
            reason: ShadowReason::Empty,
        });
    }
    None
}

/// 审计全部已知可遮蔽配置键，返回被静默忽略的项（保序：数值键先、凭据键后）
pub fn audit_config_shadows() -> Vec<ConfigShadowIssue> {
    let mut out = Vec::new();
    for (key, sensitive) in NUMERIC_CONFIG_KEYS {
        if let Some(issue) = numeric_issue(key, *sensitive) {
            out.push(issue);
        }
    }
    for (key, sensitive) in CREDENTIAL_CONFIG_KEYS {
        if let Some(issue) = credential_issue(key, *sensitive) {
            out.push(issue);
        }
    }
    out
}

/// 单条警告行（stderr 用，trust-boundary：不泄完整敏感值，只泄已打码预览）
pub fn warn_line(issue: &ConfigShadowIssue) -> String {
    match issue.reason {
        ShadowReason::Empty => format!(
            "config shadow: {} is set but empty — treated as unset (silent default/skip)",
            issue.key
        ),
        ShadowReason::InvalidValue => format!(
            "config shadow: {} is set to '{}' but not a positive number — falling back to default (silently shadowed)",
            issue.key, issue.value_preview
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立键名测试隔离（同 10-19 env_keys 纪律：测试不污染真实配置键）
    static TEST_NUM_KEYS: [(&str, bool); 2] =
        [("QBODY_TEST_SHADOW_NUM_A", false), ("QBODY_TEST_SHADOW_NUM_B", false)];
    static TEST_CRED_KEY: (&str, bool) = ("QBODY_TEST_SHADOW_CRED", true);

    fn set_env_clean() {
        for (k, _) in TEST_NUM_KEYS.iter().chain(std::iter::once(&TEST_CRED_KEY)) {
            // SAFETY: 单线程测试进程内修改自身进程环境变量，无并发读取
            unsafe {
                std::env::remove_var(k);
            }
        }
    }

    /// 测试内 set_var（单线程测试进程内安全，同 a2a/cost.rs 先例）
    fn set_var_unsafe(k: &str, v: &str) {
        // SAFETY: 单线程测试进程内修改自身进程环境变量，无并发读取
        unsafe {
            std::env::set_var(k, v);
        }
    }

    #[test]
    fn test_legal_numeric_not_reported() {
        set_env_clean();
        set_var_unsafe(TEST_NUM_KEYS[0].0, "1.0");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false);
        assert!(issue.is_none(), "legal numeric must not be shadowed");
        set_env_clean();
    }

    #[test]
    fn test_zero_and_negative_reported_invalid() {
        set_env_clean();
        set_var_unsafe(TEST_NUM_KEYS[0].0, "0");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false).unwrap();
        assert_eq!(issue.reason, ShadowReason::InvalidValue);
        set_var_unsafe(TEST_NUM_KEYS[0].0, "-3");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false).unwrap();
        assert_eq!(issue.reason, ShadowReason::InvalidValue);
        set_env_clean();
    }

    #[test]
    fn test_non_numeric_reported_invalid() {
        set_env_clean();
        set_var_unsafe(TEST_NUM_KEYS[0].0, "abc");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false).unwrap();
        assert_eq!(issue.reason, ShadowReason::InvalidValue);
        assert_eq!(issue.value_preview, "abc");
        set_env_clean();
    }

    #[test]
    fn test_blank_numeric_reported_empty() {
        set_env_clean();
        set_var_unsafe(TEST_NUM_KEYS[0].0, "   ");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false).unwrap();
        assert_eq!(issue.reason, ShadowReason::Empty);
        set_env_clean();
    }

    #[test]
    fn test_blank_credential_reported_empty_sensitive_preview() {
        set_env_clean();
        set_var_unsafe(TEST_CRED_KEY.0, "");
        let issue = credential_issue(TEST_CRED_KEY.0, true).unwrap();
        assert_eq!(issue.reason, ShadowReason::Empty);
        // 敏感值只出长度打码，绝不泄原文
        assert_eq!(issue.value_preview, "<redacted:0>");
        set_env_clean();
    }

    #[test]
    fn test_set_credential_not_reported() {
        set_env_clean();
        set_var_unsafe(TEST_CRED_KEY.0, "sk-real");
        assert!(credential_issue(TEST_CRED_KEY.0, true).is_none());
        set_env_clean();
    }

    #[test]
    fn test_preview_redacts_sensitive_even_nonblank() {
        set_env_clean();
        set_var_unsafe(TEST_CRED_KEY.0, "ghp_abcdef123456");
        let issue = credential_issue(TEST_CRED_KEY.0, true);
        assert!(issue.is_none(), "非空白凭据不应被当成遮蔽"); // 这条应无遮蔽
        // 直接测 preview 函数本身
        assert_eq!(preview("ghp_abcdef123456", true), "<redacted:16>");
        assert_eq!(preview("0.1", false), "0.1");
        set_env_clean();
    }

    #[test]
    fn test_unset_not_reported() {
        set_env_clean();
        assert!(numeric_issue(TEST_NUM_KEYS[0].0, false).is_none());
        assert!(credential_issue(TEST_CRED_KEY.0, true).is_none());
    }

    #[test]
    fn test_warn_line_shapes() {
        set_env_clean();
        set_var_unsafe(TEST_NUM_KEYS[0].0, "abc");
        let issue = numeric_issue(TEST_NUM_KEYS[0].0, false).unwrap();
        let line = warn_line(&issue);
        assert!(line.contains("QBODY_TEST_SHADOW_NUM_A"));
        assert!(line.contains("silently shadowed"));

        set_var_unsafe(TEST_CRED_KEY.0, "  ");
        let issue = credential_issue(TEST_CRED_KEY.0, true).unwrap();
        let line = warn_line(&issue);
        assert!(line.contains("set but empty"));
        assert!(!line.contains("  "), "敏感值不应出现在警告行里");
        set_env_clean();
    }
}