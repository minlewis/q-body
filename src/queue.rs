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

/// 用户取消 vs 真实失败的分类词表（借鉴 yoyo-evolve #988 —
/// Ctrl-C 取消的子 agent 不得被误读为失败）
///
/// #855 同款纪律：词表收窄到测试许可的程度——
/// 每条正路径短语配一条「prose 误燃」负路径断言钉住。
const USER_CANCELLATION_PATTERNS: &[&str] = &[
    "operation canceled",
    "operation cancelled",
    "request canceled",
    "request cancelled",
];

/// 判定 LLM HTTP 错误是否为用户主动取消（而非真实网络失败）。
///
/// 取消 → 不记 failure 事件、不触发 failover；真实失败 → 原链路。
///
/// #1126 校准（2026-11-04）：真实 reqwest 错误是 source 链——display 层
/// `e.to_string()` 只给 "error sending request for url (...)"，关键判别文本
/// （"dns error" / "operation canceled" 等）可能只在 source 链深处。只匹配
/// display 层会让取消判别依赖"猜测版表层"，与真实系统报错脱节（yoyo #1126
/// 的 matcher 匹配 'dns resolution failed' 而真实文本是 'dns error' 同型）。
/// 因此把整个 source 链拼成全文再判定，词表不变、判定域扩大。
pub fn is_user_cancellation(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    USER_CANCELLATION_PATTERNS.iter().any(|p| lower.contains(p))
}

/// 拼接一条 `std::error::Error` 的完整 source 链文本（display + 逐层 source）。
///
/// 真实 reqwest/hyper 错误的判别文本常只出现在 source 链深处，display 层
/// （`e.to_string()`）拿不到——这正是 yoyo #1126 的教训。调用方应把 `&e`
/// 经本函数展开后喂给 `is_user_cancellation` / 日志 / 诊断，而非裸 `e.to_string()`。
pub fn error_chain_text(e: &dyn std::error::Error) -> String {
    let mut out = String::new();
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(c) = cur {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&c.to_string());
        cur = c.source();
    }
    out
}

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
        let event = LlmParseEvent::record(LlmFailureKind::JsonParse, None, 0, &long_err);
        assert!(event.error_summary.len() <= 80);
        assert!(event.error_summary.ends_with("..."));
    }

    #[test]
    fn test_short_error_untouched() {
        let short = "expected value at line 1 column 1";
        let event = LlmParseEvent::record(LlmFailureKind::JsonParse, None, 0, short);
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

    // === 取消分类词表（借鉴 yoyo-evolve #988）===
    // #855 纪律：正路径短语 + prose 误燃负路径样本成对钉住

    #[test]
    fn test_cancellation_positive_paths() {
        for msg in [
            "operation canceled",
            "Operation Cancelled",
            "HTTP request failed: operation canceled by hyper",
            "the request was cancelled before completion, reported as: operation cancelled",
        ] {
            assert!(is_user_cancellation(msg), "should be cancellation: {msg}");
        }
    }

    #[test]
    fn test_real_failures_do_not_misfire() {
        for msg in [
            "connection refused: tcp connect error",
            "dns error resolving api.example.com",
            "connection timed out after 30s",
            "channel closed",
            "request body failed to send",
            "we canceled the deadline extension discussion in the changelog notes",
        ] {
            assert!(
                !is_user_cancellation(msg),
                "real failure misclassified as cancellation: {msg}"
            );
        }
    }

    #[test]
    fn test_word_boundary_not_required_but_phrase_is_exact() {
        // 词表是完整短语匹配，不是裸词——裸 "cancel" 会误燃 prose
        assert!(!is_user_cancellation("canceled")); // 裸词不够，必须有完整短语
        assert!(is_user_cancellation("error: operation canceled"));
    }

    #[test]
    fn test_cancellation_kind_not_in_failure_vocab() {
        // 取消不是 failure kind：LlmFailureKind 三个变体都不该出现在取消路径上
        let kinds = [
            LlmFailureKind::HttpRequest.as_str(),
            LlmFailureKind::ApiError.as_str(),
            LlmFailureKind::JsonParse.as_str(),
        ];
        assert!(!kinds.contains(&"canceled"));
        assert!(!kinds.contains(&"cancelled"));
    }

    // === error_chain_text：展开 source 链，锚定真实系统报错全文（yoyo #1126 校准）===
    // 构造一个多层 source 错误模拟 reqwest 真实错误：
    //   display: "error sending request for url (...)"
    //   source:  "client error (Connect)"
    //   source:  "dns error"
    // 关键判别文本 "dns error" 只在链深处，display 层拿不到。
    #[derive(Debug)]
    struct ChainErr(&'static str, Option<Box<dyn std::error::Error>>);
    impl std::fmt::Display for ChainErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl std::error::Error for ChainErr {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref()
        }
    }

    fn make_chain() -> ChainErr {
        ChainErr(
            "error sending request for url (https://x.invalid/)",
            Some(Box::new(ChainErr(
                "client error (Connect)",
                Some(Box::new(ChainErr("dns error", None))),
            ))),
        )
    }

    #[test]
    fn test_error_chain_text_contains_all_layers() {
        let e = make_chain();
        let chain = error_chain_text(&e);
        // 三层全在拼接全文里
        assert!(chain.contains("error sending request for url"));
        assert!(chain.contains("client error (Connect)"));
        assert!(chain.contains("dns error"), "source 链深处文本必须可见: {chain}");
    }

    #[test]
    fn test_display_only_misses_source_depth() {
        // 关键判别文本只在 source 链深处——display 层判定会漏（#1126 教训）
        let e = make_chain();
        let display = format!("{e}");
        assert!(!display.contains("dns error"), "display 层不含深层文本: {display}");
        let chain = error_chain_text(&e);
        assert!(chain.contains("dns error"));
    }

    #[test]
    fn test_cancellation_marker_in_deep_source_is_detected() {
        // 取消标记只出现在 source 链深处（display 层没有）——展开 source 链后
        // is_user_cancellation 命中；只匹配 display（e.to_string()）会漏判（#1126 教训）。
        // 词表保持 #855 窄语义："operation cancelled" 是关键判别短语。
        let display_only = "error sending request for url (https://x.invalid/)";
        let marker = error_chain_text(&ChainErr(
            display_only,
            Some(Box::new(ChainErr(
                "the request was cancelled before completion, reported as: operation cancelled",
                None,
            ))),
        ));
        // display 层不含判别短语
        assert!(!display_only.to_ascii_lowercase().contains("operation cancelled"));
        // 展开 source 链后命中
        assert!(is_user_cancellation(&marker));
    }

    #[test]
    fn test_real_dns_failure_not_cancelled_even_in_chain() {
        // 真实 DNS 失败展开全链后仍不是取消（不误燃）——词表不变，判定域扩大而非误伤
        let e = make_chain();
        let chain = error_chain_text(&e);
        assert!(!is_user_cancellation(&chain), "DNS 失败不是取消: {chain}");
    }
}
