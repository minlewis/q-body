//! LLM 调用事件的量化日志模块
//!
//! 为 open PR #95 的 bounded retry 提供 data-licensed 收窄依据：
//! 在 LLM JSON parse 失败等发射点加结构化计数日志，
//! 先量化再收窄，而不是先拍脑袋定重试上限。
//!
//! 借鉴：yologdev/yoyo-evolve Day190 #855 —
//! measure the three remaining broad retry words at the emission point,
//! then narrow only what the measurement licenses

use std::time::SystemTime;

/// LLM 解析失败事件的类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmFailureKind {
    /// HTTP 请求失败（连接超时/DNS 错误等）
    HttpRequest,
    /// API 返回非 2xx 状态码
    ApiError,
    /// JSON 响应解析失败
    JsonParse,
}

impl LlmFailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HttpRequest => "http_request",
            Self::ApiError => "api_error",
            Self::JsonParse => "json_parse",
        }
    }
}

/// LLM 调用失败的结构化事件
///
/// 在每次失败发射点即时记录，为后续重试策略优化提供数据依据。
#[derive(Debug)]
pub struct LlmParseEvent {
    /// 失败类型
    pub kind: LlmFailureKind,
    /// HTTP 状态码（仅 ApiError 时有效）
    pub status_code: Option<u16>,
    /// 响应体长度（字节数），0 表示未知
    pub response_length: usize,
    /// 错误消息摘要（前 80 字符，避免日志爆炸）
    pub error_summary: String,
    /// 发生时刻（Unix 时间戳，秒）
    pub timestamp: u64,
}

impl LlmParseEvent {
    /// 创建一个新的 LLM 失败事件并记录结构化日志
    pub fn record(
        kind: LlmFailureKind,
        status_code: Option<u16>,
        response_length: usize,
        error_summary: &str,
    ) -> Self {
        let event = Self {
            kind,
            status_code,
            response_length,
            error_summary: {
                let s = error_summary.trim();
                if s.len() > 80 {
                    format!("{}...", &s[..77])
                } else {
                    s.to_string()
                }
            },
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        tracing::info!(
            target: "llm_failure",
            kind = event.kind.as_str(),
            status_code = event.status_code,
            response_length = event.response_length,
            error_summary = event.error_summary,
            timestamp = event.timestamp,
            "LLM call failure recorded",
        );

        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_as_str() {
        assert_eq!(LlmFailureKind::HttpRequest.as_str(), "http_request");
        assert_eq!(LlmFailureKind::ApiError.as_str(), "api_error");
        assert_eq!(LlmFailureKind::JsonParse.as_str(), "json_parse");
    }

    #[test]
    fn test_error_summary_is_truncated() {
        let long_err = "x".repeat(200);
        let event = LlmParseEvent::record(
            LlmFailureKind::JsonParse,
            None,
            0,
            &long_err,
        );
        assert!(event.error_summary.len() <= 80);
        assert!(event.error_summary.ends_with("..."));
    }

    #[test]
    fn test_short_error_untouched() {
        let short = "expected value at line 1 column 1";
        let event = LlmParseEvent::record(
            LlmFailureKind::JsonParse,
            None,
            0,
            short,
        );
        assert_eq!(event.error_summary, short);
    }

    #[test]
    fn test_http_request_event() {
        let event = LlmParseEvent::record(
            LlmFailureKind::HttpRequest,
            None,
            0,
            "connection refused: tcp connect error",
        );
        assert_eq!(event.kind, LlmFailureKind::HttpRequest);
        assert!(event.timestamp > 0);
    }

    #[test]
    fn test_api_error_with_status() {
        let event = LlmParseEvent::record(
            LlmFailureKind::ApiError,
            Some(429),
            145,
            "rate limit exceeded",
        );
        assert_eq!(event.status_code, Some(429));
        assert_eq!(event.response_length, 145);
    }

    #[test]
    fn test_json_parse_event() {
        let event = LlmParseEvent::record(
            LlmFailureKind::JsonParse,
            None,
            3281,
            "expected value at line 1 column 1",
        );
        assert_eq!(event.kind, LlmFailureKind::JsonParse);
        assert_eq!(event.response_length, 3281);
    }
}