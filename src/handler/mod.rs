//! A2A Handler — q-body 的 A2A 请求处理核心
//!
//! 按职责拆分子模块：
//! - message.rs: SendMessage 消息处理 + LLM 调用
//! - tasks.rs: GetTask / ListTasks 查询
//! - llm.rs: LLM API 调用

pub mod llm;
pub mod message;
pub mod tasks;

use crate::a2a::types::*;
use crate::state::TaskStore;

/// q-body A2A 处理器
pub struct QBodyHandler {
    pub task_store: TaskStore,
    pub agent_card: AgentCard,
    /// HTTP 客户端（复用连接，避免每次新建）
    pub(crate) http_client: reqwest::Client,
}

impl QBodyHandler {
    pub fn new(task_store: TaskStore, agent_card: AgentCard) -> Self {
        Self {
            task_store,
            agent_card,
            http_client: reqwest::Client::new(),
        }
    }

    /// 处理 JSON-RPC 请求分发
    pub async fn handle_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        match method {
            "SendMessage" | "message/send" => {
                self.handle_send_message(params, request_id).await
            }
            "GetTask" | "tasks/get" => {
                self.handle_get_task(params, request_id).await
            }
            "ListTasks" | "tasks/list" => {
                self.handle_list_tasks(params, request_id).await
            }
            _ => serde_json::to_value(JsonRpcError::method_not_found(request_id, method))
                .unwrap(),
        }
    }
}