//! A2A Handler — q-body 的 A2A 请求处理核心
//!
//! 实现了 JSON-RPC method 分发：
//! - SendMessage：创建 Task，通过 LLM 处理消息，返回结果
//! - GetTask：查询 Task 状态
//! - ListTasks: 列出所有 Task

use uuid::Uuid;

use crate::a2a::types::*;
use crate::state::TaskStore;

/// 火山引擎 deepseek-v4-flash 的 API 端点
const LLM_API_URL: &str = "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions";
const LLM_MODEL: &str = "deepseek-v4-flash";

/// q-body A2A 处理器
pub struct QBodyHandler {
    pub task_store: TaskStore,
    pub agent_card: AgentCard,
    /// HTTP 客户端（复用连接，避免每次新建）
    http_client: reqwest::Client,
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

    /// 处理 SendMessage：接收消息 → 创建 Task → 调 LLM → 返回结果
    async fn handle_send_message(
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

    /// 调 deepseek-v4-flash（火山引擎）
    async fn query_llm(&self, user_text: &str) -> String {
        let api_key = match std::env::var("ARK_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                tracing::warn!("ARK_API_KEY not set, falling back to static reply");
                return format!(
                    "q-body received: '{}'. (LLM not configured — set ARK_API_KEY to enable AI responses)",
                    user_text
                );
            }
        };

        let request_body = serde_json::json!({
            "model": LLM_MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": "你是 q-body，Q宝宝的自进化 Rust 身体。\n\
                                你通过 A2A 协议接收外部消息。\n\
                                请保持简洁、务实、带一点 🫧 风格的回复。\n\
                                你正在进化中，回答体现你的版本号 0.1.1。"
                },
                {
                    "role": "user",
                    "content": user_text
                }
            ],
            "stream": false
        });

        let response = self
            .http_client
            .post(LLM_API_URL)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request_body)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        if status.is_success() {
                            // 从 OpenAI 格式的响应中提取文本
                            body["choices"][0]["message"]["content"]
                                .as_str()
                                .unwrap_or("(empty response from LLM)")
                                .to_string()
                        } else {
                            let err_msg = body["error"]["message"]
                                .as_str()
                                .unwrap_or("unknown error");
                            tracing::error!("LLM API error ({}): {}", status, err_msg);
                            format!("Sorry, LLM returned error {}: {}", status, err_msg)
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse LLM response: {}", e);
                        format!("Sorry, failed to parse LLM response: {}", e)
                    }
                }
            }
            Err(e) => {
                tracing::error!("HTTP request to LLM failed: {}", e);
                format!("Sorry, LLM request failed: {}", e)
            }
        }
    }

    /// 处理 GetTask：查询指定 Task 的状态和结果
    async fn handle_get_task(
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
    async fn handle_list_tasks(
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

// ============================================================
// Prompt 分解 — 检测复合任务，拆分子任务
// ============================================================

/// 子任务类型
#[derive(Debug, Clone, PartialEq)]
pub enum SubTaskKind {
    /// 独立可并行执行的任务
    Independent,
    /// 依赖前置结果的任务
    Sequential,
}

/// 一个分解后的子任务
#[derive(Debug, Clone)]
pub struct SubTask {
    pub id: usize,
    pub description: String,
    pub kind: SubTaskKind,
}

/// 检测 prompt 是否包含复合任务标记，并分解为子任务
///
/// 借鉴：yologdev/yoyo-evolve — Day 130 `Wire detect_parallelizable_tasks into /spawn`
/// yoyo 在检测到复合任务关键词后自动拆分子任务，以 JSON-RPC batch 并行发送。
/// q-body 对应：启发式关键词检测 + 分句拆分，类型层准备。
pub fn decompose_task(prompt: &str) -> Vec<SubTask> {
    let prompt_lower = prompt.to_lowercase();

    // 检测多任务关键词
    let multi_task_keywords = [
        "分步", "步骤", "同时", "并行", "分别",
        "parallel", "step by step", "meanwhile", "concurrently",
    ];
    let has_multi_task = multi_task_keywords
        .iter()
        .any(|kw| prompt_lower.contains(kw));

    if !has_multi_task {
        // 单任务：直接返回
        return vec![SubTask {
            id: 1,
            description: prompt.to_string(),
            kind: SubTaskKind::Independent,
        }];
    }

    // 按换行、句号、分号、数字编号拆分子句
    let separators = ["\n", "\r\n", "。", ";", "；"];
    let mut raw = prompt.to_string();
    for sep in &separators {
        raw = raw.replace(sep, "\x00");
    }

    let sentences: Vec<&str> = raw
        .split('\x00')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if sentences.len() <= 1 {
        return vec![SubTask {
            id: 1,
            description: prompt.to_string(),
            kind: SubTaskKind::Independent,
        }];
    }

    sentences
        .into_iter()
        .enumerate()
        .map(|(i, s)| SubTask {
            id: i + 1,
            description: s.to_string(),
            kind: SubTaskKind::Independent,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_task_no_keyword() {
        let result = decompose_task("请帮我写一首诗");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, 1);
    }

    #[test]
    fn test_single_task_english() {
        let result = decompose_task("write a python script");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_parallel_keyword_cn() {
        let result = decompose_task("请同时做三件事：分析数据。生成报告。发送邮件");
        assert!(result.len() >= 2, "should decompose into sub-tasks: got {}", result.len());
    }

    #[test]
    fn test_parallel_keyword_en() {
        let result = decompose_task("parallel: analyze the code and write tests");
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_step_by_step() {
        let result = decompose_task("step by step: parse the input, validate it, then process");
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_sub_task_has_ids() {
        let result = decompose_task("分步做：先检查。再执行。最后验证");
        assert_eq!(result[0].id, 1);
        assert_eq!(result[1].id, 2);
    }

    #[test]
    fn test_sub_task_kind_independent() {
        let result = decompose_task("同时处理：报告和图表");
        for sub in &result {
            assert_eq!(sub.kind, SubTaskKind::Independent);
        }
    }
}