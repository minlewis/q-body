//! src/nourish.rs — 养料链路治理（进化预算上限 + 花费/意图持久账本）
//!
//! 借鉴：Q00/ouroboros — budgeted evolution loop（预算制进化循环：
//! 节奏固定，池子有上限）→ q-body 把「每周觉醒」的克制扩展从理念落成
//! 每日可检数字（10-21 backlog 条目）。
//!
//! 借鉴：yologdev/yoyo-evolve #944 + rule 6 — durable sink（把每次养料
//! 采集花费与产出行追加写进持久账本，替代现在只留 stdout 的过期日志）
//! + rule 6（把「关于下一步的希望」落进 backlog 条目而非日报告口）
//! → q-body 花费/意图账固化（10-22 backlog 条目）。
//!
//! standalone 先例（08-05 learnings.rs / 09-02 clamp.rs / 09-04 darkroom.rs）：
//! 纯函数 + 单测先行落 standalone 模块；真实养料消费接线（读
//! ~/.hermes/cron/output/ 等运行时路径）合入 main 后一行接线。

/// 每日进化预算：token 上限与 PR 上限（借鉴 ouroboros budgeted loop）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvolutionBudget {
    /// 每日新扩张 LLM token 消耗上限
    pub daily_token_cap: u64,
    /// 每日新 PR 上限
    pub daily_pr_cap: u32,
}

impl EvolutionBudget {
    /// 默认预算：克制扩展（与 q-body 「克制的扩展」哲学一致）
    pub const DEFAULT: Self = Self {
        daily_token_cap: 200_000,
        daily_pr_cap: 1,
    };

    /// 判定是否超限——任一项超过即冻结新扩张。
    ///
    /// 借鉴 ouroboros budgeted loop：池子有上限，超限自动冻结新扩张直至回补。
    pub fn exceeded(&self, spent_tokens: u64, spent_prs: u32) -> bool {
        spent_tokens > self.daily_token_cap || spent_prs > self.daily_pr_cap
    }

    /// 返回首个被击穿的维度名（"tokens" / "prs" / 均未击穿则 None）。
    /// 用于冻结时给出可追溯的诊断依据（SOUL §16：数字必须可追溯）。
    pub fn first_breach(&self, spent_tokens: u64, spent_prs: u32) -> Option<&'static str> {
        if spent_tokens > self.daily_token_cap {
            Some("tokens")
        } else if spent_prs > self.daily_pr_cap {
            Some("prs")
        } else {
            None
        }
    }
}

/// 一条养料花费/意图账（durable sink 条目，借鉴 yoyo #944 + rule 6）
///
/// 改动前先自检：这是「花费计量」还是「每日预算」？
/// 两者区分的判据（SOUL §21 两本账）：
/// - 花费计量账（本条）：每次养料采集已发生的事实，落盘持久化可回放
/// - 每日预算账：新扩张的前置闸门，判定是否冻结（上方 `EvolutionBudget`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NourishLedgerEntry {
    /// 采集日期（YYYY-MM-DD，对齐 backlog 条目日期格式）
    pub date: String,
    /// 本次采集实际 token 花费
    pub spent_tokens: u64,
    /// 本次采集产出的 PR 数
    pub produced_prs: u32,
    /// 本次采集希望沉淀的「下一步」——落账而非只进日报告口（rule 6）
    pub next_step_hope: String,
}

impl NourishLedgerEntry {
    /// 序列化成 JSONL 单行（durable sink 追加写格式）
    pub fn to_jsonl(&self) -> String {
        format!(
            "{{\"date\":\"{}\",\"spent_tokens\":{},\"produced_prs\":{},\"next_step_hope\":{}}}\n",
            self.date,
            self.spent_tokens,
            self.produced_prs,
            serde_json::to_string(&self.next_step_hope).unwrap_or_else(|_| "\"\"".to_string()),
        )
    }

    /// 从 JSONL 单行解析；无法解析返回 None（不抛错，durable sink 容忍坏行）
    pub fn from_jsonl(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        Some(Self {
            date: v.get("date")?.as_str()?.to_string(),
            spent_tokens: v.get("spent_tokens")?.as_u64()?,
            produced_prs: v.get("produced_prs")?.as_u64()? as u32,
            next_step_hope: v.get("next_step_hope")?.as_str()?.to_string(),
        })
    }
}

/// 养料花费/意图账本：纯追加（append-only），跨天累计可回放。
///
/// 借鉴 yoyo #944 durable sink：替代「只留 stdout 的过期日志」，
/// 花费与意图锚定在不可伪造的落盘事实上（§15 三信号第三信号源）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NourishLedger {
    entries: Vec<NourishLedgerEntry>,
}

impl NourishLedger {
    /// 追加一条并立即回读验证（executed-artifact 回查纪律，Day 214）
    pub fn push(&mut self, entry: NourishLedgerEntry) -> &NourishLedgerEntry {
        self.entries.push(entry);
        self.entries.last().expect("just pushed")
    }

    /// 统计某日期累计 token 花费
    pub fn tokens_spent_on(&self, date: &str) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.date == date)
            .map(|e| e.spent_tokens)
            .sum()
    }

    /// 统计某日期累计 PR 产出
    pub fn prs_produced_on(&self, date: &str) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.date == date)
            .map(|e| e.produced_prs)
            .sum()
    }

    /// 序列化整本账为 JSONL 文本（供落盘）
    pub fn to_jsonl(&self) -> String {
        self.entries
            .iter()
            .map(|e| e.to_jsonl())
            .collect::<String>()
    }

    /// 从 JSONL 文本重建账本；坏行静默跳过（容忍，不丢好数据）
    pub fn from_jsonl(text: &str) -> Self {
        Self {
            entries: text
                .lines()
                .filter_map(NourishLedgerEntry::from_jsonl)
                .collect(),
        }
    }

    /// 账本条目数（供 /health 之类探针用「落盘行数」计量而非自报计数）
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空账本
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === EvolutionBudget（每日进化预算上限）===

    #[test]
    fn test_default_budget_not_exceeded_at_limits() {
        // 等于上限不算超限（对齐 09-06 recency 阈值 `<` 判例：下降到以下才触发）
        let b = EvolutionBudget::DEFAULT;
        assert!(!b.exceeded(200_000, 1));
        assert_eq!(b.first_breach(200_000, 1), None);
    }

    #[test]
    fn test_budget_exceeded_on_tokens() {
        let b = EvolutionBudget::DEFAULT;
        assert!(b.exceeded(200_001, 0));
        assert_eq!(b.first_breach(200_001, 0), Some("tokens"));
    }

    #[test]
    fn test_budget_exceeded_on_prs() {
        let b = EvolutionBudget::DEFAULT;
        assert!(b.exceeded(0, 2));
        assert_eq!(b.first_breach(0, 2), Some("prs"));
    }

    #[test]
    fn test_budget_tokens_priority_on_both_breached() {
        // 两个维度都超时，诊断依据取 token（首个被击穿的维度）
        let b = EvolutionBudget::DEFAULT;
        assert_eq!(b.first_breach(300_000, 3), Some("tokens"));
    }

    #[test]
    fn test_custom_budget_respected() {
        let b = EvolutionBudget {
            daily_token_cap: 50_000,
            daily_pr_cap: 2,
        };
        assert!(b.exceeded(50_001, 2));
        assert!(!b.exceeded(50_000, 2));
    }

    // === NourishLedgerEntry（花费/意图账条目）===

    #[test]
    fn test_entry_jsonl_roundtrip() {
        let e = NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 123_456,
            produced_prs: 1,
            next_step_hope: "把预算冻结逻辑接进 cron deliver".to_string(),
        };
        let line = e.to_jsonl();
        let back = NourishLedgerEntry::from_jsonl(&line).expect("parse back");
        assert_eq!(back, e);
    }

    #[test]
    fn test_entry_from_jsonl_rejects_garbage() {
        assert!(NourishLedgerEntry::from_jsonl("not json").is_none());
        assert!(NourishLedgerEntry::from_jsonl("").is_none());
        assert!(NourishLedgerEntry::from_jsonl("{}").is_none());
    }

    #[test]
    fn test_entry_next_step_hope_with_quotes_escaped() {
        // rule 6：希望文本可能含引号，JSONL 序列化必须正确转义
        let e = NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 1,
            produced_prs: 0,
            next_step_hope: "引号 \" 和换行 \n 都应转义".to_string(),
        };
        let back = NourishLedgerEntry::from_jsonl(&e.to_jsonl()).expect("parse back");
        assert_eq!(back.next_step_hope, e.next_step_hope);
    }

    // === NourishLedger（持久账本）===

    #[test]
    fn test_ledger_empty_by_default() {
        let l = NourishLedger::default();
        assert!(l.is_empty());
        assert_eq!(l.len(), 0);
        assert_eq!(l.tokens_spent_on("2026-10-25"), 0);
    }

    #[test]
    fn test_ledger_accumulates_per_date() {
        let mut l = NourishLedger::default();
        l.push(NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 10_000,
            produced_prs: 1,
            next_step_hope: "a".to_string(),
        });
        l.push(NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 5_000,
            produced_prs: 0,
            next_step_hope: "b".to_string(),
        });
        l.push(NourishLedgerEntry {
            date: "2026-10-24".to_string(),
            spent_tokens: 99_000,
            produced_prs: 1,
            next_step_hope: "c".to_string(),
        });
        assert_eq!(l.len(), 3);
        assert_eq!(l.tokens_spent_on("2026-10-25"), 15_000);
        assert_eq!(l.prs_produced_on("2026-10-25"), 1);
        assert_eq!(l.tokens_spent_on("2026-10-24"), 99_000);
    }

    #[test]
    fn test_ledger_full_jsonl_roundtrip() {
        let mut l = NourishLedger::default();
        l.push(NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 42_000,
            produced_prs: 1,
            next_step_hope: "回补预算".to_string(),
        });
        let text = l.to_jsonl();
        let back = NourishLedger::from_jsonl(&text);
        assert_eq!(back, l);
        // 落盘行数 = 条目数（探针只信落盘行数，evt-0028 纪律）
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn test_ledger_from_jsonl_tolerates_bad_lines() {
        let good = NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 1,
            produced_prs: 0,
            next_step_hope: "ok".to_string(),
        };
        let text = format!("garbage line\n{}\n  \n{{}}\n", good.to_jsonl());
        let l = NourishLedger::from_jsonl(&text);
        assert_eq!(l.len(), 1);
        assert_eq!(l.entries[0], good);
    }

    #[test]
    fn test_ledger_feeds_budget_check() {
        // 账本累计 → 预算判定联动：同一事实源，防「花超了还在扩张」
        let mut ledger = NourishLedger::default();
        ledger.push(NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 150_000,
            produced_prs: 1,
            next_step_hope: "x".to_string(),
        });
        assert!(!EvolutionBudget::DEFAULT.exceeded(
            ledger.tokens_spent_on("2026-10-25"),
            ledger.prs_produced_on("2026-10-25")
        ));
        ledger.push(NourishLedgerEntry {
            date: "2026-10-25".to_string(),
            spent_tokens: 60_000,
            produced_prs: 0,
            next_step_hope: "y".to_string(),
        });
        assert!(EvolutionBudget::DEFAULT.exceeded(
            ledger.tokens_spent_on("2026-10-25"),
            ledger.prs_produced_on("2026-10-25")
        ));
        // 跨天不受影响：昨天满预算不冻结今天
        assert!(!EvolutionBudget::DEFAULT.exceeded(
            ledger.tokens_spent_on("2026-10-24"),
            ledger.prs_produced_on("2026-10-24")
        ));
    }
}