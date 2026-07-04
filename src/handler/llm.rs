//! LLM API 调用 — 火山引擎 deepseek-v4-flash

use crate::handler::QBodyHandler;

/// 火山引擎 deepseek-v4-flash 的 API 端点
const LLM_API_URL: &str = "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions";
const LLM_MODEL: &str = "deepseek-v4-flash";

impl QBodyHandler {
    /// 调 deepseek-v4-flash（火山引擎）
    pub async fn query_llm(&self, user_text: &str) -> String {
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
}