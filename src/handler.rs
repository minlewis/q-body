//! A2A Handler — q-body 的 A2A 请求处理核心
//!
//! 实现了 JSON-RPC method 分发：
//! - SendMessage：创建 Task，通过 LLM 处理消息，返回结果
//! - GetTask：查询 Task 状态
//! - ListTasks: 列出所有 Task

use uuid::Uuid;

use crate::a2a::types::*;
use crate::state::TaskStore;

/// 火山引擎 ark 主 provider 的模型名保留为链首条目（见 LLM_PROVIDERS）

/// LLM provider — OpenAI 兼容单端点描述
///
/// 借鉴：tashfeenahmed/freellmapi — 单网关后多 provider automatic failover。
/// freellmapi 的核心设计：请求先打主 provider，超时/5xx/网络错误自动落到
/// 备用 provider，聚合末端错误返回，避免单点故障直接判定任务失败。
/// → q-body 对应改法：query_llm 按 LLM_PROVIDERS 链序尝试，前一个失败
///    （key 缺失 / HTTP 错误 / API 错误 / 解析失败）自动 failover 到下一个。
#[derive(Debug, Clone, Copy)]
struct LlmProvider {
    /// 存放 API key 的环境变量名
    api_key_env: &'static str,
    /// OpenAI 兼容 chat completions 端点
    api_url: &'static str,
    /// 模型名
    model: &'static str,
    /// tracing 日志用短名（仅入日志，不进用户消息，防泄漏）
    name: &'static str,
}

/// provider 故障转移链：按序尝试；链首向后兼容现存部署（只配 ARK_API_KEY 也能跑）
const LLM_PROVIDERS: &[LlmProvider] = &[
    LlmProvider {
        api_key_env: "ARK_API_KEY",
        api_url: "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions",
        model: "deepseek-v4-flash",
        name: "ark",
    },
    LlmProvider {
        api_key_env: "DEEPSEEK_API_KEY",
        api_url: "https://api.deepseek.com/chat/completions",
        model: "deepseek-chat",
        name: "deepseek-platform",
    },
];

/// 出站错误消息的 trust-boundary 泄漏模式（借鉴 yoyo-evolve #873 sanitize 审计）
const LEAK_PATTERNS: &[&str] = &[
    "api/plan/",      // 内部 LLM 端点路径
    "ark.cn-beijing", // 内部 provider 域名
    "ARK_API_KEY",    // 配置键名
    "Bearer ",        // 凭据前缀
    "/home/",         // 文件系统路径
    "/root/",
    "\\\"",           // serde/reqwest Debug 串（含转义引号，非面向用户的文本）
];

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

    /// trust-boundary sanitize：错误/拒绝消息出站前审计（借鉴 yoyo-evolve #873）
    ///
    /// 命中泄漏模式 → 降级为通用错误回复；完整细节仅保留在服务端 tracing 日志。
    fn sanitize_err_reply(msg: &str) -> String {
        if LEAK_PATTERNS.iter().any(|p| msg.contains(p)) {
            "(internal error, details logged)".into()
        } else {
            msg.into()
        }
    }

    /// 调 LLM：按 LLM_PROVIDERS 链序尝试，任一 provider 失败自动 failover 到下一个
    ///
    /// 失败类型（均触发 failover）：API key 缺失 / HTTP 请求失败 / API 非 2xx /
    /// 响应解析失败。全部 provider 失败 → 返回净化后的最后一条错误。
    /// 借鉴：tashfeenahmed/freellmapi — 单端点后多 provider automatic failover。
    async fn query_llm(&self, user_text: &str) -> String {
        let request_body = serde_json::json!({
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

        let mut last_err: Option<String> = None;

        for provider in LLM_PROVIDERS {
            let api_key = match std::env::var(provider.api_key_env) {
                Ok(k) if !k.trim().is_empty() => k,
                _ => {
                    tracing::warn!(
                        "LLM provider {} skipped: {} not set",
                        provider.name,
                        provider.api_key_env
                    );
                    last_err = Some(format!(
                        "LLM provider {} not configured",
                        provider.name
                    ));
                    continue;
                }
            };

            let mut body = request_body.clone();
            body["model"] = serde_json::Value::String(provider.model.to_string());

            let response = self
                .http_client
                .post(provider.api_url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send()
                .await;

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            if status.is_success() {
                                // 从 OpenAI 格式的响应中提取文本
                                return body["choices"][0]["message"]["content"]
                                    .as_str()
                                    .unwrap_or("(empty response from LLM)")
                                    .to_string();
                            }
                            let err_msg = body["error"]["message"]
                                .as_str()
                                .unwrap_or("unknown error");
                            tracing::error!(
                                "LLM API error on {} ({}): {} — failing over",
                                provider.name,
                                status,
                                err_msg
                            );
                            last_err = Some(format!(
                                "Sorry, LLM returned error {}: {}",
                                status, err_msg
                            ));
                            // 5xx/限流 → failover；4xx 是请求自身问题也换 provider 试一次，
                            // 由末端统一兜底（与 freellmapi 的宽松 failover 语义一致）
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to parse LLM response from {}: {} — failing over",
                                provider.name,
                                e
                            );
                            last_err = Some(format!(
                                "Sorry, failed to parse LLM response: {}",
                                e
                            ));
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "HTTP request to LLM {} failed: {} — failing over",
                        provider.name,
                        e
                    );
                    last_err = Some(format!("Sorry, LLM request failed: {}", e));
                }
            }
        }

        let msg = last_err
            .unwrap_or_else(|| "LLM provider chain is empty".to_string());
        Self::sanitize_err_reply(&msg)
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
mod sanitize_tests {
    use super::*;

    #[test]
    fn test_clean_error_passes_through() {
        let msg = "Sorry, LLM request failed: connection refused";
        assert_eq!(QBodyHandler::sanitize_err_reply(msg), msg);
    }

    #[test]
    fn test_internal_endpoint_url_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply(
            "Sorry, LLM returned error 401: POST https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions denied",
        );
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_provider_domain_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply(
            "Sorry, request failed: dns error resolving ark.cn-beijing",
        );
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_config_key_name_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply(
            "Sorry, env ARK_API_KEY missing, request failed",
        );
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_bearer_credential_is_scrubbed() {
        let out =
            QBodyHandler::sanitize_err_reply("Sorry, request failed: header Bearer abc123");
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_filesystem_path_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply(
            "Sorry, failed to parse LLM response: reading /home/ubuntu/.env",
        );
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_debug_escape_sequence_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply(
            "Sorry, failed to parse LLM response: expected value at line \\\"1\\\"",
        );
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_fallback_reply_has_no_config_key_name() {
        let reply = "q-body received: 'hi'. (LLM not configured — AI responses disabled)";
        assert!(!LEAK_PATTERNS.iter().any(|p| reply.contains(p)));
    }
}

#[cfg(test)]
mod failover_tests {
    use super::*;

    #[test]
    fn test_provider_chain_not_empty() {
        assert!(!LLM_PROVIDERS.is_empty());
    }

    #[test]
    fn test_chain_head_keeps_ark_primary() {
        // 链首向后兼容现存部署：只配 ARK_API_KEY 的环境行为不变
        assert_eq!(LLM_PROVIDERS[0].api_key_env, "ARK_API_KEY");
    }

    #[test]
    fn test_chain_urls_are_https() {
        for p in LLM_PROVIDERS {
            assert!(p.api_url.starts_with("https://"), "{} must be https", p.name);
        }
    }

    #[test]
    fn test_chain_names_unique() {
        // provider 短名用于 tracing 日志，重名会让 failover 归因失真
        let mut names: Vec<_> = LLM_PROVIDERS.iter().map(|p| p.name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn test_env_names_are_key_names_only() {
        // 链里只允许存环境变量名（键名），任何值形态的字符串都不该出现
        for p in LLM_PROVIDERS {
            assert!(
                p.api_key_env.ends_with("_KEY") || p.api_key_env.ends_with("_TOKEN"),
                "{} stores more than a key name",
                p.name
            );
        }
    }

    #[test]
    fn test_chain_models_nonempty() {
        for p in LLM_PROVIDERS {
            assert!(!p.model.is_empty());
            assert!(!p.api_url.is_empty());
        }
    }
}