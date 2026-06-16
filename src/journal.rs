//! Journal — q-body 每日进化信号记录
//!
//! 用于记录供给侧养料回灌闭环中的各类进化事件。
//! 每个事件携带：时间戳、信号类型、来源、建议。
//!
//! 借鉴来源：yologdev/yoyo-evolve — Day 104 的 journal + skill-evolve counter 设计

use chrono::{DateTime, Utc};

/// 进化信号类型
#[derive(Debug, Clone, PartialEq)]
pub enum EvolutionSignal {
    /// 重构信号
    Refactor,
    /// 去重信号
    Dedup,
    /// 新测试信号
    Test,
    /// 性能改进信号
    Perf,
    /// 依赖/版本 bump 信号
    Bump,
}

/// 单条进化事件记录
#[derive(Debug, Clone)]
pub struct EvolutionEvent {
    pub timestamp: DateTime<Utc>,
    pub signal: EvolutionSignal,
    pub source: String,
    pub suggestion: String,
}

/// Journal — 进化事件存储
#[derive(Debug, Clone)]
pub struct Journal {
    events: Vec<EvolutionEvent>,
}

impl Journal {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// 记录一条进化事件
    pub fn record(&mut self, signal: EvolutionSignal, source: String, suggestion: String) {
        self.events.push(EvolutionEvent {
            timestamp: Utc::now(),
            signal,
            source,
            suggestion,
        });
    }

    /// 返回指定信号类型的计数器
    pub fn count_by_signal(&self, signal: &EvolutionSignal) -> usize {
        self.events.iter().filter(|e| e.signal == *signal).count()
    }

    /// 返回事件总数
    pub fn total_events(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_tracks_evolution_signals() {
        let mut journal = Journal::new();

        journal.record(
            EvolutionSignal::Refactor,
            "c197a87c2805".into(),
            "extract handler error logic".into(),
        );
        journal.record(
            EvolutionSignal::Dedup,
            "c197a87c2805".into(),
            "merge duplicate task parsing".into(),
        );
        journal.record(
            EvolutionSignal::Test,
            "c197a87c2805".into(),
            "add agent-card smoke test".into(),
        );
        journal.record(
            EvolutionSignal::Perf,
            "c197a87c2805".into(),
            "use RwLock instead of Mutex".into(),
        );
        journal.record(
            EvolutionSignal::Bump,
            "c197a87c2805".into(),
            "bump tokio to 1.42".into(),
        );

        // 验证每种信号都能被统计到 Journal
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Refactor), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Dedup), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Test), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Perf), 1);
        assert_eq!(journal.count_by_signal(&EvolutionSignal::Bump), 1);
        assert_eq!(journal.total_events(), 5);
    }
}