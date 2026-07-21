//! A2A 协议核心类型定义
//!
//! 参考 A2A Protocol v1.0 specification：
//! - AgentCard: agent 发现用的"名片"
//! - Task: 任务生命周期
//! - Message / Part: 消息与内容片

use serde::{Deserialize, Serialize};

// A2A 线协议（wire protocol）使用小写枚举值（如 "submitted"/"user"），
// Rust 命名风格与协议兼容性冲突时，以协议为准（见下方 enum 上的 allow）。

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
#[allow(non_camel_case_types)]
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
#[allow(non_camel_case_types)]
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
// ============================================================
// Tests — JSON-RPC 序列化契约
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// camelCase 重命名契约：contextId / messageId / mediaType / lastChunk
    #[test]
    fn task_serializes_camel_case_fields() {
        let task = Task {
            id: "t-1".into(),
            context_id: Some("ctx-1".into()),
            status: TaskStatus {
                state: TaskState::submitted,
                message: None,
            },
            history: None,
            artifacts: None,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["contextId"], "ctx-1");
        assert!(v.get("context_id").is_none());
        // Option::is_none 字段必须被跳过
        assert!(v.get("history").is_none());
        assert!(v.get("artifacts").is_none());
    }

    #[test]
    fn message_serializes_message_id_camel_case() {
        let msg = Message {
            role: "user".into(),
            parts: vec![Part::text("hi")],
            message_id: Some("m-1".into()),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["messageId"], "m-1");
        assert!(v.get("message_id").is_none());
    }

    /// JSON-RPC 成功响应的形状契约
    #[test]
    fn jsonrpc_success_response_shape() {
        let resp = JsonRpcResponse::success(json!(1), json!({"ok": true}));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["ok"], true);
        assert!(v.get("error").is_none());
    }

    /// JSON-RPC 错误码契约：-32601 method not found / -32602 invalid params / -32603 internal
    #[test]
    fn jsonrpc_error_codes_match_spec() {
        let e = JsonRpcError::method_not_found(json!("req-1"), "tasks/send");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["error"]["code"], -32601);
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("tasks/send")
        );

        let e2 = JsonRpcError::invalid_params(json!(2), "bad");
        assert_eq!(
            serde_json::to_value(&e2).unwrap()["error"]["code"],
            json!(-32602)
        );

        let e3 = JsonRpcError::internal(json!(3), "boom");
        assert_eq!(
            serde_json::to_value(&e3).unwrap()["error"]["code"],
            json!(-32603)
        );
    }

    /// TaskState 未知状态反序列化为 Unknown（前向兼容）
    #[test]
    fn task_state_unknown_variant_roundtrip() {
        let s: TaskState = serde_json::from_value(json!("input-required")).unwrap();
        assert_eq!(s, TaskState::Unknown("input-required".into()));
        let c: TaskState = serde_json::from_value(json!("completed")).unwrap();
        assert_eq!(c, TaskState::completed);
    }

    /// SendMessageResponse::from_task 取 history 最后一条作为 message
    #[test]
    fn send_message_response_from_task_picks_last_history() {
        let task = Task {
            id: "t-9".into(),
            context_id: None,
            status: TaskStatus {
                state: TaskState::completed,
                message: None,
            },
            history: Some(vec![
                Message {
                    role: "user".into(),
                    parts: vec![Part::text("q")],
                    message_id: None,
                },
                Message {
                    role: "assistant".into(),
                    parts: vec![Part::text("a")],
                    message_id: None,
                },
            ]),
            artifacts: None,
        };
        let resp = SendMessageResponse::from_task(&task);
        assert_eq!(resp.id, "t-9");
        assert_eq!(resp.message.unwrap().role, "assistant");
    }
}
