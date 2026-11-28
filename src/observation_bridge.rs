//! src/observation_bridge.rs — 自检观测项 → backlog P0 落账通道（纯函数桥接）
//!
//! 借鉴：minlewis/reme-multi-agent — 账本与执行器解耦需显式桥接 → q-body 建
//! 「自检观测项 → backlog P0」落账通道。
//!
//! 问题（11-13 对镜点名 5 天的根因）：对镜 / 自检发现的 P0（如 state.db 膨胀）
//! 只落在对镜报告里，**不在 Cron D 的 backlog 读取域**——Cron D 每晚取
//! ~/.hermes/q-body-backlog.md 里的 PENDING 条目，观测项若不显式织成 backlog
//! 条目，就结构性漏执行。
//!
//! 本模块把自检观测到的可量化指标（名称/当前值/日增量/来源/借鉴来源）
//! 格式化成标准 backlog P0 条目 markdown（含「来源 / 状态：PENDING / 借鉴」
//! 三段），让 06:00 自检可机械地把观测项织成 Cron D 可消费的条目，
//! 不再靠手工回灌。
//!
//! standalone 先例（08-05 learnings.rs / 09-02 clamp.rs / 09-04 darkroom.rs）：
//! 纯函数 + 单测先行落 standalone 模块；真实 self-check 观测入口接线
//! （把观测值喂给本模块再写 backlog）合入后一行接线。

use chrono::NaiveDate;

/// 默认阈值：当日增量超过该值（bytes）即判定为「需落账预警」
pub const DEFAULT_GROWTH_WARN_BYTES: u64 = 1 << 20; // 1 MiB/d

/// 一个可量化的自检观测指标
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// 指标名（如 "state.db size"）
    pub name: String,
    /// 当前测量值
    pub current: u64,
    /// 相对前次测量的增量（bytes / 条目）
    pub delta: i64,
    /// 观测来源（如 grill_daily_mirror_20261112.md）
    pub source: String,
    /// 借鉴来源（owner/repo — 思路）
    pub borrowed: String,
}

impl Observation {
    pub fn new(name: impl Into<String>, current: u64, delta: i64) -> Self {
        Self {
            name: name.into(),
            current,
            delta,
            source: String::new(),
            borrowed: String::new(),
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn with_borrowed(mut self, borrowed: impl Into<String>) -> Self {
        self.borrowed = borrowed.into();
        self
    }

    /// 是否达到需落账预警阈值（增量超过默认上限）
    pub fn is_warning(&self) -> bool {
        self.delta >= DEFAULT_GROWTH_WARN_BYTES as i64
    }
}

/// 把观测值格式化成 Cron D 可消费的标准 backlog P0 条目 markdown。
///
/// 产出形如：
/// ```text
/// ### 2026-11-13 — P0
///
/// **来源**：grill_daily_mirror_20261112.md（对镜点名第 5 天，机制断链根因已证
/// ——对镜 P0 不在 Cron D 读取域）
///
/// **状态**：PENDING
///
/// P0: state.db 膨胀监控落账 — 当前 710.4 MiB / 日增量 +2.5 MiB/d —— 借鉴：
/// minlewis/reme-multi-agent — 账本与执行器解耦需显式桥接 → q-body 建「自检观测项
/// →backlog」落账通道
///
/// **借鉴**: <借鉴来源>
/// ```
pub fn render_p0_entry(obs: &Observation, date: NaiveDate) -> String {
    let current_mib = obs.current as f64 / (1 << 20) as f64;
    let delta_mib = obs.delta as f64 / (1 << 20) as f64;
    let source_line = if obs.source.is_empty() {
        "自检观测".to_string()
    } else {
        obs.source.clone()
    };
    let borrowed_line = if obs.borrowed.is_empty() {
        "（待回灌时补充）".to_string()
    } else {
        obs.borrowed.clone()
    };

    format!(
        "### {date} — P0\n\n**来源**：{source}\n\n**状态**：PENDING\n\n\
         P0: {name} 落账 — 当前 {cur:.1} MiB / 日增量 {delta:+.1} MiB/d —— 借鉴：{borrowed_first}\n\n\
         **借鉴**: {borrowed}",
        date = date.format("%Y-%m-%d"),
        source = source_line,
        name = obs.name,
        cur = current_mib,
        delta = delta_mib,
        borrowed_first = obs
            .borrowed
            .split(" → ")
            .next()
            .unwrap_or(&borrowed_line)
            .to_string(),
        borrowed = borrowed_line,
    )
}

/// 把观测项织成一条 P0 待办标题（供 issue / PR 标题复用）
pub fn p0_title(obs: &Observation) -> String {
    format!("自检观测项落账通道：{}", obs.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn render_produces_pending_entry() {
        let obs = Observation::new("state.db size", 744_000_000, 2_600_000)
            .with_source("grill_daily_mirror_20261112.md")
            .with_borrowed("minlewis/reme-multi-agent — 账本与执行器解耦需显式桥接");
        let out = render_p0_entry(&obs, d(2026, 11, 13));
        assert!(out.contains("### 2026-11-13 — P0"));
        assert!(out.contains("**状态**：PENDING"));
        assert!(out.contains("src/observation_bridge") == false);
        assert!(out.contains("state.db size 落账"));
        assert!(out.contains("709.5 MiB"));
    }

    #[test]
    fn warning_threshold_triggers_on_high_growth() {
        // 1 MiB = 1048576；2.48 MiB/d 增量 → warning
        let obs = Observation::new("state.db size", 744_000_000, 2_600_000);
        assert!(obs.is_warning());
        // 小增量 → 不预警
        let calm = Observation::new("state.db size", 744_000_000, 10_000);
        assert!(!calm.is_warning());
    }

    #[test]
    fn empty_source_borrowed_fall_back() {
        let obs = Observation::new("metric", 100, 5);
        let out = render_p0_entry(&obs, d(2026, 11, 13));
        assert!(out.contains("**来源**：自检观测"));
        assert!(out.contains("（待回灌时补充）"));
    }

    #[test]
    fn is_warning_uses_default_byte_threshold() {
        // 恰好等于阈值 → 触发（>=）
        let at = Observation::new("m", 100, DEFAULT_GROWTH_WARN_BYTES as i64);
        assert!(at.is_warning());
        // 差 1 → 不触发
        let below = Observation::new("m", 100, (DEFAULT_GROWTH_WARN_BYTES - 1) as i64);
        assert!(!below.is_warning());
    }

    #[test]
    fn zero_bytes_renders_zero_mib() {
        let obs = Observation::new("metric", 0, 0);
        let out = render_p0_entry(&obs, d(2026, 11, 13));
        assert!(out.contains("0.0 MiB"));
    }
}