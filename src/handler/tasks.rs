//! GetTask / ListTasks 查询处理

use crate::a2a::types::*;
use crate::handler::QBodyHandler;

impl QBodyHandler {
    /// 处理 GetTask：查询指定 Task 的状态和结果
    pub async fn handle_get_task(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let req: GetTaskRequest = match params
            .and_then(|p| serde_json::from_value(p).ok())
        {
            Some(r) => r,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    "missing or invalid GetTask params",
                ))
                .unwrap();
            }
        };

        match self.task_store.get_task(&req.id).await {
            Some(task) => {
                let resp = GetTaskResponse::from_task(&task);
                serde_json::to_value(JsonRpcResponse::success(request_id, resp)).unwrap()
            }
            None => serde_json::to_value(JsonRpcError::invalid_params(
                request_id,
                &format!("task not found: {}", req.id),
            ))
            .unwrap(),
        }
    }

    /// 处理 ListTasks（简化版，只返回 ID 列表）
    pub async fn handle_list_tasks(
        &self,
        _params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let result = serde_json::json!({
            "tasks": [],
            "nextPageToken": null,
        });
        serde_json::to_value(JsonRpcResponse::success(request_id, result)).unwrap()
    }
}