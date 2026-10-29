//! Per-surface 命令命名辅助 — q-body 各入口（A2A JSON-RPC / WebUI / 微信）
//! 的命令命名收拢为单一事实源，各 surface 只注册自己形态的命令。
//!
//! 背景：A2A JSON-RPC 方法名（handler.rs 分发表）、WebUI 前端 POST 的
//! `message/send`、微信触发词三处命令命名历史上各自硬编码、互不引用。
//! 本模块提供 per-surface 派发 helper，让每个 surface 从同一命名表取命令，
//! 消灭「同一动作在不同入口命名漂移」。
//!
//! 借鉴：yologdev/yoyo-evolve #1086 — surface-aware usage helper（每个 surface
//! 只注册自己的形态，命名单一事实源）→ q-body 对应：Surface 枚举 + for_method
//! 归属 + command() 命名单一事实源。

use crate::a2a::types::TransportScope;

/// q-body 对外暴露命令的入口面（surface）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A2A JSON-RPC 端点（POST /a2a/jsonrpc）
    A2a,
    /// WebUI 单页聊天（浏览器 /）
    WebUi,
    /// 微信桥（外部 wechat bridge 转 A2A）
    WeChat,
}

impl Surface {
    /// 方法名（含别名归一化，与 TransportScope::for_method 同一原始命名表）到
    /// surface 的归属。单一事实源：handler.rs 分发表、webui.rs 前端、微信桥
    /// 的命令名均派生自本表。
    pub fn for_method(method: &str) -> Option<Self> {
        match method.trim() {
            // A2A 对外开放方法（与 TransportScope::A2a 对齐）
            "SendMessage" | "message/send" => Some(Self::A2a),
            "GetTask" | "tasks/get" | "ListTasks" | "tasks/list" => Some(Self::A2a),
            // internal 方法也经 A2A 通道分发（scope 门在 handler 前置拒绝）
            "Reflect" | "reflection/score" | "JournalRecord" | "journal/record" => {
                Some(Self::A2a)
            }
            _ => None,
        }
    }

    /// 该 surface 实际用于发起对话的命令名（单一事实源，供各入口引用，
    /// 避免 WebUI 硬编码 "message/send"、微信桥另写一套）。
    pub fn command(self) -> &'static str {
        match self {
            Surface::A2a | Surface::WeChat => "message/send",
            // WebUI 走同一 A2A 端点，前端 POST 方法名与 A2A 一致
            Surface::WebUi => "message/send",
        }
    }

    /// 校验：A2A 对外开放方法必须与 TransportScope::A2a 归属完全一致；
    /// 漂移即编译期测试红（防本表与 scope 表分叉）。
    pub fn assert_scope_consistency(method: &str) -> bool {
        let s = Self::for_method(method);
        match TransportScope::for_method(method) {
            Some(TransportScope::A2a) => s == Some(Self::A2a),
            Some(TransportScope::Internal) => s == Some(Self::A2a),
            None => s.is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_method_belongs_to_a2a_surface() {
        for m in [
            "SendMessage",
            "message/send",
            "GetTask",
            "tasks/get",
            "ListTasks",
            "tasks/list",
            "Reflect",
            "reflection/score",
            "JournalRecord",
            "journal/record",
        ] {
            assert_eq!(Surface::for_method(m), Some(Surface::A2a), "{m}");
        }
    }

    #[test]
    fn test_all_surfaces_share_same_command_source() {
        // 三入口的对话命令是同一命名源（消灭 WebUI/微信命名漂移）
        assert_eq!(Surface::A2a.command(), "message/send");
        assert_eq!(Surface::WebUi.command(), "message/send");
        assert_eq!(Surface::WeChat.command(), "message/send");
    }

    #[test]
    fn test_scope_consistency_with_transport_scope() {
        // surface 归属表与 TransportScope 表不漂移（全分发表逐条对齐）
        for m in [
            "SendMessage",
            "message/send",
            "GetTask",
            "tasks/get",
            "ListTasks",
            "tasks/list",
            "Reflect",
            "reflection/score",
            "JournalRecord",
            "journal/record",
            "unknown/method",
            "",
        ] {
            // 对 A2a 与 Internal 两种 scope，surface 均归 A2a；未知方法两表都 None
            assert!(Surface::assert_scope_consistency(m), "{m}");
        }
    }

    #[test]
    fn test_unknown_method_belongs_to_none() {
        assert_eq!(Surface::for_method("no/such/method"), None);
        assert_eq!(Surface::for_method(""), None);
    }
}