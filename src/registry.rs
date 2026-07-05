//! HandlerRegistry — 动态方法注册与分发
//!
//! 借鉴 IBM/mcp-context-forge 的 plugin registry 架构：
//! 用 HashMap 做动态注册表，避免静态 match 硬编码路由。
//! 类型层准备，运行时接线（handler.rs 替换 match）按既定先例推迟。
//!
//! # 设计映射
//!
//! | mcp-context-forge plugin registry | q-body HandlerRegistry |
//! |---|---|
//! | `PluginRegistry.register(name, handler)` | `register(method, handler)` |
//! | Plugin handlers as typed callables | `HandlerFn = Arc<dyn Fn(...) -> BoxFuture<Value>>` |
//! | Dynamic dispatch via registry lookup | `dispatch(method, params, id)` → HashMap lookup |
//! | Unknown method → plugin not found | Unknown method → JsonRpcError::method_not_found |

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::a2a::types::*;

/// 异步 handler 函数签名：接收 params + request_id，返回 JSON-RPC 响应 Value
pub type HandlerFn = Arc<
    dyn Fn(Option<serde_json::Value>, serde_json::Value) -> Pin<Box<dyn Future<Output = serde_json::Value> + Send>>
        + Send
        + Sync,
>;

/// 动态方法注册表
///
/// 替代静态 match 分发，支持运行时注册新方法。
/// 线程安全（Arc<HashMap>），适用 axum 的共享状态模式。
pub struct HandlerRegistry {
    handlers: HashMap<String, HandlerFn>,
}

impl HandlerRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// 注册一个方法处理器
    ///
    /// `method` 支持多种风格（如 "SendMessage" 或 "message/send"），
    /// 调用方负责传已归一化的方法名。
    pub fn register(&mut self, method: impl Into<String>, handler: HandlerFn) {
        self.handlers.insert(method.into(), handler);
    }

    /// 分发方法调用
    ///
    /// 查表找到对应 handler 并异步执行。找不到返回 method_not_found 错误。
    pub async fn dispatch(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        match self.handlers.get(method) {
            Some(handler) => handler(params, request_id).await,
            None => {
                serde_json::to_value(JsonRpcError::method_not_found(request_id, method))
                    .unwrap_or_else(|_| {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32601,
                                "message": "Method not found"
                            },
                            "id": null,
                        })
                    })
            }
        }
    }

    /// 获取已注册方法列表
    pub fn methods(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.handlers.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 检查方法是否已注册
    pub fn has_method(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// 已注册方法数量
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// 是否为空注册表
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 辅助：创建返回固定值的 handler
    fn make_ok_handler(response: serde_json::Value) -> HandlerFn {
        Arc::new(move |_params, request_id| {
            let resp = response.clone();
            Box::pin(async move {
                serde_json::to_value(JsonRpcResponse::success(request_id, resp)).unwrap()
            })
        })
    }

    #[tokio::test]
    async fn test_register_and_dispatch() {
        let mut registry = HandlerRegistry::new();

        registry.register("ping", make_ok_handler(json!("pong")));

        let result = registry.dispatch("ping", None, json!("req-1")).await;
        assert_eq!(result["result"], json!("pong"));
        assert_eq!(result["id"], json!("req-1"));
    }

    #[tokio::test]
    async fn test_dispatch_unknown_method() {
        let registry = HandlerRegistry::new();

        let result = registry.dispatch("SendMessage", None, json!("req-2")).await;
        assert_eq!(result["error"]["code"], -32601);
        assert_eq!(result["id"], json!("req-2"));
    }

    #[tokio::test]
    async fn test_dispatch_with_params() {
        let mut registry = HandlerRegistry::new();

        registry.register(
            "echo",
            Arc::new(|params, request_id| {
                Box::pin(async move {
                    let response = json!({ "echo": params });
                    serde_json::to_value(JsonRpcResponse::success(request_id, response)).unwrap()
                })
            }),
        );

        let params = json!({ "text": "hello" });
        let result = registry
            .dispatch("echo", Some(params.clone()), json!("req-3"))
            .await;
        assert_eq!(result["result"]["echo"], params);
        assert_eq!(result["id"], json!("req-3"));
    }

    #[tokio::test]
    async fn test_has_method_and_methods_list() {
        let mut registry = HandlerRegistry::new();
        registry.register("SendMessage", make_ok_handler(json!("ok")));
        registry.register("GetTask", make_ok_handler(json!("ok")));

        assert!(registry.has_method("SendMessage"));
        assert!(registry.has_method("GetTask"));
        assert!(!registry.has_method("ListTasks"));
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.methods(), vec!["GetTask", "SendMessage"]);
    }

    #[tokio::test]
    async fn test_empty_registry() {
        let registry = HandlerRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}