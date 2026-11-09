//! 配置层修复提示（借鉴 yologdev/yoyo-evolve #1163 auth-hint）。
//!
//! 目标：LLM 调用返回 400/401/403/429 时，把裸错误翻译成「去哪改、改哪项」的
//! 配置层修复提示，禁止裸错误穿透到用户（用户不知道 ARK_API_KEY 是什么、去哪改）。
//!
//! 设计：`auth_hint_for` 纯函数，输入 (http_status, env_key, provider_name) →
//! 返回修复提示。status → (tier, message) 分类表单一事实源；识别出的失败
//! 由 handler 在返回 last_err 前附上提示，详细原因只进 tracing 日志
//! （trust-boundary sanitize 同款纪律，SOUL §9）。

/// 修复提示层级：决定提示进入用户响应的强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintTier {
    /// 明确指向配置（key 无效/欠费/配额）—— 强烈建议修复
    Config,
    /// 临时性/上游错误 —— 温和提示
    Transient,
    /// 无法归类的错误 —— 不附加配置提示
    None,
}

/// 一条 auth-hint 分类表项。
pub struct AuthHintSpec {
    /// 匹配的单个 HTTP 状态码
    pub status: u16,
    /// 分类层级
    pub tier: HintTier,
    /// 面向用户的修复提示模板；`{env}` 与 `{provider}` 占位符会被替换
    pub hint: &'static str,
}

/// 唯一事实源：HTTP 状态 → 配置层修复提示分类表。
pub const AUTH_HINT_SPECS: &[AuthHintSpec] = &[
    AuthHintSpec {
        status: 400,
        tier: HintTier::Config,
        hint: "请求参数被拒绝（400）。检查 provider `{provider}` 的模型名/配置项是否与厂商端一致。",
    },
    AuthHintSpec {
        status: 401,
        tier: HintTier::Config,
        hint: "API key 无效或已失效（401）。请检查环境变量 `{env}` 是否为 provider `{provider}` 的有效 key（当前它被用作请求凭据）。",
    },
    AuthHintSpec {
        status: 403,
        tier: HintTier::Config,
        hint: "访问被拒绝（403）。该 API key（`{env}`，provider `{provider}`）可能无权访问此模型/资源，或已欠费。",
    },
    AuthHintSpec {
        status: 429,
        tier: HintTier::Transient,
        hint: "触发限流或配额上限（429，provider `{provider}`）。稍后重试，或提高 `{env}` 对应账户的配额。",
    },
];

/// 根据 HTTP 状态码返回配置层修复提示分类。
///
/// 找不到对应 spec 的 status 返回 `(HintTier::None, None)`——不附加配置提示，
/// 由调用方按原样处理（422/500 等无配置修复语义）。
pub fn classify_status(status: u16) -> (HintTier, Option<&'static str>) {
    for spec in AUTH_HINT_SPECS {
        if spec.status == status {
            return (spec.tier, Some(spec.hint));
        }
    }
    (HintTier::None, None)
}

/// 渲染含占位符的修复提示（`{env}` / `{provider}` 替换为实际值）。
///
/// 纯函数：不含任何 I/O / 日志，可直接在单测中断言字节级输出。
pub fn render_hint(template: &str, env_key: &str, provider: &str) -> String {
    template
        .replace("{env}", env_key)
        .replace("{provider}", provider)
}

/// 组合入口：一次给出 (tier, 渲染后的提示全文 或 None)。
pub fn auth_hint_for(status: u16, env_key: &str, provider: &str) -> (HintTier, Option<String>) {
    let (tier, hint) = classify_status(status);
    let rendered = hint.map(|h| render_hint(h, env_key, provider));
    (tier, rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_tier_statuses() {
        for status in [400u16, 401, 403] {
            let (tier, hint) = classify_status(status);
            assert_eq!(tier, HintTier::Config);
            assert!(hint.is_some(), "status {} should carry a hint", status);
        }
    }

    #[test]
    fn test_429_is_transient() {
        let (tier, hint) = classify_status(429);
        assert_eq!(tier, HintTier::Transient);
        assert!(hint.is_some());
    }

    #[test]
    fn test_unknown_status_no_hint() {
        for status in [422u16, 500, 502, 503, 200] {
            let (tier, hint) = classify_status(status);
            assert_eq!(tier, HintTier::None);
            assert!(hint.is_none(), "status {} should carry no hint", status);
        }
    }

    #[test]
    fn test_render_replaces_placeholders() {
        let out = render_hint("check {env} for provider {provider}", "ARK_API_KEY", "ark");
        assert_eq!(out, "check ARK_API_KEY for provider ark");
    }

    #[test]
    fn test_auth_hint_for_401_full_path() {
        let (tier, hint) = auth_hint_for(401, "DEEPSEEK_API_KEY", "deepseek-platform");
        assert_eq!(tier, HintTier::Config);
        let hint = hint.expect("401 must carry a hint");
        assert!(hint.contains("DEEPSEEK_API_KEY"));
        assert!(hint.contains("deepseek-platform"));
    }

    #[test]
    fn test_auth_hint_for_500_none() {
        let (tier, hint) = auth_hint_for(500, "ARK_API_KEY", "ark");
        assert_eq!(tier, HintTier::None);
        assert!(hint.is_none());
    }

    #[test]
    fn test_placeholder_not_in_input_is_preserved() {
        // 用户提供的字符串里若含占位符字样，只替换我们声明的 `{env}`/`{provider}`，
        // 不引入额外风险格式。
        let template = "env={env} provider={provider} literal={{x}}";
        let out = render_hint(template, "K", "p");
        assert_eq!(out, "env=K provider=p literal={{x}}");
    }
}
