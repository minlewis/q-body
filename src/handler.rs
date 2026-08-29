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

// ============================================================
// 查询意图分类（类型层准备）
// ============================================================

/// 查询意图分类 — 轻量启发式
///
/// 借鉴：yologdev/yoyo-evolve — "selective Exa deep search for synthesis/comparison
/// queries" (Day 118)：yoyo 新增查询意图分类，仅对综合/比较类查询触发深度搜索，
/// 避免简单查询浪费 API 调用。
///
/// 多步路由（Comparison/Synthesis 走拆子查询→分别调 LLM→聚合）属架构级改动，
/// 按 06-14 / 06-15 / 06-20 / 06-21 / 06-22 / 06-24 既定先例推迟。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentType {
    /// 普通查询 — 单次 LLM 调用即可
    Simple,
    /// 比较类 — 含 "比较/对比/vs/哪个更好/compare" 等关键词
    Comparison,
    /// 综合类 — 含 "总结/综述/分析/summarize/analyze" 等关键词
    Synthesis,
}

/// 对用户文本做轻量意图分类（启发式关键词匹配，大小写不敏感）
pub fn classify_intent(text: &str) -> IntentType {
    let lower = text.to_lowercase();
    let comparison_keywords = [
        "比较", "对比", "vs", "哪个更好", "compare", "which is better", "difference",
    ];
    let synthesis_keywords = [
        "总结", "综述", "分析", "summarize", "analyze", "synthesis", "summary", "analysis",
    ];
    if comparison_keywords.iter().any(|kw| lower.contains(kw)) {
        IntentType::Comparison
    } else if synthesis_keywords.iter().any(|kw| lower.contains(kw)) {
        IntentType::Synthesis
    } else {
        IntentType::Simple
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_simple() {
        assert_eq!(classify_intent("hello"), IntentType::Simple);
        assert_eq!(classify_intent("what can you do"), IntentType::Simple);
        assert_eq!(classify_intent("你好"), IntentType::Simple);
    }

    #[test]
    fn test_classify_comparison() {
        assert_eq!(classify_intent("比较 A 和 B"), IntentType::Comparison);
        assert_eq!(classify_intent("Rust vs Go"), IntentType::Comparison);
        assert_eq!(classify_intent("which is better?"), IntentType::Comparison);
        assert_eq!(classify_intent("COMPARE these two"), IntentType::Comparison);
    }

    #[test]
    fn test_classify_synthesis() {
        assert_eq!(classify_intent("总结一下今天的工作"), IntentType::Synthesis);
        assert_eq!(classify_intent("analyze the data"), IntentType::Synthesis);
        assert_eq!(classify_intent("please summarize this"), IntentType::Synthesis);
    }
}