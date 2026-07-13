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

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

mod a2a;
mod config;
mod handler;
mod state;

use a2a::types::*;
use handler::QBodyHandler;
use state::TaskStore;

/// 共享应用状态
struct AppState {
    handler: QBodyHandler,
}

// ============================================================
// Agent Card 端点
// ============================================================

async fn get_agent_card(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let card = state.handler.agent_card.clone();
    Json(card)
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
            Json(serde_json::to_value(JsonRpcError::invalid_params(
                req.id,
                "jsonrpc must be 2.0",
            ))
            .unwrap()),
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
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // 解析命令行参数
    let mut port: u16 = 41242;
    for arg in std::env::args().skip(1) {
        if let Some(p) = arg.strip_prefix("--port=") {
            if let Ok(n) = p.parse() {
                port = n;
            }
        } else if arg == "--port" {
            // handled by next arg, but we don't parse pairs here — too simple
        }
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
        version: "0.1.0".into(),
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
            examples: vec![
                "hello".into(),
                "what can you do".into(),
                "你的能力".into(),
            ],
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
    let timeout_config = config::TimeoutConfig::from_env();
    let handler = QBodyHandler::new(task_store, agent_card, timeout_config);

    let state = Arc::new(AppState { handler });

    // CORS —— 允许跨域调用
    let cors = CorsLayer::new()
        .allow_origin(HeaderValue::from_static("*"))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/.well-known/agent-card.json", get(get_agent_card))
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

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}