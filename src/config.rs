//! 超时配置 — q-body 的边界防护
//!
//! 定义 TimeoutConfig 结构体，管理 HTTP 请求和 Task 生命周期超时。
//! 借鉴: yologdev/yoyo-evolve Day 131 timeout guardrails
//!
//! yoyo-evolve 在 Day 131 为其 LLM 调用和 task 执行增加了显式 timeout 层：
//! LLM_TIMEOUT=60s + TASK_TIMEOUT=300s，在 HTTP 客户端构造时传入 timeout()，
//! 在 task 调度层用 tokio::time::timeout 包裹全生命周期。超时后优雅降级
//! （记录日志返回错误响应标记 task 为 failed），而非静默挂起。
//!
//! 当前层只做类型准备：定义 TimeoutConfig + 接入 HTTP 客户端。
//! Task wall-clock timeout 接线（tokio::time::timeout 包裹 handle_send_message）
//! 属架构级改动，按 06-14/06-15/06-20/06-21 既定先例推迟。

use std::time::Duration;

/// 超时配置
///
/// 提供 HTTP 请求和 Task 生命周期的超时边界。
/// 默认值: http_timeout = 60s, task_timeout = 300s
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    /// 对外 HTTP 请求超时（如 LLM API 调用）
    pub http_timeout: Duration,
    /// 单个 Task 最大执行时间（wall-clock）
    pub task_timeout: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            http_timeout: Duration::from_secs(60),
            task_timeout: Duration::from_secs(300),
        }
    }
}

impl TimeoutConfig {
    /// 创建新的超时配置
    pub fn new(http_timeout: Duration, task_timeout: Duration) -> Self {
        Self {
            http_timeout,
            task_timeout,
        }
    }

    /// 从环境变量构建超时配置（可选）
    ///
    /// 环境变量:
    /// - Q_BODY_HTTP_TIMEOUT_SECS (默认 60)
    /// - Q_BODY_TASK_TIMEOUT_SECS (默认 300)
    pub fn from_env() -> Self {
        let http_secs = std::env::var("Q_BODY_HTTP_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        let task_secs = std::env::var("Q_BODY_TASK_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        Self {
            http_timeout: Duration::from_secs(http_secs),
            task_timeout: Duration::from_secs(task_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_config_default() {
        let cfg = TimeoutConfig::default();
        assert_eq!(cfg.http_timeout, Duration::from_secs(60));
        assert_eq!(cfg.task_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_timeout_config_custom() {
        let cfg = TimeoutConfig::new(Duration::from_secs(30), Duration::from_secs(120));
        assert_eq!(cfg.http_timeout, Duration::from_secs(30));
        assert_eq!(cfg.task_timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_timeout_config_from_env_default() {
        // 未设置环境变量时返回默认值
        let cfg = TimeoutConfig::from_env();
        assert_eq!(cfg.http_timeout, Duration::from_secs(60));
        assert_eq!(cfg.task_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_timeout_config_from_env_custom() {
        // 临时设置环境变量
        unsafe {
            std::env::set_var("Q_BODY_HTTP_TIMEOUT_SECS", "10");
            std::env::set_var("Q_BODY_TASK_TIMEOUT_SECS", "45");
        }
        let cfg = TimeoutConfig::from_env();
        assert_eq!(cfg.http_timeout, Duration::from_secs(10));
        assert_eq!(cfg.task_timeout, Duration::from_secs(45));
        // 清理环境变量
        unsafe {
            std::env::remove_var("Q_BODY_HTTP_TIMEOUT_SECS");
            std::env::remove_var("Q_BODY_TASK_TIMEOUT_SECS");
        }
    }
}