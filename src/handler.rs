//! A2A Handler — q-body 的 A2A 请求处理核心
//!
//! 实现了 JSON-RPC method 分发：
//! - SendMessage：创建 Task，通过 LLM 处理消息，返回结果
//! - GetTask：查询 Task 状态
//! - ListTasks: 列出所有 Task

use uuid::Uuid;

use q_body::a2a::types::*;
use q_body::config::{Config, M3Config};
use q_body::journal::{JournalEntry, JournalStore};
use q_body::state::TaskStore;

/// q-body A2A 处理器
pub struct QBodyHandler {
    pub task_store: TaskStore,
    pub journal_store: JournalStore,
    pub agent_card: AgentCard,
    /// HTTP 客户端（复用连接，避免每次新建）
    http_client: reqwest::Client,
    /// LLM 配置 (默认 / OpenAI Chat Completions)
    llm_api_url: String,
    llm_model: String,
    llm_api_key_env: String,
    /// M3 provider 配置 (Anthropic Messages)
    m3: M3Config,
    /// 系统提示词
    system_prompt: String,
}

impl QBodyHandler {
    pub fn new(task_store: TaskStore, journal_store: JournalStore, agent_card: AgentCard, config: &Config) -> Self {
        Self {
            task_store,
            journal_store,
            agent_card,
            http_client: reqwest::Client::new(),
            llm_api_url: config.llm.api_url.clone(),
            llm_model: config.llm.model.clone(),
            llm_api_key_env: config.llm.api_key_env.clone(),
            m3: config.m3.clone(),
            system_prompt: config.agent.system_prompt.clone(),
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
            "JournalSave" | "journal/save" => {
                self.handle_journal_save(params, request_id).await
            }
            "JournalGet" | "journal/get" => {
                self.handle_journal_get(params, request_id).await
            }
            "JournalList" | "journal/list" => {
                self.handle_journal_list(params, request_id).await
            }
            "InferWithM3" | "infer/m3" => {
                self.handle_infer_with_m3(params, request_id).await
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

    /// 调 LLM（从配置中读取 API URL、模型、Key 环境变量名）
    async fn query_llm(&self, user_text: &str) -> String {
        let api_key = match std::env::var(&self.llm_api_key_env) {
            Ok(k) => k,
            Err(_) => {
                tracing::warn!("{} not set, falling back to static reply", self.llm_api_key_env);
                return format!(
                    "q-body received: '{}'. (LLM not configured — set {} to enable AI responses)",
                    user_text, self.llm_api_key_env
                );
            }
        };

        let request_body = serde_json::json!({
            "model": self.llm_model,
            "messages": [
                {
                    "role": "system",
                    "content": &self.system_prompt
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
            .post(&self.llm_api_url)
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

    /// 处理 JournalSave：手动保存 Journal 条目
    async fn handle_journal_save(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        #[derive(serde::Deserialize)]
        struct JournalSaveParams {
            task_id: String,
            summary: String,
            #[serde(default)]
            learnings: Vec<String>,
        }

        let req: JournalSaveParams = match params
            .and_then(|p| serde_json::from_value(p).ok())
        {
            Some(r) => r,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    "missing or invalid JournalSave params (need: task_id, summary)",
                ))
                .unwrap();
            }
        };

        let entry = JournalEntry::manual(&req.task_id, req.summary, req.learnings);
        self.journal_store.save(entry).await;

        let result = serde_json::json!({
            "status": "saved",
            "task_id": req.task_id,
        });
        serde_json::to_value(JsonRpcResponse::success(request_id, result)).unwrap()
    }

    /// 处理 JournalGet：查询指定 Task 的 Journal
    async fn handle_journal_get(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        #[derive(serde::Deserialize)]
        struct JournalGetParams {
            task_id: String,
        }

        let req: JournalGetParams = match params
            .and_then(|p| serde_json::from_value(p).ok())
        {
            Some(r) => r,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    "missing task_id",
                ))
                .unwrap();
            }
        };

        match self.journal_store.get(&req.task_id).await {
            Some(entry) => {
                serde_json::to_value(JsonRpcResponse::success(request_id, entry)).unwrap()
            }
            None => serde_json::to_value(JsonRpcError::invalid_params(
                request_id,
                &format!("journal not found for task: {}", req.task_id),
            ))
            .unwrap(),
        }
    }

    /// 处理 JournalList：列出所有 Journal ID
    async fn handle_journal_list(
        &self,
        _params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let ids = self.journal_store.list().await;
        let result = serde_json::json!({
            "task_ids": ids,
            "count": ids.len(),
        });
        serde_json::to_value(JsonRpcResponse::success(request_id, result)).unwrap()
    }

    // ============================================================
    // M3 Provider (Anthropic Messages 协议)
    // ============================================================

    /// 处理 InferWithM3：通过 M3 处理消息，支持文本 + 可选 url 形式 image/video
    ///
    /// 请求 params 格式：
    /// ```json
    /// {
    ///   "message": { "role": "user", "parts": [ {"text": "..."}, {"url": "...", "mediaType": "image/jpeg"} ] },
    ///   "system": "可选，覆盖默认 system_prompt"
    /// }
    /// ```
    async fn handle_infer_with_m3(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let req: SendMessageRequest = match params
            .and_then(|p| serde_json::from_value(p).ok())
        {
            Some(r) => r,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    "missing or invalid InferWithM3 params (need: message)",
                ))
                .unwrap();
            }
        };

        let task_id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let context_id = format!("ctx-m3-{}", &task_id[..8]);

        // 创建 Task
        self.task_store
            .create_task(task_id.clone(), context_id, req.message.clone())
            .await;
        self.task_store.update_status(&task_id, TaskState::working).await;

        // 调 M3
        let reply = self.query_llm_m3(&req.message.parts).await;

        // 组装响应
        let agent_msg = Message {
            role: "assistant".into(),
            parts: vec![Part::text(&reply)],
            message_id: Some(Uuid::new_v4().to_string()),
        };
        let artifact = Artifact {
            parts: vec![Part::text(&reply)],
            name: Some("m3-response".into()),
            last_chunk: Some(true),
        };
        self.task_store
            .add_reply(&task_id, agent_msg, vec![artifact])
            .await;
        self.task_store.update_status(&task_id, TaskState::completed).await;

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

    /// 调 M3（Anthropic Messages 协议）
    ///
    /// 把 A2A Part 列表转成 Anthropic content blocks：
    /// - Part.text → { type: "text", text }
    /// - Part.url  → 下载 → { type: "image", source: { type: "base64", media_type, data } }
    async fn query_llm_m3(&self, parts: &[Part]) -> String {
        let api_key = match std::env::var(&self.m3.api_key_env) {
            Ok(k) => k,
            Err(_) => {
                tracing::warn!("{} not set, falling back to static reply", self.m3.api_key_env);
                return format!(
                    "q-body M3 received: '{} parts'. (M3 not configured — set {} to enable AI responses)",
                    parts.len(),
                    self.m3.api_key_env
                );
            }
        };

        // 构造 Anthropic content blocks
        let mut content_blocks: Vec<serde_json::Value> = Vec::new();
        for part in parts {
            if let Some(text) = &part.text {
                content_blocks.push(serde_json::json!({
                    "type": "text",
                    "text": text,
                }));
            } else if let (Some(url), Some(media_type)) = (&part.url, &part.media_type) {
                // 下载 url → base64
                match self.fetch_url_to_base64(url).await {
                    Ok(b64) => {
                        content_blocks.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": b64,
                            }
                        }));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch media from {}: {}", url, e);
                        return format!("Sorry, failed to fetch media: {}", e);
                    }
                }
            }
        }

        if content_blocks.is_empty() {
            return "q-body M3 received empty content.".to_string();
        }

        let request_body = serde_json::json!({
            "model": self.m3.model,
            "max_tokens": self.m3.max_tokens,
            "system": self.system_prompt,
            "messages": [
                {
                    "role": "user",
                    "content": content_blocks,
                }
            ],
        });

        let response = self
            .http_client
            .post(&self.m3.api_url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", &self.m3.anthropic_version)
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        if status.is_success() {
                            // Anthropic 响应: { content: [{type: "text", text: "..."} | {type: "thinking", thinking: "..."}], ... }
                            let text = body["content"]
                                .as_array()
                                .and_then(|arr| {
                                    arr.iter()
                                        .find(|b| b["type"] == "text")
                                        .and_then(|b| b["text"].as_str())
                                })
                                .unwrap_or("(empty M3 response)");
                            text.to_string()
                        } else {
                            let err_msg = body["error"]["message"]
                                .as_str()
                                .unwrap_or("unknown error");
                            tracing::error!("M3 API error ({}): {}", status, err_msg);
                            format!("Sorry, M3 returned error {}: {}", status, err_msg)
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse M3 response: {}", e);
                        format!("Sorry, failed to parse M3 response: {}", e)
                    }
                }
            }
            Err(e) => {
                tracing::error!("HTTP request to M3 failed: {}", e);
                format!("Sorry, M3 request failed: {}", e)
            }
        }
    }

    /// 下载 URL 内容并 base64 编码（M3 image content block 需要 base64）
    async fn fetch_url_to_base64(&self, url: &str) -> Result<String, String> {
        use base64::Engine;
        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("fetch failed: {}", e))?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("read body failed: {}", e))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }
}