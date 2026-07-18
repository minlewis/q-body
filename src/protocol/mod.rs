//! Protocol 注册与分发 — q-body 的 A2A/MCP 协议网关层
//!
//! 借鉴：IBM/mcp-context-forge — 统一网关将 MCP/A2A/REST/gRPC
//! 聚合为统一 endpoint，带集中发现和 guardrails。
//!
//! 当前层是类型准备：定义 ProtocolKind 枚举 + ProtocolRegistry 注册表，
//! 运行时 MCP handler 接线按既定先例推迟。

pub mod registry;

pub use registry::*;