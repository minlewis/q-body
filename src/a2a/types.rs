//! A2A 协议核心类型定义
//!
//! 参考 A2A Protocol v1.0 specification：
//! - AgentCard: agent 发现用的"名片"
//! - Task: 任务生命周期
//! - Message / Part: 消息与内容片

use serde::{Deserialize, Serialize};

// ============================================================
// Agent Card — agent 对外暴露的元信息
// ============================================================

/// Agent Card：描述一个 A2A agent 的能力
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: Option<String>,
    pub provider: Option<AgentProvider>,
    pub version: String,
    pub capabilities: Option<AgentCapabilities>,
    #[serde(rename = "defaultInputModes")]
    pub default_input_modes: Vec<String>,
    #[serde(rename = "defaultOutputModes")]
    pub default_output_modes: Vec<String>,
    pub skills: Vec<AgentSkill>,
    #[serde(rename = "supportedInterfaces")]
    pub supported_interfaces: Vec<AgentInterface>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvider {
    pub organization: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    #[serde(rename = "pushNotifications")]
    pub push_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub examples: Vec<String>,
    pub input_modes: Vec<String>,
    pub output_modes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInterface {
    #[serde(rename = "protocolBinding")]
    pub protocol_binding: String,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub url: String,
}

// ============================================================
// Transport — 传输协议枚举
// ============================================================

/// A2A 传输协议
///
/// 借鉴：tomtom215/a2a-rust — 四传输层设计（JSON-RPC/REST/WebSocket/gRPC），
/// q-body 当前仅实现 JSON-RPC over HTTP；WebSocket 为计划中的传输扩展，
/// 用于支持长连接双向通信和流式 LLM 响应推送。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Transport {
    /// JSON-RPC over HTTP（当前唯一支持的传输）
    Jsonrpc,
    /// WebSocket 传输（计划中 — 支持 streaming 响应和实时状态推送）
    Ws,
}

impl Transport {
    /// 返回 `AgentInterface.protocol_binding` 字段使用的规范字符串
    pub fn as_binding(&self) -> &'static str {
        match self {
            Transport::Jsonrpc => "JSONRPC",
            Transport::Ws => "WS",
        }
    }
}

impl AgentInterface {
    /// 判断此接口是否使用 WebSocket 传输
    pub fn is_websocket(&self) -> bool {
        self.protocol_binding.eq_ignore_ascii_case("WS")
            || self.protocol_binding.eq_ignore_ascii_case("WEBSOCKET")
    }
}

// ============================================================
// Message — agent 之间交换的消息
// ============================================================

/// 消息中的角色
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    user,
    assistant,
}

/// Part：消息内容片——可以是 text、url、data 等
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Part {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            media_type: None,
            url: None,
        }
    }
}

/// A2A 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "role")]
    pub role: String,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "messageId")]
    pub message_id: Option<String>,
}

// ============================================================
// Task — 任务生命周期
// ============================================================

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskState {
    submitted,
    working,
    completed,
    failed,
    canceled,
    #[serde(untagged)]
    Unknown(String),
}

/// 任务状态包装
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "message")]
    pub message: Option<Message>,
}

/// 一个完整的 Task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "contextId")]
    pub context_id: Option<String>,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Message>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
}

/// 任务产出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub parts: Vec<Part>,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastChunk")]
    pub last_chunk: Option<bool>,
}

// ============================================================
// SendMessage 请求/响应
// ============================================================

/// 发送消息请求（对应 A2A 的 SendMessage）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// 发送消息响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub id: String,
    pub message: Option<Message>,
    pub status: TaskStatus,
    pub artifacts: Option<Vec<Artifact>>,
}

impl SendMessageResponse {
    pub fn from_task(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            message: task.history.as_ref().and_then(|h| h.last().cloned()),
            status: task.status.clone(),
            artifacts: task.artifacts.clone(),
        }
    }
}

// ============================================================
// GetTask 请求/响应
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskResponse {
    pub id: String,
    pub status: TaskStatus,
    pub history: Option<Vec<Message>>,
    pub artifacts: Option<Vec<Artifact>>,
}

impl GetTaskResponse {
    pub fn from_task(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            status: task.status.clone(),
            history: task.history.clone(),
            artifacts: task.artifacts.clone(),
        }
    }
}

// ============================================================
// JSON-RPC 协议层
// ============================================================

/// JSON-RPC 请求
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 成功响应
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse<T: Serialize> {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub result: T,
}

impl<T: Serialize> JsonRpcResponse<T> {
    pub fn success(id: serde_json::Value, result: T) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result,
        }
    }
}

/// JSON-RPC 错误响应
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub error: JsonRpcErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcErrorDetail {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    pub fn method_not_found(id: serde_json::Value, method: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: JsonRpcErrorDetail {
                code: -32601,
                message: format!("Method not found: {method}"),
            },
        }
    }

    pub fn invalid_params(id: serde_json::Value, msg: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: JsonRpcErrorDetail {
                code: -32602,
                message: format!("Invalid params: {msg}"),
            },
        }
    }

    pub fn internal(id: serde_json::Value, msg: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: JsonRpcErrorDetail {
                code: -32603,
                message: format!("Internal error: {msg}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_binding_strings() {
        assert_eq!(Transport::Jsonrpc.as_binding(), "JSONRPC");
        assert_eq!(Transport::Ws.as_binding(), "WS");
    }

    #[test]
    fn test_agent_interface_is_websocket() {
        let ws_iface = AgentInterface {
            protocol_binding: "WS".into(),
            protocol_version: "1.0".into(),
            url: "ws://127.0.0.1:41242/a2a/ws".into(),
        };
        assert!(ws_iface.is_websocket());

        let http_iface = AgentInterface {
            protocol_binding: "JSONRPC".into(),
            protocol_version: "1.0".into(),
            url: "http://127.0.0.1:41242/a2a/jsonrpc".into(),
        };
        assert!(!http_iface.is_websocket());
    }

    #[test]
    fn test_transport_serde_roundtrip() {
        let json = serde_json::to_string(&Transport::Ws).unwrap();
        assert_eq!(json, "\"WS\"");
        let back: Transport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Transport::Ws);
    }
}