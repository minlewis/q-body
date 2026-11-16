//! 会话边界上下文消毒守卫 —— 借鉴 yoyodev/yoyo-evolve #1206
//!
//! 场景：被拒轮（refused turn）或 thinking-only 消息若被原样回送下一请求，
//! 上一轮的拒绝/空转残余会毒化本轮上下文——LLM 看到「上一轮用户要求被拒绝」或
//! 大量无内容的思考块，可能复读拒绝或思维僵化。
//!
//! 本模块提供三类纯函数判定（无 I/O、无状态）：
//! - [`is_poisoned`]：判定一条 assistant 消息是否携带毒化残余（refusal 非空，
//!   或 content 为空且无 tool_calls 的空转轮）。
//! - [`strip_stale_residue`]：过滤消息列表中被判毒化 + 纯空转的 assistant 残余，
//!   保留 system 首条与 user 真实输入，返回清洗后的上下文。
//! - [`session_context_reset`]：session-end 完全重置钩子——保留 system 首条
//!   与最近一条 user 输入，清空其余所有累积残余，供 cron 在会话收尾调用。
//!
//! 判定语义对齐 yoyo #982 fail-closed：无法判定的消息保守视为「保留」，
//! 只有明确命中「毒化残余」形态才剥离，避免误伤合法对话历史。

use serde_json::Value;

/// 一条消息的消毒判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextVerdict {
    /// 保留：正常对话内容，不得剥离。
    Keep,
    /// 剥离：被拒轮 / thinking-only 空转残余，应移出下一请求上下文。
    Strip,
}

/// 判定一条 assistant 消息是否是被拒轮（refused turn）。
///
/// OpenAI-compatible 被拒形态：`refusal` 字段非空（顶层级或 `message` 包装级）。
/// 本谓词把「显式 refusal 非空」判为毒化。
pub fn is_refused(msg: &Value) -> bool {
    if let Some(r) = msg.get("refusal").and_then(|v| v.as_str()) {
        if !r.trim().is_empty() {
            return true;
        }
    }
    if let Some(m) = msg.get("message") {
        if let Some(r) = m.get("refusal").and_then(|v| v.as_str()) {
            if !r.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

/// 判定一条 assistant 消息是否是 thinking-only 空转轮：
/// content 为空（或缺失），且无 tool_calls —— 纯思考/纯拒绝但没说任何话、没动任何工具，
/// 对下一请求是纯噪声残余。
///
/// 仅对 `role == "assistant"` 的消息判定；其它 role（system/user/tool）一律保留。
pub fn is_thinking_only(msg: &Value) -> bool {
    let Some(role) = msg.get("role").and_then(|v| v.as_str()) else {
        return false;
    };
    if role != "assistant" {
        return false;
    }
    // 含 tool_calls 的工具调用轮不是空转，保留。
    if let Some(tc) = msg.get("tool_calls") {
        if tc.is_array() {
            if let Some(arr) = tc.as_array() {
                if !arr.is_empty() {
                    return false;
                }
            }
        }
    }
    // content 为空或缺失 → 空转。
    let content_empty = msg
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().is_empty())
        .unwrap_or(true);
    content_empty
}

/// 判定一条消息是否携带毒化残余：被拒轮 或 thinking-only 空转轮。
pub fn is_poisoned(msg: &Value) -> bool {
    is_refused(msg) || is_thinking_only(msg)
}

/// 过滤消息列表中的毒化残余（被拒轮 / thinking-only 空转），保留正常对话历史。
///
/// fail-closed 语义：只剥离明确命中毒化形态的 assistant 残余；
/// system / user / tool / 携带 tool_calls 的 assistant 轮全部保留。
/// 返回清洗后的消息列表（保持原顺序）。
pub fn strip_stale_residue(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| !is_poisoned(m))
        .cloned()
        .collect()
}

/// 会话边界状态重置钩子（session-end）：
/// 保留 system 首条（若有）与最近一条 user 真实输入，清空其余所有累积残余。
///
/// 语义：新会话只带「系统人格 + 用户本次真正想问的」，上一轮全部对话/工具/
/// 拒绝/思考残余均不跨会话传递。system 缺失时退化为保留最近一条 user（若有）。
/// 空输入 / 无 user 时返回空列表（fail-closed，不伪造上下文）。
pub fn session_context_reset(messages: &[Value]) -> Vec<Value> {
    let mut system: Option<Value> = None;
    let mut last_user: Option<Value> = None;

    for m in messages {
        let is_system = m.get("role").and_then(|v| v.as_str()) == Some("system");
        let is_user = m
            .get("role")
            .and_then(|v| v.as_str())
            .map(|r| r == "user")
            .unwrap_or(false);
        if is_system && system.is_none() {
            system = Some(m.clone());
        } else if is_user {
            // 记录最新一条 user（保留真实最新输入）
            last_user = Some(m.clone());
        }
    }

    let mut out = Vec::new();
    if let Some(s) = system {
        out.push(s);
    }
    if let Some(u) = last_user {
        out.push(u);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: &str) -> Value {
        json!({"role": role, "content": content})
    }

    #[test]
    fn kept_because_no_poison_shape() {
        let m = msg("assistant", "正常回复");
        assert_eq!(is_poisoned(&m), false);
        assert_eq!(is_thinking_only(&m), false);
        assert_eq!(is_refused(&m), false);
    }

    #[test]
    fn refused_is_poisoned() {
        let m = json!({"role": "assistant", "refusal": "I cannot comply", "content": null});
        assert!(is_refused(&m));
        assert!(is_poisoned(&m));
    }

    #[test]
    fn refused_wrapped_in_message() {
        let m = json!({
            "message": {"role": "assistant", "refusal": "blocked", "content": ""}
        });
        assert!(is_refused(&m));
        assert!(is_poisoned(&m));
    }

    #[test]
    fn thinking_only_empty_content_no_tools() {
        let m = json!({"role": "assistant", "content": "", "reasoning": "..."});
        assert!(is_thinking_only(&m));
        assert!(is_poisoned(&m));
    }

    #[test]
    fn tool_call_round_is_not_stripped() {
        let m = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{"id": "c1", "function": {"name": "ls", "arguments": "{}"}}]
        });
        assert!(!is_thinking_only(&m));
        assert!(!is_poisoned(&m));
    }

    #[test]
    fn non_assistant_roles_never_stripped() {
        assert!(!is_thinking_only(&msg("system", "")));
        assert!(!is_thinking_only(&msg("user", "")));
        assert!(!is_thinking_only(&msg("tool", "")));
    }

    #[test]
    fn strip_removes_only_poison() {
        let xs = vec![
            msg("system", "SYS"),
            msg("user", "你好"),
            json!({"role": "assistant", "refusal": "no"}),
            msg("assistant", "正常回复"),
            json!({"role": "assistant", "content": "", "reasoning": "thinking"}),
        ];
        let out = strip_stale_residue(&xs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], msg("system", "SYS"));
        assert_eq!(out[1], msg("user", "你好"));
        assert_eq!(out[2], msg("assistant", "正常回复"));
    }

    #[test]
    fn reset_keeps_system_and_last_user() {
        let xs = vec![
            msg("system", "SYS"),
            msg("user", "第一问"),
            msg("assistant", "答1"),
            msg("user", "第二问"),
            msg("assistant", "答2"),
            msg("tool", "res"),
        ];
        let out = session_context_reset(&xs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], msg("system", "SYS"));
        assert_eq!(out[1], msg("user", "第二问"));
    }

    #[test]
    fn reset_no_system_keeps_last_user_only() {
        let xs = vec![msg("user", "问"), msg("assistant", "答"), msg("user", "问2")];
        let out = session_context_reset(&xs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], msg("user", "问2"));
    }

    #[test]
    fn reset_empty_fails_closed() {
        let out = session_context_reset(&[]);
        assert!(out.is_empty());
    }
}