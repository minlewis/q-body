//! Skill Evolution Counter — 循环式迭代计数器
//!
//! 借鉴：yologdev/yoyo-evolve — skill_evolve counter 的循环式 reset 语义。
//! yoyo-evolve 在每个进化周期边界处把 counter 和 seen-state 一起 reset，
//! 配合 `.skill_evolve_counter` 文件做 bump 计数，使下一轮判重直接查 map
//! 而不是回扫整个 journal。reset 时将当前周期计数归档为快照，清空后
//! 进入下一周期，保证单调推进、不丢失历史趋势。
//!
//! → q-body 对应：`SkillEvolveCounter` 结构体 + `bump` / `reset` / `counts` API，
//!   用 `cycle: u64` 标识当前周期，`archive: HashMap<u64, HashMap<String, u32>>`
//!   保留历史快照供趋势分析。handler.rs 运行时接线按既定先例推迟。

use std::collections::HashMap;

/// 技能进化计数器 — 循环式迭代
///
/// 每个周期开始时计数值归零，reset() 时将当前周期快照归档到 history。
/// bump() 累加信号计数，counts() 返回当前周期所有信号及其计数。
///
/// # 设计决策
///
/// - 用 `cycle: u64` 标识当前周期，从 0 开始，每次 reset 递增。
/// - `archive` 保留历史快照（`cycle -> signals`），供趋势分析用。
/// - reset() 返回当前周期的快照，调用方可以选择是否持久化。
#[derive(Debug, Clone)]
pub struct SkillEvolveCounter {
    /// 当前周期编号
    cycle: u64,
    /// 当前周期内的信号计数
    signals: HashMap<String, u32>,
    /// 历史归档：cycle → signals
    archive: HashMap<u64, HashMap<String, u32>>,
}

impl Default for SkillEvolveCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillEvolveCounter {
    /// 创建一个新的空计数器（cycle=0，无信号）
    pub fn new() -> Self {
        Self {
            cycle: 0,
            signals: HashMap::new(),
            archive: HashMap::new(),
        }
    }

    /// 返回当前周期编号
    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    /// 统计指定信号在当前周期的出现次数
    pub fn get(&self, signal: &str) -> u32 {
        self.signals.get(signal).copied().unwrap_or(0)
    }

    /// 返回当前周期所有信号的计数快照
    pub fn counts(&self) -> &HashMap<String, u32> {
        &self.signals
    }

    /// 返回当前周期所有信号的总计数值
    pub fn total(&self) -> u32 {
        self.signals.values().sum()
    }

    /// 增加一个信号计数
    ///
    /// 如果信号已存在，计数加 1；否则初始化为 1。
    pub fn bump(&mut self, signal: &str) {
        let entry = self.signals.entry(signal.to_string()).or_insert(0);
        *entry += 1;
    }

    /// 增加指定次数的信号计数
    ///
    /// 用于批量导入场景。
    pub fn bump_by(&mut self, signal: &str, count: u32) {
        let entry = self.signals.entry(signal.to_string()).or_insert(0);
        *entry += count;
    }

    /// 重置当前周期，将当前信号归档到 history
    ///
    /// 返回被归档的当前周期信号快照（`HashMap<String, u32>`）。
    /// 调用方可以选择将快照写入文件或丢弃。
    /// 调用后 cycle 递增，信号表清空。
    pub fn reset(&mut self) -> HashMap<String, u32> {
        let snapshot = std::mem::take(&mut self.signals);
        self.archive.insert(self.cycle, snapshot.clone());
        self.cycle += 1;
        snapshot
    }

    /// 返回历史归档（cycle → signals）
    pub fn archive(&self) -> &HashMap<u64, HashMap<String, u32>> {
        &self.archive
    }

    /// 返回历史周期数（已完成的周期数量，不含当前周期）
    pub fn history_len(&self) -> usize {
        self.archive.len()
    }

    /// 检查是否有信号在当前周期已被记录
    pub fn has_signal(&self, signal: &str) -> bool {
        self.signals.contains_key(signal)
    }

    /// 返回当前周期中所有信号名称的列表
    pub fn signal_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.signals.keys().cloned().collect();
        names.sort();
        names
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_counter_is_empty() {
        let c = SkillEvolveCounter::new();
        assert_eq!(c.cycle(), 0);
        assert!(c.counts().is_empty());
        assert_eq!(c.total(), 0);
        assert_eq!(c.history_len(), 0);
    }

    #[test]
    fn test_bump_increments_counter() {
        let mut c = SkillEvolveCounter::new();
        c.bump("refactor");
        assert_eq!(c.get("refactor"), 1);
        assert_eq!(c.total(), 1);
        assert!(c.has_signal("refactor"));
        assert!(!c.has_signal("dedup"));
    }

    #[test]
    fn test_bump_multiple_signals() {
        let mut c = SkillEvolveCounter::new();
        c.bump("refactor");
        c.bump("dedup");
        c.bump("refactor");
        c.bump("test");
        assert_eq!(c.get("refactor"), 2);
        assert_eq!(c.get("dedup"), 1);
        assert_eq!(c.get("test"), 1);
        assert_eq!(c.total(), 4);
    }

    #[test]
    fn test_bump_by_adds_specified_amount() {
        let mut c = SkillEvolveCounter::new();
        c.bump_by("refactor", 5);
        assert_eq!(c.get("refactor"), 5);
        assert_eq!(c.total(), 5);
    }

    #[test]
    fn test_reset_archives_and_clears() {
        let mut c = SkillEvolveCounter::new();
        c.bump("refactor");
        c.bump("dedup");
        c.bump("refactor");
        assert_eq!(c.cycle(), 0);

        let snapshot = c.reset();
        assert_eq!(c.cycle(), 1);
        assert!(c.counts().is_empty());
        assert_eq!(c.total(), 0);
        assert_eq!(c.history_len(), 1);

        // 快照内容正确
        assert_eq!(snapshot.get("refactor"), Some(&2));
        assert_eq!(snapshot.get("dedup"), Some(&1));
    }

    #[test]
    fn test_multiple_resets() {
        let mut c = SkillEvolveCounter::new();

        // 周期 0
        c.bump("refactor");
        c.bump("refactor");
        c.reset();

        // 周期 1
        c.bump("dedup");
        c.reset();

        // 周期 2
        c.bump("test");
        c.bump("test");
        c.bump("test");

        assert_eq!(c.cycle(), 2);
        assert_eq!(c.get("test"), 3);
        assert_eq!(c.history_len(), 2);
        assert_eq!(c.archive().len(), 2);
        assert_eq!(c.signal_names(), vec!["test"]);
    }

    #[test]
    fn test_archive_preserves_history() {
        let mut c = SkillEvolveCounter::new();

        c.bump("refactor");
        c.reset();
        c.bump("dedup");
        c.reset();
        c.bump("test");
        c.reset();
        // 当前周期 3，空

        assert_eq!(c.history_len(), 3);
        let archive = c.archive();
        assert_eq!(archive.get(&0).unwrap().get("refactor"), Some(&1));
        assert_eq!(archive.get(&1).unwrap().get("dedup"), Some(&1));
        assert_eq!(archive.get(&2).unwrap().get("test"), Some(&1));
    }

    #[test]
    fn test_signal_names_sorted() {
        let mut c = SkillEvolveCounter::new();
        c.bump("test");
        c.bump("dedup");
        c.bump("refactor");
        assert_eq!(c.signal_names(), vec!["dedup", "refactor", "test"]);
    }

    #[test]
    fn test_unknown_signal_returns_zero() {
        let c = SkillEvolveCounter::new();
        assert_eq!(c.get("nonexistent"), 0);
    }

    #[test]
    fn test_empty_reset_advances_cycle() {
        let mut c = SkillEvolveCounter::new();
        assert_eq!(c.cycle(), 0);
        let snapshot = c.reset();
        assert!(snapshot.is_empty());
        assert_eq!(c.cycle(), 1);
        assert_eq!(c.history_len(), 1);
    }
}