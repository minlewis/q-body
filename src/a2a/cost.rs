//! 成本警告门控 — 借鉴 yoyo-evolve `--cost-warn <USD>` 标志门设计
//!
//! 成本阈值用环境变量门控（`QBODY_COST_WARN_USD`），单任务 LLM 花费超线时
//! 先在 journal 记一条 `cost_warn` 事件（警告先落账再考虑拦截，P0=journal 记忆优先）。
//! 超线只警告不硬拦——警告可观测、执行不阻塞。

use std::sync::Arc;
use tokio::sync::RwLock;

/// 默认警告阈值（美元，仅当显式设置 `QBODY_COST_WARN_USD` 时门控才开启）
// DEBT(unwired): 命名为"默认值"但无任何读取方 —— 门控未设置时是直接关闭，并不回退到本常量。
#[allow(dead_code)]
pub const DEFAULT_COST_WARN_USD: f64 = 0.50;

/// 读取成本警告阈值（环境变量门控）。未设置或非法（≤0 / 非数字）→ None（门控关闭）
pub fn cost_warn_threshold_usd() -> Option<f64> {
    match std::env::var("QBODY_COST_WARN_USD") {
        Ok(raw) => raw.trim().parse::<f64>().ok().filter(|v| *v > 0.0),
        Err(_) => None,
    }
}

/// journal 事件：成本警告（内存层，P0 先落账）
#[derive(Debug, Clone, PartialEq)]
pub struct CostWarnEvent {
    /// 事件时刻（RFC3339 UTC）
    pub at: String,
    /// 触发来源（如 task_id）
    pub source: String,
    /// 本次 LLM 花费估算（美元）
    pub cost_usd: f64,
    /// 当前阈值（美元）
    pub threshold_usd: f64,
}

/// 内存 journal：cost_warn 事件追加式记录
#[derive(Debug, Default, Clone)]
pub struct CostJournal {
    events: Arc<RwLock<Vec<CostWarnEvent>>>,
}

impl CostJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条 cost_warn 事件
    pub async fn record(&self, event: CostWarnEvent) {
        self.events.write().await.push(event);
    }

    /// 读取全部事件（审计用）
    // DEBT(unwired): 仅单测调用，运行时无审计读取方。
    #[allow(dead_code)]
    pub async fn events(&self) -> Vec<CostWarnEvent> {
        self.events.read().await.clone()
    }
}

/// 由 usage tokens 估算单次调用成本（美元）。
///
/// 定价模型：input $1.0 / 1M tokens，output $2.0 / 1M tokens
/// （deepseek 量级保守估计；估算只用于超线判定，不作为计费依据）。
pub fn estimate_cost_usd(input_tokens: u64, output_tokens: u64) -> f64 {
    input_tokens as f64 * 1.0 / 1_000_000.0 + output_tokens as f64 * 2.0 / 1_000_000.0
}

/// 判定是否超线并构造事件；未超线 / 门控关闭 → None
pub fn check_cost_warn(source: &str, cost_usd: f64, now_rfc3339: &str) -> Option<CostWarnEvent> {
    let threshold = cost_warn_threshold_usd()?;
    if cost_usd <= threshold {
        return None;
    }
    Some(CostWarnEvent {
        at: now_rfc3339.to_string(),
        source: source.to_string(),
        cost_usd,
        threshold_usd: threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// 串行化所有改写 `QBODY_COST_WARN_USD` 的测试。
    ///
    /// `cargo test` 默认**多线程并行**，而环境变量是进程级共享状态。
    /// 「在单条测试内顺序验证」只能防住测试*内部*的先后关系，
    /// 防不住两个测试*之间*交叉改写同一变量 —— 这正是本文件此前
    /// 并行必挂、`--test-threads=1` 必过的根因。
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        // 前一个测试 panic 会毒化锁；这里只用它做互斥，不保护数据不变式，
        // 故直接取回内部值而非继续传播毒化。
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_env(v: &str) {
        // SAFETY: 调用方持有 `env_lock()`，本测试二进制内对该变量的
        // 改写与读取均在同一把锁下串行，不存在并发访问。
        unsafe {
            std::env::set_var("QBODY_COST_WARN_USD", v);
        }
    }

    fn clear_env() {
        // SAFETY: 同 `set_env`
        unsafe {
            std::env::remove_var("QBODY_COST_WARN_USD");
        }
    }

    #[test]
    fn test_env_gate_states() {
        let _guard = env_lock();
        clear_env();
        assert_eq!(cost_warn_threshold_usd(), None);

        set_env("0.30");
        assert_eq!(cost_warn_threshold_usd(), Some(0.30));

        set_env("abc");
        assert_eq!(cost_warn_threshold_usd(), None);
        set_env("-1");
        assert_eq!(cost_warn_threshold_usd(), None);
        set_env("0");
        assert_eq!(cost_warn_threshold_usd(), None);

        clear_env();
        assert_eq!(cost_warn_threshold_usd(), None);
    }

    #[test]
    fn test_estimate_cost_basic() {
        // 1M input + 0 output = $1.0；0 + 0.5M output = $1.0
        assert!((estimate_cost_usd(1_000_000, 0) - 1.0).abs() < 1e-9);
        assert!((estimate_cost_usd(0, 500_000) - 1.0).abs() < 1e-9);
        assert!((estimate_cost_usd(1000, 2000) - 0.005).abs() < 1e-9);
    }

    #[test]
    fn test_check_warn_threshold_gate_states() {
        let _guard = env_lock();
        clear_env();
        assert!(check_cost_warn("t", 999.0, "x").is_none()); // 门控关闭不触发

        set_env("0.50");
        assert!(check_cost_warn("t1", 0.49, "2026-09-10T00:00:00Z").is_none());
        assert!(check_cost_warn("t1", 0.50, "2026-09-10T00:00:00Z").is_none()); // 等于阈值不触发

        let ev = check_cost_warn("task-42", 0.75, "2026-09-10T00:00:00Z").unwrap();
        assert_eq!(ev.source, "task-42");
        assert!((ev.cost_usd - 0.75).abs() < 1e-9);
        assert!((ev.threshold_usd - 0.50).abs() < 1e-9);
        assert_eq!(ev.at, "2026-09-10T00:00:00Z");

        clear_env();
    }

    #[tokio::test]
    async fn test_journal_roundtrip() {
        let journal = CostJournal::new();
        assert!(journal.events().await.is_empty());
        journal
            .record(CostWarnEvent {
                at: "2026-09-10T00:00:00Z".into(),
                source: "t1".into(),
                cost_usd: 1.0,
                threshold_usd: 0.5,
            })
            .await;
        let evs = journal.events().await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].source, "t1");
    }

    #[tokio::test]
    async fn test_journal_append_order() {
        let journal = CostJournal::new();
        for i in 0..3 {
            journal
                .record(CostWarnEvent {
                    at: format!("t{i}"),
                    source: format!("s{i}"),
                    cost_usd: 1.0,
                    threshold_usd: 0.5,
                })
                .await;
        }
        let evs = journal.events().await;
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].source, "s0");
        assert_eq!(evs[2].source, "s2");
    }
}
