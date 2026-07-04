//! SendMessage 处理 — 接收消息、创建 Task、调 LLM、返回结果

use uuid::Uuid;

use crate::a2a::types::*;
use crate::handler::QBodyHandler;

impl QBodyHandler {
    /// 处理 SendMessage：接收消息 → 创建 Task → 调 LLM → 返回结果
    pub async fn handle_send_message(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        // 解析参数
        let req: SendMessageRequest = match params
            .and_then(|p| serde_json::from_value(p).ok())
        {
            Some(r) => r,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    "missing or invalid SendMessage params",
                ))
                .unwrap();
            }
        };

        let task_id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let context_id = format!("ctx-{}", &task_id[..8]);

        // 提取用户文本
        let user_text = req
            .message
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ");

        // 创建 Task
        self.task_store
            .create_task(task_id.clone(), context_id, req.message)
            .await;

        // 标记为 working
        self.task_store.update_status(&task_id, TaskState::working).await;

        // === 核心：调 LLM ===
        let reply = self.query_llm(&user_text).await;

        // agent 回复
        let agent_msg = Message {
            role: "assistant".into(),
            parts: vec![Part::text(&reply)],
            message_id: Some(Uuid::new_v4().to_string()),
        };

        let artifact = Artifact {
            parts: vec![Part::text(&reply)],
            name: Some("response".into()),
            last_chunk: Some(true),
        };

        self.task_store
            .add_reply(&task_id, agent_msg, vec![artifact])
            .await;

        // 标记为 completed
        self.task_store.update_status(&task_id, TaskState::completed).await;

        // 获取完整 Task 并返回
        match self.task_store.get_task(&task_id).await {
            Some(task) => {
                let resp = SendMessageResponse::from_task(&task);
                serde_json::to_value(JsonRpcResponse::success(request_id, resp)).unwrap()
            }
            None => serde_json::to_value(JsonRpcError::internal(
                request_id,
                "task not found after creation",
            ))
            .unwrap(),
        }
    }
}