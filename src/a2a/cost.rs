//! 成本警告门控 — 借鉴 yoyo-evolve `--cost-warn <USD>` 标志门设计
//!
//! 成本阈值用环境变量门控（`QBODY_COST_WARN_USD`），单任务 LLM 花费超线时
//! 先在 journal 记一条 `cost_warn` 事件（警告先落账再考虑拦截，P0=journal 记忆优先）。
//! 超线只警告不硬拦——警告可观测、执行不阻塞。
//!
//! ## 负债可视化到子代理粒度（借鉴 yoyo-evolve Fixes #1108）
//! 每个 `CostWarnEvent` 的 `source`（task_id）即一次 LLM 调用的归属方——主会话
//! 或委托出的子代理（sub_agent）共享同一 `CostJournal`，按 `source` 聚合即可把
//! 成本负债透视到单次调用/单个子代理粒度，而不是只有一个整体数字。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostWarnEvent {
    /// 事件时刻（RFC3339 UTC）
    pub at: String,
    /// 触发来源（如 task_id；主会话或委托出的子代理共享同一 source 命名空间）
    pub source: String,
    /// 本次 LLM 花费估算（美元）
    pub cost_usd: f64,
    /// 当前阈值（美元）
    pub threshold_usd: f64,
}

/// 单一成本归属方（source=task_id）的聚合开销，供 `/cost` 类端点按子代理粒度透视
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCostBreakdown {
    /// 归属方标识（即 CostWarnEvent.source：task_id）
    pub source: String,
    /// 该归属方触发的成本警告次数
    pub warn_count: u64,
    /// 该归属方累计估算花费（美元）
    pub total_cost_usd: f64,
    /// 最后一次触发时刻（RFC3339 UTC）；一次都未触发则空串
    pub last_at: String,
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

    /// 按归属方（source=task_id）聚合成本开销，透视到主会话/子代理粒度
    ///
    /// 借鉴 yoyo-evolve Fixes #1108（"delegate overhead visible to sub-agent
    /// granularity"）：负债不只给一个整体数字，而是按每个 source 分别累计
    /// 警告次数 + 估算成本 + 最后触发时刻。空 journal → 空列表，不假造一条
    /// 无意义的合计行（未测量即不得宣称，health.rs 三态同款纪律）。
    pub async fn breakdown_by_source(&self) -> Vec<SourceCostBreakdown> {
        let events = self.events.read().await;
        // BTreeMap<source, (warn_count, total, last_at)> — 键有序，输出确定可测试
        let mut acc: BTreeMap<String, (u64, f64, String)> = BTreeMap::new();
        for ev in events.iter() {
            let entry = acc
                .entry(ev.source.clone())
                .or_insert((0, 0.0, String::new()));
            entry.0 += 1;
            entry.1 += ev.cost_usd;
            // 保留最晚 at（RFC3339 字典序即时间序）
            if ev.at > entry.2 {
                entry.2 = ev.at.clone();
            }
        }
        acc.into_iter()
            .map(|(source, (warn_count, total_cost_usd, last_at))| SourceCostBreakdown {
                source,
                warn_count,
                total_cost_usd,
                last_at,
            })
            .collect()
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

    #[tokio::test]
    async fn test_breakdown_empty_journal_is_empty() {
        // 未测量即不宣称：空 journal → 空列表，不假造合计行（health.rs 三态纪律）
        let journal = CostJournal::new();
        assert!(journal.breakdown_by_source().await.is_empty());
    }

    #[tokio::test]
    async fn test_breakdown_aggregates_per_source_subagent_granularity() {
        // 主会话 task "main" 触发 2 次，委托出的子代理 "delegate-1"/"delegate-2"
        // 各 1 次——负债必须透视到子代理粒度而非只给整体数字（yoyo #1108）
        let journal = CostJournal::new();
        for (at, source, cost) in [
            ("2026-11-02T10:00:00Z", "main", 0.75),
            ("2026-11-02T10:01:00Z", "delegate-1", 1.20),
            ("2026-11-02T10:02:00Z", "main", 0.60),
            ("2026-11-02T10:03:00Z", "delegate-2", 0.90),
        ] {
            journal
                .record(CostWarnEvent {
                    at: at.into(),
                    source: source.into(),
                    cost_usd: cost,
                    threshold_usd: 0.5,
                })
                .await;
        }

        let mut bd = journal.breakdown_by_source().await;
        // BTreeMap 键序：delegate-1 < delegate-2 < main
        assert_eq!(bd.len(), 3);

        let d1 = bd.remove(0);
        assert_eq!(d1.source, "delegate-1");
        assert_eq!(d1.warn_count, 1);
        assert!((d1.total_cost_usd - 1.20).abs() < 1e-9);
        assert_eq!(d1.last_at, "2026-11-02T10:01:00Z");

        let d2 = bd.remove(0);
        assert_eq!(d2.source, "delegate-2");
        assert_eq!(d2.warn_count, 1);
        assert!((d2.total_cost_usd - 0.90).abs() < 1e-9);
        assert_eq!(d2.last_at, "2026-11-02T10:03:00Z");

        let m = bd.remove(0);
        assert_eq!(m.source, "main");
        assert_eq!(m.warn_count, 2); // 同一归属方多次触发累计
        assert!((m.total_cost_usd - 1.35).abs() < 1e-9);
        assert_eq!(m.last_at, "2026-11-02T10:02:00Z"); // 保留最晚时刻（非最早）
    }

    #[tokio::test]
    async fn test_breakdown_serialized_as_json() {
        // SourceCostBreakdown 须可 JSON 序列化——/cost 端点直接回 this 结构
        let journal = CostJournal::new();
        journal
            .record(CostWarnEvent {
                at: "2026-11-02T10:00:00Z".into(),
                source: "delegate-1".into(),
                cost_usd: 1.20,
                threshold_usd: 0.5,
            })
            .await;
        let bd = journal.breakdown_by_source().await;
        let json = serde_json::to_value(&bd[0]).unwrap();
        assert_eq!(json["source"], "delegate-1");
        assert_eq!(json["warn_count"], 1);
        assert!((json["total_cost_usd"].as_f64().unwrap() - 1.20).abs() < 1e-9);
        assert_eq!(json["last_at"], "2026-11-02T10:00:00Z");
    }
}
