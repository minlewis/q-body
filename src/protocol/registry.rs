//! Protocol Registry — 协议注册表
//!
//! 借鉴：IBM/mcp-context-forge — 统一网关将 MCP/A2A/REST/gRPC
//! 聚合为统一 endpoint，带集中发现和 guardrails。
//!
//! ProtocolRegistry 支持注册/分发/列举协议，为后续 MCP handler 接线
//! 奠定类型基础。

use std::collections::HashMap;

/// 支持的协议类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProtocolKind {
    /// A2A (Agent-to-Agent) JSON-RPC 协议
    A2A,
    /// MCP (Model Context Protocol) 协议
    MCP,
}

impl ProtocolKind {
    /// 从字符串解析协议类型
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "a2a" | "json-rpc" | "jsonrpc" => Some(ProtocolKind::A2A),
            "mcp" => Some(ProtocolKind::MCP),
            _ => None,
        }
    }

    /// 返回协议的字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtocolKind::A2A => "a2a",
            ProtocolKind::MCP => "mcp",
        }
    }
}

/// 协议处理函数签名
pub type ProtocolHandler = fn(serde_json::Value) -> Result<serde_json::Value, String>;

/// 协议注册表
///
/// 管理协议注册和分发，支持运行时动态注册。
#[derive(Debug, Clone)]
pub struct ProtocolRegistry {
    /// 协议名 → 处理函数映射
    protocols: HashMap<String, ProtocolHandler>,
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolRegistry {
    /// 创建一个空的协议注册表
    pub fn new() -> Self {
        Self {
            protocols: HashMap::new(),
        }
    }

    /// 注册一个协议
    ///
    /// 返回旧的 handler（如果已存在同名协议）。
    pub fn register(
        &mut self,
        kind: ProtocolKind,
        handler: ProtocolHandler,
    ) -> Option<ProtocolHandler> {
        self.protocols.insert(kind.as_str().to_string(), handler)
    }

    /// 分发消息到指定协议
    ///
    /// 协议不存在时返回 Err。
    pub fn dispatch(
        &self,
        kind: &ProtocolKind,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match self.protocols.get(kind.as_str()) {
            Some(handler) => handler(payload),
            None => Err(format!("Protocol not supported: {}", kind.as_str())),
        }
    }

    /// 检查协议是否已注册
    pub fn has_protocol(&self, kind: &ProtocolKind) -> bool {
        self.protocols.contains_key(kind.as_str())
    }

    /// 列出所有已注册的协议
    pub fn list_protocols(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.protocols.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 已注册的协议数量
    pub fn len(&self) -> usize {
        self.protocols.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.protocols.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_kind_from_str() {
        assert_eq!(ProtocolKind::from_str("a2a"), Some(ProtocolKind::A2A));
        assert_eq!(ProtocolKind::from_str("A2A"), Some(ProtocolKind::A2A));
        assert_eq!(ProtocolKind::from_str("json-rpc"), Some(ProtocolKind::A2A));
        assert_eq!(ProtocolKind::from_str("jsonrpc"), Some(ProtocolKind::A2A));
        assert_eq!(ProtocolKind::from_str("mcp"), Some(ProtocolKind::MCP));
        assert_eq!(ProtocolKind::from_str("MCP"), Some(ProtocolKind::MCP));
        assert_eq!(ProtocolKind::from_str("unknown"), None);
    }

    #[test]
    fn test_empty_registry() {
        let reg = ProtocolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(!reg.has_protocol(&ProtocolKind::A2A));
        assert!(!reg.has_protocol(&ProtocolKind::MCP));
    }

    #[test]
    fn test_register_and_has_protocol() {
        let mut reg = ProtocolRegistry::new();
        let handler: ProtocolHandler = |payload| Ok(payload);

        reg.register(ProtocolKind::A2A, handler);
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
        assert!(reg.has_protocol(&ProtocolKind::A2A));
        assert!(!reg.has_protocol(&ProtocolKind::MCP));
    }

    #[test]
    fn test_register_multiple_protocols() {
        let mut reg = ProtocolRegistry::new();
        let handler_a2a: ProtocolHandler = |payload| Ok(payload);
        let handler_mcp: ProtocolHandler = |payload| Ok(payload);

        reg.register(ProtocolKind::A2A, handler_a2a);
        reg.register(ProtocolKind::MCP, handler_mcp);
        assert_eq!(reg.len(), 2);
        assert!(reg.has_protocol(&ProtocolKind::A2A));
        assert!(reg.has_protocol(&ProtocolKind::MCP));
    }

    #[test]
    fn test_dispatch_known_protocol() {
        let mut reg = ProtocolRegistry::new();
        let handler: ProtocolHandler = |payload| {
            let result = serde_json::json!({
                "processed": true,
                "payload": payload,
            });
            Ok(result)
        };

        reg.register(ProtocolKind::A2A, handler);
        let payload = serde_json::json!({"method": "test"});
        let result = reg.dispatch(&ProtocolKind::A2A, payload.clone()).unwrap();
        assert_eq!(result["processed"], true);
        assert_eq!(result["payload"], payload);
    }

    #[test]
    fn test_dispatch_unknown_protocol() {
        let reg = ProtocolRegistry::new();
        let result = reg.dispatch(&ProtocolKind::MCP, serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Protocol not supported: mcp"));
    }

    #[test]
    fn test_register_overwrites_existing() {
        let mut reg = ProtocolRegistry::new();
        let handler_old: ProtocolHandler = |_| Ok(serde_json::json!({"version": "old"}));
        let handler_new: ProtocolHandler = |_| Ok(serde_json::json!({"version": "new"}));

        reg.register(ProtocolKind::A2A, handler_old);
        let old = reg.register(ProtocolKind::A2A, handler_new);
        assert!(old.is_some()); // 返回旧 handler
        assert_eq!(reg.len(), 1); // 不增加数量

        let result = reg.dispatch(&ProtocolKind::A2A, serde_json::json!({})).unwrap();
        assert_eq!(result["version"], "new");
    }

    #[test]
    fn test_list_protocols() {
        let mut reg = ProtocolRegistry::new();
        let handler: ProtocolHandler = |payload| Ok(payload);

        reg.register(ProtocolKind::MCP, handler);
        reg.register(ProtocolKind::A2A, handler);

        let protocols = reg.list_protocols();
        assert_eq!(protocols, vec!["a2a", "mcp"]);
    }
}