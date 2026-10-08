//! q-body A2A 服务端
//!
//! 启动命令：
//!     source "$HOME/.cargo/env"
//!     cargo run --release -- [--port 41242]
//!
//! 端点：
//!     GET  /.well-known/agent-card.json   — Agent Card 发现
//!     POST /a2a/jsonrpc                   — JSON-RPC 端点
//!
//! A2A 方法支持：
//!     SendMessage  — 发送消息，创建 Task，等待结果
//!     GetTask      — 查询 Task 状态与历史
//!     ListTasks    — 列出所有 Task

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

mod a2a;
mod artifact;
mod clamp;
mod darkroom;
mod evolution_gate;
mod gate_audit;
mod handler;
mod health;
mod queue;
pub mod reflect;
mod state;
mod tools;
mod trust_input;
mod usage;
mod validator;
mod webui;

// ============================================================
// standalone 纯函数模块（learnings / clamp / darkroom 同先例）：
// cron D 养料回灌产生的独立功能模块，main 上无对应生产接线，
// 以单测形式维护，待集成主线合入后一行接线。
// ============================================================
mod backlog;
mod journal;

use a2a::types::*;
use handler::QBodyHandler;
use state::TaskStore;

/// 共享应用状态
struct AppState {
    handler: QBodyHandler,
    /// 进程启动时刻（/health uptime 自证用）
    started_at: std::time::SystemTime,
}

// ============================================================
// Web UI — 浏览器聊天页面（SSH 隧道使用；服务只绑 127.0.0.1）
// ============================================================

async fn get_index() -> impl IntoResponse {
    Html(webui::CHAT_HTML)
}

// ============================================================
// Agent Card 端点
// ============================================================

async fn get_agent_card(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let card = state.handler.agent_card.clone();
    Json(card)
}

// ============================================================
// /health 端点 — 健康判定自带测量证据（借鉴 yoyo-evolve #915：
// verdict 必须内嵌测量依据，UNVERIFIED 不升格）
// ============================================================

async fn get_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use health::{
        EvidenceSource, build_report, exe_mtime_evidence, journal_freshness_evidence,
        journal_line_count_evidence,
    };

    let started = state.started_at;
    let uptime = health::uptime_secs(std::time::SystemTime::now(), started);

    let exe_src = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from));
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let exe_ev = exe_mtime_evidence(exe_src.as_deref(), now_secs);
    let journal_ev =
        journal_freshness_evidence(std::env::var("QBODY_JOURNAL_PATH").ok().as_deref());
    // 证据 3：pump 统计锚定 JSONL 落盘行数（yoyo evt-0028：file reads 替代自报计数）
    let journal_lc_ev =
        journal_line_count_evidence(std::env::var("QBODY_JOURNAL_PATH").ok().as_deref());

    let report = build_report(uptime, &[exe_ev, journal_ev, journal_lc_ev]);
    Json(report)
}

// ============================================================
// JSON-RPC 端点
// ============================================================

async fn jsonrpc_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // 验证 jsonrpc 版本
    if req.jsonrpc != "2.0" {
        return (
            StatusCode::OK,
            Json(
                serde_json::to_value(JsonRpcError::invalid_params(req.id, "jsonrpc must be 2.0"))
                    .unwrap(),
            ),
        );
    }

    let id = req.id;
    let method = req.method;
    let params = req.params;

    // A2A 方法名归一化：支持 "/" 分隔和 PascalCase 两种形式
    let normalized = method.trim();
    let result = state.handler.handle_request(normalized, params, id).await;

    (StatusCode::OK, Json(result))
}

// ============================================================
// 启动
// ============================================================

#[tokio::main]
async fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // 结构化分类（#982 同款）：未知/非法参数不再静默吞掉——每次出现都产生
    // 分类条目，Unknown/InvalidValue 类错误直接非零退出并打结构化 error body
    // （golden 字节级钉死），杜绝「非法输入静默回退默认端口」的自 concealing。
    let arg_strings: Vec<String> = std::env::args().skip(1).collect();
    let (port, outcomes) = usage::resolve_port(&arg_strings);
    if outcomes
        .iter()
        .any(|o| !matches!(o, usage::PortOutcome::Parsed(_)))
    {
        let err = usage::cli_error(&arg_strings, &outcomes);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&err)
                .unwrap_or_else(|_| format!("{{\"error\":\"{}\",\"exit_code\":2}}", err.error))
        );
        std::process::exit(err.exit_code);
    }

    // 构建 Agent Card
    let agent_card = AgentCard {
        name: "q-body".into(),
        description: "Q宝宝的自进化身体 — 一个正在学习、实验、进化的 AI Agent (Rust)".into(),
        url: Some(format!("http://127.0.0.1:{port}")),
        provider: Some(AgentProvider {
            organization: "Q宝宝实验室".into(),
            url: "https://github.com/q-baby".into(),
        }),
        // 单一事实源：编译期从 Cargo.toml 取版本 + build.rs 注入 git 短 commit
        //（yoyo Day219 Task 2 同款：版本自描述且不可漂移）
        version: usage::version_string(),
        capabilities: Some(AgentCapabilities {
            streaming: false,
            push_notifications: false,
        }),
        default_input_modes: vec!["text".into()],
        default_output_modes: vec!["text".into()],
        skills: vec![AgentSkill {
            id: "q-body-core".into(),
            name: "q-body Core".into(),
            description: "核心 A2A 通信能力，用于验证 agent 间协作链路".into(),
            tags: vec!["a2a".into(), "core".into(), "evolution".into()],
            examples: vec!["hello".into(), "what can you do".into(), "你的能力".into()],
            input_modes: vec!["text".into()],
            output_modes: vec!["text".into()],
        }],
        supported_interfaces: vec![AgentInterface {
            protocol_binding: "JSONRPC".into(),
            protocol_version: "1.0".into(),
            url: format!("http://127.0.0.1:{port}/a2a/jsonrpc"),
        }],
    };

    let task_store = TaskStore::new();
    let handler = QBodyHandler::new(task_store, agent_card);

    // Journal 启动加载：QBODY_JOURNAL_PATH 存在则 load（记忆泵接上心跳），
    // 缺省不落盘 —— 探针 Unconfigured 语义保持向后兼容。
    // Resume-strict gate：journal 可读性校验在任何请求处理之前执行——
    // 损坏即退出报错，绝不静默降级为空 journal 带病续跑。
    if let Ok(p) = std::env::var("QBODY_JOURNAL_PATH") {
        if let Err(reason) = journal::Journal::verify_resume_strict(&p) {
            tracing::error!("resume-strict gate: {reason}");
            eprintln!("resume-strict gate: {reason}");
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&usage::missing_resource_error(&reason))
                    .unwrap_or_else(|_| "{\"error\":\"missing or unusable resource\",\"exit_code\":1}".into())
            );
            std::process::exit(usage::EXIT_MISSING_RESOURCE);
        }
        let j = journal::Journal::load_from_jsonl(&p).unwrap_or_default();
        *handler.journal.write().await = j;
        tracing::info!("journal loaded from {}", p);
    }

    let state = Arc::new(AppState {
        handler,
        started_at: std::time::SystemTime::now(),
    });

    // CORS —— 允许跨域调用
    let cors = CorsLayer::new()
        .allow_origin(HeaderValue::from_static("*"))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/", get(get_index))
        .route("/.well-known/agent-card.json", get(get_agent_card))
        .route("/health", get(get_health))
        .route("/a2a/jsonrpc", post(jsonrpc_handler))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
        port,
    );

    let sep: String = std::iter::repeat('=').take(50).collect();
    tracing::info!("{sep}");
    tracing::info!("🚀 q-body A2A Server (Rust) starting...");
    tracing::info!("   Agent Card: http://{addr}/.well-known/agent-card.json");
    tracing::info!("   JSON-RPC:   http://{addr}/a2a/jsonrpc");
    tracing::info!("{sep}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind to {addr}: {e}");
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&usage::internal_error(&format!("bind {addr}: {e}")))
                    .unwrap_or_else(|_| "{\"error\":\"internal error\",\"exit_code\":70}".into())
            );
            std::process::exit(usage::EXIT_INTERNAL);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        // 检查是否是 IO 错误（broken-pipe / connection-reset / connection-aborted）
        // 这类错误在客户端断开连接时常见，不应 panic 退出
        if let Some(io_err) = e.source().and_then(|s| s.downcast_ref::<std::io::Error>()) {
            match io_err.kind() {
                std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted => {
                    tracing::warn!(
                        "Server connection closed (IO error: {}), shutting down gracefully",
                        io_err
                    );
                    std::process::exit(0);
                }
                _ => {
                    tracing::error!("Unexpected IO error during serve: {io_err}");
                    eprintln!(
                        "{}",
                        serde_json::to_string_pretty(&usage::internal_error(&io_err.to_string()))
                            .unwrap_or_else(|_| "{\"error\":\"internal error\",\"exit_code\":70}".into())
                    );
                    std::process::exit(usage::EXIT_INTERNAL);
                }
            }
        } else {
            // 非 IO 错误（如 hyper 内部错误）— 仍应 panic 报告
            tracing::error!("Server error during serve: {e}");
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&usage::internal_error(&e.to_string()))
                    .unwrap_or_else(|_| "{\"error\":\"internal error\",\"exit_code\":70}".into())
            );
            std::process::exit(usage::EXIT_INTERNAL);
        }
    }
}
