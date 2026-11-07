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

/// 定价基准（美元 / 1M tokens）— deepseek-v4-Flash 峰值 cache-miss 保守档，
/// 对齐官方价目表 2026-08-16 重定价（peak：input $0.44 / output $1.32；
/// off-peak 减半 $0.22 / $0.66，此处取峰值保守估计）。
/// 可用环境变量 `QBODY_COST_INPUT_USD_PER_MTOK` / `QBODY_COST_OUTPUT_USD_PER_MTOK`
/// 覆盖，非法值（≤0 / 非数字）回落默认。
pub const DEFAULT_INPUT_USD_PER_MTOK: f64 = 0.44;
pub const DEFAULT_OUTPUT_USD_PER_MTOK: f64 = 1.32;

/// 读取单价：env 合法值优先，非法（≤0 / 非数字）或未设置回落默认
fn price_per_mtok(env_key: &str, default_usd: f64) -> f64 {
    match std::env::var(env_key) {
        Ok(raw) => raw
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v > 0.0)
            .unwrap_or(default_usd),
        Err(_) => default_usd,
    }
}

/// input 单价（美元 / 1M tokens），env `QBODY_COST_INPUT_USD_PER_MTOK` 可覆盖
pub fn input_price_usd_per_mtok() -> f64 {
    price_per_mtok("QBODY_COST_INPUT_USD_PER_MTOK", DEFAULT_INPUT_USD_PER_MTOK)
}

/// output 单价（美元 / 1M tokens），env `QBODY_COST_OUTPUT_USD_PER_MTOK` 可覆盖
pub fn output_price_usd_per_mtok() -> f64 {
    price_per_mtok(
        "QBODY_COST_OUTPUT_USD_PER_MTOK",
        DEFAULT_OUTPUT_USD_PER_MTOK,
    )
}

/// 由 usage tokens 估算单次调用成本（美元）。
///
/// 定价基准见 [`DEFAULT_INPUT_USD_PER_MTOK`]（估算只用于超线判定，不作为计费依据）。
pub fn estimate_cost_usd(input_tokens: u64, output_tokens: u64) -> f64 {
    input_tokens as f64 * input_price_usd_per_mtok() / 1_000_000.0
        + output_tokens as f64 * output_price_usd_per_mtok() / 1_000_000.0
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
        let _g = crate::test_env_lock::env_lock_guard();
        // SAFETY: 持全局 env 锁 + 单线程测试进程内修改自身进程环境变量
        unsafe {
            std::env::set_var("QBODY_COST_WARN_USD", v);
        }
    }

    fn clear_env() {
        let _g = crate::test_env_lock::env_lock_guard();
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
    }

    #[test]
    fn test_estimate_cost_basic() {
        // 默认基准：1M input = $0.44；1M output = $1.32；混合按比例
        assert!((estimate_cost_usd(1_000_000, 0) - 0.44).abs() < 1e-9);
        assert!((estimate_cost_usd(0, 1_000_000) - 1.32).abs() < 1e-9);
        assert!((estimate_cost_usd(500_000, 250_000) - 0.22 - 0.33).abs() < 1e-9);
    }

    #[test]
    fn test_price_env_override_states() {
        // 单条测试内顺序验证全部 env 覆盖状态，避免并行测试互相改同一变量
        unsafe {
            std::env::remove_var("QBODY_COST_INPUT_USD_PER_MTOK");
            std::env::remove_var("QBODY_COST_OUTPUT_USD_PER_MTOK");
        }
        assert_eq!(input_price_usd_per_mtok(), 0.44);
        assert_eq!(output_price_usd_per_mtok(), 1.32);

        unsafe {
            std::env::set_var("QBODY_COST_INPUT_USD_PER_MTOK", "0.10");
            std::env::set_var("QBODY_COST_OUTPUT_USD_PER_MTOK", "0.20");
        }
        assert_eq!(input_price_usd_per_mtok(), 0.10);
        assert_eq!(output_price_usd_per_mtok(), 0.20);
        assert!((estimate_cost_usd(1_000_000, 1_000_000) - 0.30).abs() < 1e-9);

        // 非法值回落默认
        unsafe {
            std::env::set_var("QBODY_COST_INPUT_USD_PER_MTOK", "abc");
            std::env::set_var("QBODY_COST_OUTPUT_USD_PER_MTOK", "-1");
        }
        assert_eq!(input_price_usd_per_mtok(), 0.44);
        assert_eq!(output_price_usd_per_mtok(), 1.32);

        unsafe {
            std::env::remove_var("QBODY_COST_INPUT_USD_PER_MTOK");
            std::env::remove_var("QBODY_COST_OUTPUT_USD_PER_MTOK");
        }
        assert_eq!(input_price_usd_per_mtok(), 0.44);
        assert_eq!(output_price_usd_per_mtok(), 1.32);
    }

    #[test]
    fn test_check_warn_threshold_gate_states() {
        // 单条测试内顺序验证，避免并行测试互相改同一变量
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
