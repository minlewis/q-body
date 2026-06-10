//! q-body — Q宝宝的自进化 Rust 身体
//!
//! 本 crate 同时作为 binary 和 library 使用：
//! - **binary**: `src/main.rs` — A2A HTTP server 入口
//! - **library**: 暴露 `config` 和 `journal` 模块供集成测试使用
//!
//! ## 模块
//!
//! - [`a2a`] — A2A 协议核心类型（AgentCard / Task / Message 等）
//! - [`config`] — 配置管理（从 config.toml 加载）
//! - [`journal`] — Journal 会话记录与学习提取系统
//! - [`state`] — Task 存储（Arc<RwLock<HashMap>>）
//!
//! 注意：`handler` 模块不导出 — 它依赖外部 HTTP server 状态，仅 binary 使用。

pub mod a2a;
pub mod config;
pub mod journal;
pub mod state;