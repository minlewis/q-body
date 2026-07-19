//! MCP 注册表 — 参考 modelcontextprotocol/specification 的协议注册/发现模式
//!
//! MCP 的核心模式是 server 通过 capabilities 声明能力（tools/resources/prompts），
//! client 通过 initialize 握手发现可用能力。
//!
//! 借鉴：modelcontextprotocol/specification — 标准协议注册/发现模式
//!
//! 类型层准备，handler.rs 运行时接线按既定先例推迟。

use std::collections::HashMap;

use crate::a2a::types::{McpCapability, McpServerCapabilities};

/// MCP 注册表：管理已注册能力，支持注册/发现/查询
#[derive(Debug, Clone)]
pub struct McpRegistry {
    /// 方法名 → 能力映射
    methods: HashMap<String, McpCapability>,
    /// Server 能力声明（用于 initialize 握手）
    capabilities: McpServerCapabilities,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            methods: HashMap::new(),
            capabilities: McpServerCapabilities::new(),
        }
    }

    /// 注册一个方法到指定能力
    pub fn register(&mut self, method: &str, capability: McpCapability) {
        self.methods.insert(method.to_string(), capability.clone());
        self.capabilities.register(capability);
    }

    /// 发现指定方法对应的能力
    pub fn discover(&self, method: &str) -> Option<&McpCapability> {
        self.methods.get(method)
    }

    /// 检查是否支持指定能力
    pub fn supports(&self, capability: &McpCapability) -> bool {
        self.capabilities.supports(capability)
    }

    /// 获取所有已注册的方法名
    pub fn methods(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.methods.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }

    /// 获取 Server 能力声明（用于 initialize 响应）
    pub fn capabilities(&self) -> &McpServerCapabilities {
        &self.capabilities
    }

    /// 注册数量
    pub fn len(&self) -> usize {
        self.methods.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::McpCapability;

    #[test]
    fn test_new_registry_is_empty() {
        let reg = McpRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_register_and_discover() {
        let mut reg = McpRegistry::new();
        reg.register("tools/call", McpCapability::Tool);
        assert_eq!(reg.discover("tools/call"), Some(&McpCapability::Tool));
        assert!(reg.supports(&McpCapability::Tool));
        assert!(!reg.supports(&McpCapability::Resource));
    }

    #[test]
    fn test_register_unknown_returns_none() {
        let reg = McpRegistry::new();
        assert_eq!(reg.discover("nonexistent"), None);
    }

    #[test]
    fn test_register_capability_updates_capabilities() {
        let mut reg = McpRegistry::new();
        reg.register("tools/call", McpCapability::Tool);
        reg.register("resources/list", McpCapability::Resource);
        let caps = reg.capabilities();
        assert!(caps.supports(&McpCapability::Tool));
        assert!(caps.supports(&McpCapability::Resource));
        assert!(!caps.supports(&McpCapability::Prompt));
    }

    #[test]
    fn test_register_duplicate_does_not_duplicate() {
        let mut reg = McpRegistry::new();
        reg.register("tools/call", McpCapability::Tool);
        reg.register("tools/call", McpCapability::Tool);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_methods_sorted() {
        let mut reg = McpRegistry::new();
        reg.register("zzz", McpCapability::Prompt);
        reg.register("aaa", McpCapability::Tool);
        let methods = reg.methods();
        assert_eq!(methods, vec!["aaa", "zzz"]);
    }
}