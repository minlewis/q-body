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