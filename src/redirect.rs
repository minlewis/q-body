//! 跨域重定向凭据隔离（借鉴 yologdev/yoyo-evolve — cross-origin 307/302 凭据隔离测试，Fix #1181）
//!
//! 缺陷面：LLM provider 请求路径把 `Authorization: Bearer <key>` 加在请求头里，经
//! reqwest 发出。若上游返回 307/302 且重定向到另一 origin，凭据随重定向带到其他
//! 主机会把 API key 泄露给第三方域。reqwest 默认策略会剥离跨 host 的敏感头，但
//! 这是"依赖默认"——本条把它升级为 fail-closed 显式闸：跨域重定向一律 Stop，
//! 宁可请求停在 30x 响应也不把凭据带过 origin 边界（yoyo #982 fail-closed 语义）。

/// 重定向目标判定：是否落在同一 origin。
///
/// origin = scheme + host + 规范化端口（缺失时按 scheme 默认端口归一）。
/// 判定用 `reqwest::Url`（同 url crate re-export，不新增依赖）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectVerdict {
    /// 目标与来源 origin 完全一致（scheme/host/port 均同），可安全跟随。
    SameOrigin,
    /// 目标跨 origin（scheme/host/port 任一不同）。凭据不得携带，fail-closed 拒绝。
    CrossOrigin,
    /// 来源或目标 URL 无法解析（形态非法/相对路径），无 origin 可比，保守按跨域处理。
    Unresolvable,
}

impl RedirectVerdict {
    /// 是否允许携带凭据跟随重定向：仅同一 origin 放行。
    pub fn may_follow(&self) -> bool {
        matches!(self, RedirectVerdict::SameOrigin)
    }
}

/// 判定重定向 from → to 是否同一 origin。
pub fn redirect_verdict(from: &str, to: &str) -> RedirectVerdict {
    match (reqwest::Url::parse(from), reqwest::Url::parse(to)) {
        (Ok(src), Ok(dst)) => match origin_of(&src) {
            Some(src_origin) => {
                if origin_of(&dst) == Some(src_origin) {
                    RedirectVerdict::SameOrigin
                } else {
                    RedirectVerdict::CrossOrigin
                }
            }
            None => RedirectVerdict::Unresolvable,
        },
        _ => RedirectVerdict::Unresolvable,
    }
}

fn origin_of(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    let scheme = url.scheme();
    let normalized_port = url.port_or_known_default()?;
    Some(format!("{}://{}:{}", scheme, host, normalized_port))
}

/// 构造 fail-closed 重定向策略：仅同一 origin 的重定向跟随，跨域即 Stop。
///
/// 直接用于 `reqwest::ClientBuilder::redirect()`。`attempt.previous()[0]` 是初始请求
/// URL（凭据悬挂处），`attempt.url()` 是重定向目标——两者 origin 不同即 Stop，
/// 让 30x 响应直接返回而不带凭据跟随（yoyo #982 fail-closed：宁可中断也不泄漏）。
pub fn fail_closed_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let src = attempt
            .previous()
            .first()
            .map(|u| u.to_string())
            .unwrap_or_else(|| attempt.url().to_string());
        if redirect_verdict(&src, &attempt.url().to_string()).may_follow() {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

#[cfg(test)]
mod redirect_tests {
    use super::*;

    #[test]
    fn same_origin_same_scheme_host_allows() {
        assert_eq!(
            redirect_verdict(
                "https://ark.cn-beijing.volces.com/api/plan/v3/chat/completions",
                "https://ark.cn-beijing.volces.com/v1/chat/completions"
            ),
            RedirectVerdict::SameOrigin
        );
    }

    #[test]
    fn same_origin_default_port_normalization() {
        // 443 显式与 https 默认等价 → 同一 origin
        assert_eq!(
            redirect_verdict(
                "https://api.deepseek.com",
                "https://api.deepseek.com:443/chat"
            ),
            RedirectVerdict::SameOrigin
        );
    }

    #[test]
    fn cross_origin_host_change_denied() {
        assert_eq!(
            redirect_verdict(
                "https://ark.cn-beijing.volces.com/api/v3/chat",
                "https://evil.example.com/steal"
            ),
            RedirectVerdict::CrossOrigin
        );
    }

    #[test]
    fn cross_origin_scheme_change_denied() {
        // https → http 降级，凭据不得带过
        assert_eq!(
            redirect_verdict(
                "https://api.deepseek.com/chat",
                "http://api.deepseek.com/chat"
            ),
            RedirectVerdict::CrossOrigin
        );
    }

    #[test]
    fn cross_origin_port_change_denied() {
        assert_eq!(
            redirect_verdict(
                "https://api.deepseek.com/chat",
                "https://api.deepseek.com:8443/chat"
            ),
            RedirectVerdict::CrossOrigin
        );
    }

    #[test]
    fn unresolvable_is_denied() {
        // 相对路径/非法 URL 无 origin，保守按跨域
        assert_eq!(
            redirect_verdict("not-a-url", "https://api.deepseek.com"),
            RedirectVerdict::Unresolvable
        );
        assert!(!redirect_verdict("not-a-url", "https://api.deepseek.com").may_follow());
    }

    #[test]
    fn may_follow_only_same_origin() {
        let same = RedirectVerdict::SameOrigin;
        let cross = RedirectVerdict::CrossOrigin;
        let unres = RedirectVerdict::Unresolvable;
        assert!(same.may_follow());
        assert!(!cross.may_follow());
        assert!(!unres.may_follow());
    }

    // ---- 端到端：真实 client + fail_closed 策略，跨域重定向被拦（凭据不外泄） ----

    #[tokio::test]
    async fn e2e_cross_origin_redirect_is_stopped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::net::TcpListener;
        use tokio::task::JoinHandle;
        use axum::response::IntoResponse;

        // 跨 origin 目标：不同端口的本地服务（端口不同 = 不同 origin）
        let saw_auth = Arc::new(AtomicBool::new(false));
        let saw_closure = saw_auth.clone();
        let target_app = axum::Router::new().route(
            "/land",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let saw = saw_closure.clone();
                async move {
                    if headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .is_some()
                    {
                        saw.store(true, Ordering::SeqCst);
                    }
                    axum::response::Json(serde_json::json!({"ok": true}))
                }
            }),
        );
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target_listener.local_addr().unwrap().port();
        let _target_h: JoinHandle<()> = tokio::spawn(async move {
            let _ = axum::serve(target_listener, target_app).await;
        });

        // 源服务：/start 返回 302 指向跨 origin 的 /land
        let start_app = axum::Router::new().route(
            "/start",
            axum::routing::get(move || async move {
                (
                    axum::http::StatusCode::FOUND,
                    [("Location", format!("http://127.0.0.1:{}/land", target_port))],
                )
                    .into_response()
            }),
        );
        let start_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let start_port = start_listener.local_addr().unwrap().port();
        let _start_h: JoinHandle<()> = tokio::spawn(async move {
            let _ = axum::serve(start_listener, start_app).await;
        });

        // 等 server 就绪
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::builder()
            .redirect(fail_closed_redirect_policy())
            .build()
            .unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{}/start", start_port))
            .header("Authorization", "Bearer SECRET-KEY-123")
            .send()
            .await
            .unwrap();

        // 跨域重定向被 Stop → 返回 302 原始响应，客户端不强跟
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::FOUND,
            "cross-origin redirect must not be auto-followed"
        );
        // 目标服务器从未收到 Authorization（凭据未带过 origin 边界）
        assert!(
            !saw_auth.load(Ordering::SeqCst),
            "credential leaked to cross-origin host"
        );
    }

    #[tokio::test]
    async fn e2e_same_origin_redirect_is_followed() {
        use tokio::net::TcpListener;
        use tokio::task::JoinHandle;
        use axum::response::IntoResponse;

        // 单服务内 /start → /land 同 origin，凭据可安全跟随且目标收到
        let app = axum::Router::new()
            .route(
                "/start",
                axum::routing::get(|| async {
                    (axum::http::StatusCode::FOUND, [("Location", "/land")]).into_response()
                }),
            )
            .route(
                "/land",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    let has_auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .is_some();
                    axum::response::Json(serde_json::json!({"has_auth": has_auth}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _h: JoinHandle<()> = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::builder()
            .redirect(fail_closed_redirect_policy())
            .build()
            .unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{}/start", port))
            .header("Authorization", "Bearer SECRET-KEY-123")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        // 同 origin 跟随后，Authorization 保留传给目标（这是预期的合法路径）
        assert_eq!(body["has_auth"], serde_json::json!(true));
    }
}
