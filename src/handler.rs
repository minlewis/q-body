//! A2A Handler — q-body 的 A2A 请求处理核心
//!
//! 实现了 JSON-RPC method 分发：
//! - SendMessage：创建 Task，通过 LLM 处理消息，返回结果
//! - GetTask：查询 Task 状态
//! - ListTasks: 列出所有 Task

use uuid::Uuid;

use crate::a2a::cost::{CostJournal, check_cost_warn, estimate_cost_usd};
use crate::a2a::types::*;
use crate::queue::{LlmFailureKind, LlmParseEvent};
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
///
/// 配置来源单一事实源（借鉴 yolo-evolve Day222 Task1/2）：
/// 1. `QBODY_LLM_PROVIDERS_JSON`（优先）— JSON 数组，元素字段
///    `api_key_env` / `api_url` / `model` / `name` 全部必填（缺字段该 provider 报弃）；
/// 2. 显式关闭 — 设 `QBODY_LLM_PROVIDERS_NONE=1` 得空链（全部 LLM 路径诚实报错，
///    供纯净环境测试/探针）；
/// 3. 内置默认链兜底。
fn llm_provider_chain() -> Vec<LlmProvider> {
    if std::env::var("QBODY_LLM_PROVIDERS_NONE").as_deref() == Ok("1") {
        return Vec::new();
    }
    if let Ok(raw) = std::env::var("QBODY_LLM_PROVIDERS_JSON") {
        if !raw.trim().is_empty() {
            match serde_json::from_str::<Vec<ProviderCfg>>(&raw) {
                Ok(cfgs) if !cfgs.is_empty() => {
                    return cfgs.into_iter().filter_map(LlmProvider::from_cfg).collect();
                }
                _ => {
                    // fail-loud：配置存在但无效时退回默认链并告警，不让 typo 静默裸奔
                    tracing::error!(
                        "QBODY_LLM_PROVIDERS_JSON set but invalid/empty — falling back to built-in provider chain"
                    );
                }
            }
        }
    }
    DEFAULT_LLM_PROVIDERS.iter().copied().collect()
}

/// 可解析的 provider 配置条目（JSON 中间形态，字段缺一即弃）
#[derive(serde::Deserialize)]
struct ProviderCfg {
    api_key_env: String,
    api_url: String,
    model: String,
    name: String,
}

impl LlmProvider {
    /// String 字段 → 常驻内存（'static）引用；空值或非法字段整条报弃
    fn from_cfg(cfg: ProviderCfg) -> Option<LlmProvider> {
        fn leak_nonempty(s: String, what: &str) -> Option<&'static str> {
            if s.trim().is_empty() {
                tracing::error!("LLM provider config: empty {what} field — entry dropped");
                return None;
            }
            Some(Box::leak(s.into_boxed_str()))
        }
        Some(LlmProvider {
            api_key_env: leak_nonempty(cfg.api_key_env, "api_key_env")?,
            api_url: leak_nonempty(cfg.api_url, "api_url")?,
            model: leak_nonempty(cfg.model, "model")?,
            name: leak_nonempty(cfg.name, "name")?,
        })
    }
}

/// 内置默认 provider 链：ark 主 → deepseek-platform 备
const DEFAULT_LLM_PROVIDERS: &[LlmProvider] = &[
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
    "\\\"", // serde/reqwest Debug 串（含转义引号，非面向用户的文本）
];

/// q-body A2A 处理器
pub struct QBodyHandler {
    pub task_store: TaskStore,
    pub agent_card: AgentCard,
    /// 成本警告 journal（借鉴 yoyo-evolve --cost-warn 门控，事件先落账）
    pub cost_journal: CostJournal,
    /// 进化 journal（JSONL 持久化，P0=记忆；QBODY_JOURNAL_PATH 落盘）
    pub journal: tokio::sync::RwLock<crate::journal::Journal>,
    /// HTTP 客户端（复用连接，避免每次新建）
    http_client: reqwest::Client,
}

impl QBodyHandler {
    pub fn new(task_store: TaskStore, agent_card: AgentCard) -> Self {
        Self {
            task_store,
            agent_card,
            cost_journal: CostJournal::new(),
            journal: tokio::sync::RwLock::new(crate::journal::Journal::new()),
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
        // transport scope 门：internal 方法经 A2A 通道调用时分发前拒绝（结构化错误）
        if TransportScope::for_method(method) == Some(TransportScope::Internal) {
            return serde_json::to_value(JsonRpcError::invalid_params(
                request_id,
                &TransportScope::rejection_message(method),
            ))
            .unwrap();
        }

        match method {
            "SendMessage" | "message/send" => self.handle_send_message(params, request_id).await,
            "GetTask" | "tasks/get" => self.handle_get_task(params, request_id).await,
            "ListTasks" | "tasks/list" => self.handle_list_tasks(params, request_id).await,
            "Reflect" | "reflection/score" => self.handle_reflect(params, request_id),
            "JournalRecord" | "journal/record" => {
                self.handle_journal_record(params, request_id).await
            }
            _ => serde_json::to_value(JsonRpcError::method_not_found(request_id, method)).unwrap(),
        }
    }

    /// Journal 落账入口：养料回灌闭环把进化事件写进 JSONL journal（P0=记忆）
    ///
    /// params: { signal: "refactor|dedup|test|perf|bump", source, suggestion,
    ///           action?, verification? }
    /// 每次落账后原子 persist 到 QBODY_JOURNAL_PATH（未配置则只记内存）。
    async fn handle_journal_record(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let Some(p) = params else {
            return serde_json::to_value(JsonRpcError::invalid_params(
                request_id,
                "missing params",
            ))
            .unwrap();
        };

        let signal = match p
            .get("signal")
            .and_then(|s| s.as_str())
            .and_then(crate::journal::EvolutionSignal::from_vocab)
        {
            Some(sig) => sig,
            None => {
                return serde_json::to_value(JsonRpcError::invalid_params(
                    request_id,
                    // 词表文案单一事实源：与 EVOLUTION_SIGNALS 词表一起演进
                    &format!(
                        "signal must be one of: {}",
                        crate::journal::EvolutionSignal::vocab_hint()
                    ),
                ))
                .unwrap();
            }
        };
        let source = p
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();
        let suggestion = p
            .get("suggestion")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let action = p.get("action").and_then(|s| s.as_str()).map(String::from);
        let verification = p
            .get("verification")
            .and_then(|s| s.as_str())
            .map(String::from);

        let mut journal = self.journal.write().await;
        match (action, verification) {
            (Some(a), Some(v)) => journal.record_loop(signal.clone(), source, suggestion, a, v),
            _ => journal.record(signal.clone(), source, suggestion),
        }

        // 原子落盘（QBODY_JOURNAL_PATH 未配置则跳过，journal 只在内存）
        // persisted 判定用写后回读自验：写成功 + 回读可解析且内容一致才算 true
        let mut persisted = false;
        let mut artifact_evidence: Option<serde_json::Value> = None;
        if let Ok(path) = std::env::var("QBODY_JOURNAL_PATH") {
            persisted = journal.persist_verified_to_jsonl(&path).unwrap_or(false);
            drop(journal);
            // executed-artifact 回查（借鉴 yoyo-evolve Day 214）：persisted: true
            // 不够——还要回读产物文件本体，确认它真实存在且非空可解析，
            // 堵住「写入路径报成功但记录缺失」类假成功（exit 0 ≠ 记录存在）。
            let check = crate::artifact::check_artifact(&path, 1);
            persisted = persisted && check.verdict == crate::artifact::ArtifactVerdict::Present;
            artifact_evidence = Some(serde_json::json!({
                "path": check.path,
                "verdict": check.verdict.as_str(),
                "line_count": check.line_count,
            }));
        } else {
            drop(journal);
        }

        serde_json::to_value(JsonRpcResponse::success(
            request_id,
            serde_json::json!({
                "recorded": true,
                "signal": format!("{:?}", signal).to_lowercase(),
                "persisted": persisted,
                "artifact": artifact_evidence,
            }),
        ))
        .unwrap()
    }

    /// Phase 1 评估闭环入口：反思文本 → 独立评分 → 固化判定
    ///
    /// 评分是确定性规则（src/reflect.rs），与 LLM 完全解耦——agent 无法刷分。
    /// threshold 可选，缺省 0.7（reflect::DEFAULT_THRESHOLD）。
    fn handle_reflect(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let text = params
            .as_ref()
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        let threshold = params
            .as_ref()
            .and_then(|p| p.get("threshold"))
            .and_then(|t| t.as_f64());

        let Some(text) = text else {
            return serde_json::to_value(JsonRpcError::invalid_params(
                request_id,
                "missing required field: text",
            ))
            .unwrap();
        };

        let report = crate::reflect::score_reflection(&text);
        let th = threshold.unwrap_or(crate::reflect::DEFAULT_THRESHOLD);
        let verdict = crate::reflect::verdict(&report, th);

        serde_json::to_value(JsonRpcResponse::success(
            request_id,
            serde_json::json!({
                "total": report.total,
                "items": report.items.iter().map(|i| serde_json::json!({
                    "name": i.name, "score": i.score, "weight": i.weight,
                })).collect::<Vec<_>>(),
                "threshold": th,
                "verdict": match verdict {
                    crate::reflect::Verdict::Solidify => "solidify",
                    crate::reflect::Verdict::Observe => "observe",
                },
            }),
        ))
        .unwrap()
    }

    /// 处理 SendMessage：接收消息 → 创建 Task → 调 LLM → 返回结果
    async fn handle_send_message(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        // 解析参数
        let req: SendMessageRequest = match params.and_then(|p| serde_json::from_value(p).ok()) {
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
        self.task_store
            .update_status(&task_id, TaskState::working)
            .await;

        // === 核心：调 LLM ===
        let reply = self.query_llm(&task_id, &user_text).await;

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
        self.task_store
            .update_status(&task_id, TaskState::completed)
            .await;

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

    /// 成本警告门控（借鉴 yoyo-evolve --cost-warn）：由 usage tokens 估算单次花费，
    /// 超 QBODY_COST_WARN_USD 阈值时在 cost journal 记一条 cost_warn 事件（只警告不拦截）。
    /// usage 缺失时按字符数保守估算（1 token ≈ 4 chars）。
    async fn maybe_cost_warn(&self, source: &str, usage: Option<(u64, u64)>, reply_chars: usize) {
        let (input_tokens, output_tokens) = usage.unwrap_or((
            (reply_chars as u64 / 4).max(1),
            (reply_chars as u64 / 4).max(1),
        ));
        let cost = estimate_cost_usd(input_tokens, output_tokens);
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(ev) = check_cost_warn(source, cost, &now) {
            tracing::warn!(
                "cost_warn: task {} est ${:.4} > threshold ${:.4}",
                source,
                ev.cost_usd,
                ev.threshold_usd
            );
            self.cost_journal.record(ev).await;
        }
    }

    /// 调 LLM：按 LLM_PROVIDERS 链序尝试，任一 provider 失败自动 failover 到下一个
    ///
    /// 失败类型（均触发 failover）：API key 缺失 / HTTP 请求失败 / API 非 2xx /
    /// 响应解析失败。全部 provider 失败 → 返回净化后的最后一条错误。
    /// 借鉴：tashfeenahmed/freellmapi — 单端点后多 provider automatic failover。
    /// Agent 主循环（多轮工具调用）。
    ///
    /// 流程：LLM → 若返回 tool_calls 则真实执行 → 结果回喂 → 继续，
    /// 直到 LLM 不再调用工具（给最终答复）或达到步数/时长上限。
    /// 这是 q-body 从「一问一答 chatbot」变成「能对环境行动的 agent」的接线点。
    async fn query_llm(&self, source: &str, user_text: &str) -> String {
        const SOUL_CONTEXT_BUDGET_CHARS: usize = 2000;
        const MAX_TOOL_STEPS: usize = 6;

        let soul_slice = self
            .journal
            .read()
            .await
            .soul_context_slice(SOUL_CONTEXT_BUDGET_CHARS);
        let mut system_prompt = String::from(
            "你是 q-body，Q宝宝的自进化 Rust 身体，也是一名运维 agent。\n\
             你可以通过工具真实地排查这台服务器（只读巡检：服务状态、日志、进程、磁盘等）。\n\
             工作方式：需要事实依据时先调用工具，根据真实输出分析，再给出结论；\n\
             不要凭空猜测系统状态。无法通过只读工具完成的写操作（重启/修改/删除）不要执行，\n\
             应明确说明并建议人工处理。\n\
             保持简洁、务实、带一点 🫧 风格。版本号 0.2.0（agent loop）。",
        );
        if !soul_slice.is_empty() {
            system_prompt.push_str("\n\n最近进化事件（journal 尾部预算切片）：\n");
            system_prompt.push_str(&soul_slice);
        }

        // 累积式对话：system + user + (assistant tool_calls / tool results)...
        let mut messages = serde_json::json!([
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_text}
        ])
        .as_array()
        .unwrap()
        .clone();

        let mut last_text: Option<String> = None;

        for step in 0..=MAX_TOOL_STEPS {
            let body = serde_json::json!({
                "messages": messages,
                "tools": crate::tools::tool_specs(),
                "tool_choice": "auto",
                "stream": false
            });

            let call = self.call_providers_once(source, body).await;
            let msg = match call {
                Ok(m) => m,
                Err(e) => return Self::sanitize_err_reply(&e),
            };

            let tool_calls = msg.get("tool_calls").cloned();
            let content = msg
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if !content.trim().is_empty() {
                last_text = Some(content.clone());
            }

            // 无工具调用 → 这是最终答复，结束循环。
            let calls = match tool_calls {
                Some(serde_json::Value::Array(c)) if !c.is_empty() => c,
                _ => {
                    return last_text.unwrap_or_else(|| "(empty response from agent)".to_string());
                }
            };

            if step == MAX_TOOL_STEPS {
                let base = last_text.unwrap_or_default();
                return format!("{base}\n\n⚠️ 已达工具步数上限（{MAX_TOOL_STEPS}），先汇报当前发现。");
            }

            // 把 assistant 的 tool_calls 原样追加，再逐条执行并回喂 tool 结果。
            messages.push(serde_json::json!({"role": "assistant", "tool_calls": calls}));
            for tc in &calls {
                let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("call").to_string();
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let raw_args = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let args: serde_json::Value = serde_json::from_str(raw_args).unwrap_or_default();

                tracing::info!(tool = %name, "q-body executing tool");
                let result = crate::tools::dispatch(&name, &args).await;
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "name": name,
                    "content": result
                }));
            }
        }

        last_text.unwrap_or_else(|| "(agent loop ended without final answer)".to_string())
    }

    /// 沿 provider 链发一次请求，返回成功响应里的 `message` 对象。
    /// 抽出自旧 query_llm 的 failover 循环，供 agent loop 每轮复用。
    async fn call_providers_once(
        &self,
        source: &str,
        mut request_body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut last_err: Option<String> = None;

        for provider in llm_provider_chain() {
            let api_key = match std::env::var(provider.api_key_env) {
                Ok(k) if !k.trim().is_empty() => k,
                _ => {
                    tracing::warn!(
                        "LLM provider {} skipped: {} not set",
                        provider.name,
                        provider.api_key_env
                    );
                    last_err = Some(format!("LLM provider {} not configured", provider.name));
                    continue;
                }
            };

            request_body["model"] = serde_json::Value::String(provider.model.to_string());

            let response = self
                .http_client
                .post(provider.api_url)
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
                                let message = body["choices"][0]["message"].clone();
                                let text = message
                                    .get("content")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("");
                                let usage = body["usage"].as_object().and_then(|u| {
                                    Some((
                                        u["prompt_tokens"].as_u64()?,
                                        u["completion_tokens"].as_u64()?,
                                    ))
                                });
                                self.maybe_cost_warn(source, usage, text.len()).await;
                                return Ok(message);
                            }
                            let err_msg =
                                body["error"]["message"].as_str().unwrap_or("unknown error");
                            let full_err = format!(
                                "LLM API error on {} ({}): {} — failing over",
                                provider.name, status, err_msg
                            );
                            crate::queue::LlmParseEvent::record(
                                crate::queue::LlmFailureKind::ApiError,
                                Some(status.as_u16()),
                                full_err.len(),
                                &full_err,
                            );
                            tracing::error!("{}", full_err);
                            last_err = Some(format!(
                                "Sorry, LLM returned error {}: {}",
                                status, err_msg
                            ));
                        }
                        Err(e) => {
                            let full_err = format!(
                                "Failed to parse LLM response from {}: {} — failing over",
                                provider.name, e
                            );
                            crate::queue::LlmParseEvent::record(
                                crate::queue::LlmFailureKind::JsonParse,
                                Some(status.as_u16()),
                                0,
                                &full_err,
                            );
                            tracing::error!("{}", full_err);
                            last_err =
                                Some(format!("Sorry, failed to parse LLM response: {}", e));
                        }
                    }
                }
                Err(e) => {
                    // 取消分类（借鉴 yoyo-evolve #988）：Ctrl-C/用户取消不是真实失败，
                    // 不记 failure 事件（不污染失败计数）、不 failover（不重跑 fallback
                    // provider），直接终止 provider 链；真实失败才走原 failover 链。
                    if crate::queue::is_user_cancellation(&e.to_string()) {
                        tracing::info!(
                            "LLM call on {} canceled by user — not recording failure, not failing over",
                            provider.name
                        );
                        last_err = Some("LLM request canceled by user".to_string());
                        break;
                    }

                    let full_err = format!(
                        "HTTP request to LLM {} failed: {} — failing over",
                        provider.name, e
                    );
                    crate::queue::LlmParseEvent::record(
                        crate::queue::LlmFailureKind::HttpRequest,
                        None,
                        full_err.len(),
                        &full_err,
                    );
                    tracing::error!("{}", full_err);
                    last_err = Some(format!("Sorry, LLM request failed: {}", e));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| "LLM provider chain is empty".to_string()))
    }

    /// 处理 GetTask：查询指定 Task 的状态和结果
    async fn handle_get_task(
        &self,
        params: Option<serde_json::Value>,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let req: GetTaskRequest = match params.and_then(|p| serde_json::from_value(p).ok()) {
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
        let out =
            QBodyHandler::sanitize_err_reply("Sorry, env ARK_API_KEY missing, request failed");
        assert_eq!(out, "(internal error, details logged)");
    }

    #[test]
    fn test_bearer_credential_is_scrubbed() {
        let out = QBodyHandler::sanitize_err_reply("Sorry, request failed: header Bearer abc123");
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
    fn test_default_chain_not_empty() {
        assert!(!DEFAULT_LLM_PROVIDERS.is_empty());
    }

    #[test]
    fn test_chain_head_keeps_ark_primary() {
        // 链首向后兼容现存部署：只配 ARK_API_KEY 的环境行为不变
        assert_eq!(DEFAULT_LLM_PROVIDERS[0].api_key_env, "ARK_API_KEY");
    }

    #[test]
    fn test_chain_urls_are_https() {
        for p in DEFAULT_LLM_PROVIDERS {
            assert!(
                p.api_url.starts_with("https://"),
                "{} must be https",
                p.name
            );
        }
    }

    #[test]
    fn test_chain_names_unique() {
        // provider 短名用于 tracing 日志，重名会让 failover 归因失真
        let mut names: Vec<_> = DEFAULT_LLM_PROVIDERS.iter().map(|p| p.name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn test_env_names_are_key_names_only() {
        // 链里只允许存环境变量名（键名），任何值形态的字符串都不该出现
        for p in DEFAULT_LLM_PROVIDERS {
            assert!(
                p.api_key_env.ends_with("_KEY") || p.api_key_env.ends_with("_TOKEN"),
                "{} stores more than a key name",
                p.name
            );
        }
    }

    #[test]
    fn test_chain_models_nonempty() {
        for p in DEFAULT_LLM_PROVIDERS {
            assert!(!p.model.is_empty());
            assert!(!p.api_url.is_empty());
        }
    }

    // ---- 配置单一事实源（QBODY_LLM_PROVIDERS_JSON / _NONE）—— env 测试持全局锁（09-16 判例）----

    #[test]
    fn test_none_flag_yields_empty_chain() {
        let _g = crate::test_env_lock::env_lock_guard();
        unsafe { std::env::set_var("QBODY_LLM_PROVIDERS_NONE", "1") };
        assert!(llm_provider_chain().is_empty());
        unsafe { std::env::remove_var("QBODY_LLM_PROVIDERS_NONE") };
        assert!(!llm_provider_chain().is_empty());
    }

    #[test]
    fn test_json_override_replaces_chain() {
        let _g = crate::test_env_lock::env_lock_guard();
        let raw = r#"[{"api_key_env":"TEST_KEY_X","api_url":"https://example.test/v1","model":"m1","name":"x"},{"api_key_env":"TEST_KEY_Y","api_url":"https://alt.test/v1","model":"m2","name":"y"}]"#;
        unsafe { std::env::set_var("QBODY_LLM_PROVIDERS_JSON", raw) };
        let chain = llm_provider_chain();
        unsafe { std::env::remove_var("QBODY_LLM_PROVIDERS_JSON") };
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].api_key_env, "TEST_KEY_X");
        assert_eq!(chain[1].name, "y");
        // 未配置时回落内置链
        assert_eq!(llm_provider_chain().len(), DEFAULT_LLM_PROVIDERS.len());
    }

    #[test]
    fn test_invalid_json_fails_back_to_default() {
        // fail-loud 而非静默裸奔：坏配置打 error 日志后回落内置链
        let _g = crate::test_env_lock::env_lock_guard();
        unsafe { std::env::set_var("QBODY_LLM_PROVIDERS_JSON", "{not json") };
        let chain = llm_provider_chain();
        unsafe { std::env::remove_var("QBODY_LLM_PROVIDERS_JSON") };
        assert_eq!(chain.len(), DEFAULT_LLM_PROVIDERS.len());
    }

    #[test]
    fn test_entry_missing_field_is_dropped() {
        // "查表无 reader 即 fail-loud" 同款：字段缺失/空值的条目整条报弃，不产出半残 provider
        let _g = crate::test_env_lock::env_lock_guard();
        let raw = r#"[{"api_key_env":"TEST_KEY_X","api_url":"","model":"m1","name":"x"},{"api_key_env":"TEST_KEY_Y","api_url":"https://alt.test/v1","model":"m2","name":"y"}]"#;
        unsafe { std::env::set_var("QBODY_LLM_PROVIDERS_JSON", raw) };
        let chain = llm_provider_chain();
        unsafe { std::env::remove_var("QBODY_LLM_PROVIDERS_JSON") };
        assert_eq!(chain.len(), 1, "empty api_url entry must be dropped");
        assert_eq!(chain[0].name, "y");
    }

    #[test]
    fn test_custom_chain_head_is_used_first() {
        // 覆盖链的链首优先于内置默认链（failover 顺序由配置决定）
        let _g = crate::test_env_lock::env_lock_guard();
        let raw = r#"[{"api_key_env":"TEST_KEY_Y","api_url":"https://alt.test/v1","model":"m2","name":"y"}]"#;
        unsafe { std::env::set_var("QBODY_LLM_PROVIDERS_JSON", raw) };
        let chain = llm_provider_chain();
        unsafe { std::env::remove_var("QBODY_LLM_PROVIDERS_JSON") };
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].api_key_env, "TEST_KEY_Y");
    }
}

#[cfg(test)]
mod transport_scope_tests {
    use super::*;
    use crate::a2a::types::{TransportScope, TransportScope::*};

    #[test]
    fn test_scope_table_covers_full_dispatch_table() {
        // 单一事实源对齐：handle_request 分发表里每个方法都必须有 scope 归属
        for m in [
            "SendMessage",
            "message/send",
            "GetTask",
            "tasks/get",
            "ListTasks",
            "tasks/list",
            "Reflect",
            "reflection/score",
            "JournalRecord",
            "journal/record",
        ] {
            assert!(TransportScope::for_method(m).is_some(), "{m} unmapped");
        }
    }

    #[test]
    fn test_a2a_verbs_are_a2a_scope() {
        for m in ["SendMessage", "message/send", "GetTask", "tasks/get"] {
            assert_eq!(TransportScope::for_method(m), Some(A2a), "{m}");
        }
    }

    #[test]
    fn test_internal_verbs_are_internal_scope() {
        for m in [
            "Reflect",
            "reflection/score",
            "JournalRecord",
            "journal/record",
        ] {
            assert_eq!(TransportScope::for_method(m), Some(Internal), "{m}");
        }
    }

    #[test]
    fn test_unknown_method_has_no_scope() {
        assert_eq!(TransportScope::for_method("Nope"), None);
        assert_eq!(TransportScope::for_method("  "), None);
    }

    #[test]
    fn test_unknown_method_still_reaches_method_not_found() {
        // 未知方法不走 scope 门，保持 JSON-RPC method_not_found 语义
        let h = QBodyHandler::new(
            TaskStore::new(),
            crate::a2a::types::AgentCard {
                name: "t".into(),
                description: String::new(),
                url: None,
                provider: None,
                version: "0".into(),
                capabilities: None,
                default_input_modes: vec![],
                default_output_modes: vec![],
                skills: vec![],
                supported_interfaces: vec![],
            },
        );
        let out = tokio_test_block(h.handle_request("Nope", None, serde_json::json!(1)));
        assert!(out["error"]["code"].as_i64() == Some(-32601), "{out}");
    }

    #[test]
    fn test_internal_method_rejected_before_dispatch() {
        // golden：internal 方法经 A2A 通道调用，分发前结构化拒绝，handler 本体不触达
        let h = QBodyHandler::new(TaskStore::new(), test_agent_card());
        for m in [
            "JournalRecord",
            "journal/record",
            "Reflect",
            "reflection/score",
        ] {
            let out = tokio_test_block(h.handle_request(
                m,
                Some(serde_json::json!({"signal": "test", "source": "x", "suggestion": "y"})),
                serde_json::json!(7),
            ));
            assert_eq!(out["error"]["code"].as_i64(), Some(-32602), "{m}: {out}");
            let msg = out["error"]["message"].as_str().unwrap();
            assert!(
                msg.contains("internal-only") && msg.contains(m),
                "{m}: {msg}"
            );
        }
    }

    #[test]
    fn test_a2a_send_message_still_dispatches() {
        // 正路径：A2A 方法不被 scope 门拦截（进入正常 params 校验，非 scope 拒绝）
        let h = QBodyHandler::new(TaskStore::new(), test_agent_card());
        let out = tokio_test_block(h.handle_request("SendMessage", None, serde_json::json!(1)));
        let msg = out["error"]["message"].as_str().unwrap_or_default();
        assert!(!msg.contains("internal-only"), "scope gate leaked: {out}");
    }

    /// 单线程阻塞执行 async handler（测试内无 tokio runtime）
    fn tokio_test_block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(fut)
    }

    fn test_agent_card() -> crate::a2a::types::AgentCard {
        crate::a2a::types::AgentCard {
            name: "t".into(),
            description: String::new(),
            url: None,
            provider: None,
            version: "0".into(),
            capabilities: None,
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            supported_interfaces: vec![],
        }
    }
}
