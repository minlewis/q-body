//! 模型 ID → 端点路由泛化（standalone 纯函数模块）
//!
//! 借鉴：yologdev/yoyo-evolve #1060 — openai wire residue after GPT-6 → Responses：
//! 把「模型 ID → 端点路由」从手写映射升级为泛化逻辑——按 model 前缀匹配 →
//! 动态决定走 chat/completions 还是 responses 端点，新模型无需改代码即可经
//! generic 路由接入。
//!
//! → q-body 对应改法：把 `LlmProvider` 的写死完整 `api_url` 改为
//! `base_url` + `route_for_model(model)` 泛化路由派生端点（deepseek-* 系 →
//! chat/completions；未来 responses 端点模型只需加前缀规则，调用方零改动）。
//!
//! 本模块为 standalone：handler.rs 生产接线按「借思路不借宿主」先例推迟，
//! 以单测形式维护路由表与 roundtrip。

/// 端点形态：模型族决议走哪类 API 协议
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// OpenAI 兼容 chat completions 端点（deepseek-* 系 / 缺省回落）
    ChatCompletions,
    /// OpenAI Responses 端点（未来新模型族，如 gpt-6 系）
    Responses,
}

impl EndpointKind {
    /// 端点路径后缀（不含 base_url）
    pub fn path(self) -> &'static str {
        match self {
            EndpointKind::ChatCompletions => "/chat/completions",
            EndpointKind::Responses => "/responses",
        }
    }
}

/// 模型前缀 → 端点路由表（单一事实源）
///
/// 路由语义：按 model 前缀匹配。未知/未列出的模型保守回落
/// ChatCompletions（fail-closed，参照 yoyo #982——未知形态走最保守路径，
/// 绝不让新模型误入未验证的 Responses 端点）。
const ROUTE_RULES: &[(&str, EndpointKind)] = &[
    // deepseek-* 系：文生/推理统一 chat completions
    ("deepseek-", EndpointKind::ChatCompletions),
    // 未来 responses 端点模型族在此加前缀规则即可，调用方零改动
];

/// 按模型 ID 决议端点形态。未知回落 ChatCompletions（保守）。
///
/// 前缀匹配按表序取首条命中；空模型名也回落缺省（不 panic）。
pub fn route_for_model(model: &str) -> EndpointKind {
    for (prefix, kind) in ROUTE_RULES {
        if model.starts_with(prefix) {
            return *kind;
        }
    }
    EndpointKind::ChatCompletions
}

/// 端点完整 URL = base_url（去尾部 `/`） + 形态路径
///
/// base_url 空串 → 仅返回路径（便于测试与退化）；不以 `/` 开头仍拼接。
pub fn endpoint_url(base_url: &str, kind: EndpointKind) -> String {
    let base = base_url.trim_end_matches('/');
    if base.is_empty() {
        kind.path().to_string()
    } else {
        format!("{}{}", base, kind.path())
    }
}

/// 从存量完整 api_url 反解 base_url（迁移助手）
///
/// 规则：匹配已知端点形态后缀，剥掉即得 base；未知结构原样返回（保守，
/// 不猜路径——迁移后由 route_for_model 重新派生，无匹配则回落 chat）。
pub fn split_base_url(api_url: &str) -> &str {
    for kind in [EndpointKind::ChatCompletions, EndpointKind::Responses] {
        let suffix = kind.path();
        if let Some(stripped) = api_url.strip_suffix(suffix) {
            return stripped;
        }
    }
    api_url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepseek_family_routes_to_chat() {
        assert_eq!(
            route_for_model("deepseek-v4-flash"),
            EndpointKind::ChatCompletions
        );
        assert_eq!(
            route_for_model("deepseek-chat"),
            EndpointKind::ChatCompletions
        );
        assert_eq!(
            route_for_model("deepseek-r1"),
            EndpointKind::ChatCompletions
        );
        // 前缀匹配：deepseek 家族任意 id 都命中，不改代码即可接入
        assert_eq!(
            route_for_model("deepseek-v4-ultra"),
            EndpointKind::ChatCompletions
        );
    }

    #[test]
    fn test_unknown_model_falls_back_to_chat_conservatively() {
        // 未知/空模型保守回落 chat completions（fail-closed，不误入 responses）
        assert_eq!(route_for_model("gpt-4o"), EndpointKind::ChatCompletions);
        assert_eq!(route_for_model(""), EndpointKind::ChatCompletions);
        assert_eq!(
            route_for_model("unknown-model-xyz"),
            EndpointKind::ChatCompletions
        );
    }

    #[test]
    fn test_response_kind_has_path() {
        assert_eq!(EndpointKind::ChatCompletions.path(), "/chat/completions");
        assert_eq!(EndpointKind::Responses.path(), "/responses");
    }

    #[test]
    fn test_endpoint_url_joins_base_and_path() {
        assert_eq!(
            endpoint_url(
                "https://ark.cn-beijing.volces.com/api/plan/v3",
                EndpointKind::ChatCompletions
            ),
            "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions"
        );
        // 尾部斜杠剥离，不双斜杠
        assert_eq!(
            endpoint_url("https://api.deepseek.com/", EndpointKind::ChatCompletions),
            "https://api.deepseek.com/chat/completions"
        );
        // 空 base → 仅路径
        assert_eq!(endpoint_url("", EndpointKind::Responses), "/responses");
    }

    #[test]
    fn test_split_base_url_roundtrip() {
        // 存量静态数组两条 api_url 反解回 base，再经 route_for_model 派生回原 URL——roundtrip 等价
        let ark_full = "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions";
        let deepseek_full = "https://api.deepseek.com/chat/completions";
        for full in [ark_full, deepseek_full] {
            let base = split_base_url(full);
            let rebuilt = endpoint_url(base, route_for_model("deepseek-v4-flash"));
            assert_eq!(rebuilt, full);
        }
    }

    #[test]
    fn test_split_base_url_unknown_structure_returns_unchanged() {
        // 未知结构保守原样返回，不猜路径
        assert_eq!(
            split_base_url("https://api.example.com/v1/weird"),
            "https://api.example.com/v1/weird"
        );
    }
}
