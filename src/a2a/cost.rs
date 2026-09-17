//! 成本警告门控 — 借鉴 yoyo-evolve `--cost-warn <USD>` 标志门设计
//!
//! 成本阈值用环境变量门控（`QBODY_COST_WARN_USD`），单任务 LLM 花费超线时
//! 先在 journal 记一条 `cost_warn` 事件（警告先落账再考虑拦截，P0=journal 记忆优先）。
//! 超线只警告不硬拦——警告可观测、执行不阻塞。

use std::sync::Arc;
use tokio::sync::RwLock;

/// 默认警告阈值（美元，仅当显式设置 `QBODY_COST_WARN_USD` 时门控才开启）
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

    fn set_env(v: &str) {
        // SAFETY: 单线程测试进程内修改自身进程环境变量，无并发读取
        unsafe {
            std::env::set_var("QBODY_COST_WARN_USD", v);
        }
    }

    fn clear_env() {
        // SAFETY: 同上
        unsafe {
            std::env::remove_var("QBODY_COST_WARN_USD");
        }
    }

    #[test]
    fn test_env_gate_states() {
        // 单条测试内顺序验证全部 env 门控状态，避免并行测试互相改同一变量
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

        // 估算纯函数（无 env 依赖，可内联在单条测试内）
        // 1M input + 0 output = $1.0；0 + 0.5M output = $1.0
        assert!((estimate_cost_usd(1_000_000, 0) - 1.0).abs() < 1e-9);
        assert!((estimate_cost_usd(0, 500_000) - 1.0).abs() < 1e-9);
        assert!((estimate_cost_usd(1000, 2000) - 0.005).abs() < 1e-9);

        // 门控状态同样单条内顺序验证 — 09-10 惯例：同键 env 变异的测试并行时互踩
        // （历史教训：本条曾拆成独立测试，与上面 set/clear 竞态导致阈值读成 None，
        //  2026-09-17 全量回归抓到 unwrap panic — §22 修复不带复验钩子会回退）
        set_env("0.50");
        assert!(check_cost_warn("t1", 0.49, "2026-09-10T00:00:00Z").is_none());
        assert!(check_cost_warn("t1", 0.50, "2026-09-10T00:00:00Z").is_none()); // 等于阈值不触发

        let ev = check_cost_warn("task-42", 0.75, "2026-09-10T00:00:00Z").unwrap();
        assert_eq!(ev.source, "task-42");
        assert!((ev.cost_usd - 0.75).abs() < 1e-9);
        assert!((ev.threshold_usd - 0.50).abs() < 1e-9);
        assert_eq!(ev.at, "2026-09-10T00:00:00Z");

        // 门控关闭时不触发
        clear_env();
        assert!(check_cost_warn("t", 999.0, "x").is_none());
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
