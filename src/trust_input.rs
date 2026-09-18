//! 高影响力输入信任门控 — 借鉴 yoyo-evolve Day193
//!
//! instruction-file disclosure：高影响力输入（SOUL.md / skill 指令）零 gate
//! 是自 concealing 的信任漏洞——被无声篡改的输入直接进 system prompt，
//! 事后无法审计「当时注入了什么」。
//!
//! 设计（P0 = journal 记忆优先，警告先落账再考虑拦截）：
//! - 启动/注入前对内容算指纹（FNV-1a 64，无第三方依赖）+ TAO 行数预算校验
//! - hash 变化 → 先落一条 `trust_input` journal 事件再注入（可审计可回放）
//! - 超预算不硬拦，`budget_ok=false` 落账可观测（同 cost_warn 哲学）
//!
//! standalone 模块先例同 learnings/clamp/cost：main 上暂无 SOUL 注入路径，
//! 注入点落地 main 后一行接线。

// DEBT(unwired): 高影响力输入信任门控，全仓零引用。
// 注：gate_audit.rs 文档注释称本模块「未合 main」，与事实不符——它已在 main 上。
// 处置：本轮只落 CI 闸门、不做删留裁决（见 plan 方向 A）。
// 闸门生效后，新增死代码会被直接拦下；存量债务在下一轮按实据逐条裁决。
#![allow(dead_code)]

use std::sync::Arc;
use tokio::sync::RwLock;

/// TAO 宪法行数硬上限（≤3000 行）
pub const TAO_MAX_LINES: usize = 3000;

/// journal 事件类型名
pub const TRUST_INPUT_EVENT: &str = "trust_input";

/// FNV-1a 64-bit 指纹（确定性、无依赖；仅用于变更检测，非密码学用途）
pub fn fnv1a64(data: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 注入内容的指纹（hash + 行数）
pub fn fingerprint(content: &str) -> (String, usize) {
    (fnv1a64(content), content.lines().count())
}

/// TAO 行数预算校验
pub fn within_budget(lines: usize, max_lines: usize) -> bool {
    lines <= max_lines
}

/// journal 事件：高影响力输入变更
#[derive(Debug, Clone, PartialEq)]
pub struct TrustInputEvent {
    /// 事件时刻（RFC3339 UTC）
    pub at: String,
    /// 输入名（如 "SOUL.md" / "skill:xxx"）
    pub name: String,
    /// 上一次注入的指纹（空串 = 首次注入）
    pub prev_hash: String,
    /// 本次注入的指纹
    pub new_hash: String,
    /// 本次内容行数
    pub lines: usize,
    /// 是否在 TAO 行数预算内
    pub budget_ok: bool,
}

/// gate 判定：hash 未变化 → None（无需落账）；
/// 首次注入（prev 为空）或 hash 变化 → 构造事件（先落账再注入）。
pub fn audit_injection(
    name: &str,
    prev_hash: &str,
    content: &str,
    now_rfc3339: &str,
) -> Option<TrustInputEvent> {
    let (new_hash, lines) = fingerprint(content);
    if new_hash == prev_hash {
        return None;
    }
    Some(TrustInputEvent {
        at: now_rfc3339.to_string(),
        name: name.to_string(),
        prev_hash: prev_hash.to_string(),
        new_hash,
        lines,
        budget_ok: within_budget(lines, TAO_MAX_LINES),
    })
}

/// 内存 journal：trust_input 事件追加式记录（同 CostJournal 形制）
#[derive(Debug, Default, Clone)]
pub struct TrustInputJournal {
    events: Arc<RwLock<Vec<TrustInputEvent>>>,
}

impl TrustInputJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条 trust_input 事件
    pub async fn record(&self, event: TrustInputEvent) {
        self.events.write().await.push(event);
    }

    /// 读取全部事件（审计/回放用）
    pub async fn events(&self) -> Vec<TrustInputEvent> {
        self.events.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a64_is_stable_and_hex16() {
        let h1 = fnv1a64("hello");
        let h2 = fnv1a64("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_fingerprint_empty_content() {
        let (hash, lines) = fingerprint("");
        assert_eq!(hash, fnv1a64(""));
        assert_eq!(lines, 0);
    }

    #[test]
    fn test_fingerprint_distinguishes_content() {
        let (h1, _) = fingerprint("# SOUL v1\nbelief: 知行合一");
        let (h2, _) = fingerprint("# SOUL v2\nbelief: 知行合一");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_fingerprint_counts_lines() {
        let (_, lines) = fingerprint("a\nb\nc");
        assert_eq!(lines, 3);
    }

    #[test]
    fn test_within_budget() {
        assert!(within_budget(3000, TAO_MAX_LINES));
        assert!(!within_budget(3001, TAO_MAX_LINES));
    }

    #[test]
    fn test_first_injection_records_event_with_empty_prev() {
        let ev = audit_injection("SOUL.md", "", "# soul\nline2", "2026-09-12T14:30:00Z")
            .expect("first injection must record");
        assert_eq!(ev.name, "SOUL.md");
        assert_eq!(ev.prev_hash, "");
        assert_eq!(ev.new_hash, fnv1a64("# soul\nline2"));
        assert_eq!(ev.lines, 2);
        assert!(ev.budget_ok);
    }

    #[test]
    fn test_unchanged_content_returns_none() {
        let content = "# soul\nstable";
        let (hash, _) = fingerprint(content);
        assert!(audit_injection("SOUL.md", &hash, content, "t").is_none());
    }

    #[test]
    fn test_changed_content_records_event() {
        let old = "# soul v1";
        let (old_hash, _) = fingerprint(old);
        let new = "# soul v2 — silently tampered";
        let ev = audit_injection("SOUL.md", &old_hash, new, "t2").expect("change must record");
        assert_eq!(ev.prev_hash, old_hash);
        assert_eq!(ev.new_hash, fnv1a64(new));
    }

    #[test]
    fn test_over_budget_content_marks_budget_ok_false() {
        let big = "x\n".repeat(TAO_MAX_LINES + 1);
        let ev =
            audit_injection("SOUL.md", "", &big, "t").expect("first injection must record");
        assert!(!ev.budget_ok);
        assert_eq!(ev.lines, TAO_MAX_LINES + 1);
    }

    #[tokio::test]
    async fn test_journal_roundtrip() {
        let journal = TrustInputJournal::new();
        assert!(journal.events().await.is_empty());
        let ev = audit_injection("SOUL.md", "", "content", "t1").unwrap();
        journal.record(ev.clone()).await;
        let ev2 = audit_injection("SOUL.md", &ev.new_hash, "changed", "t2").unwrap();
        journal.record(ev2.clone()).await;
        let all = journal.events().await;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], ev);
        assert_eq!(all[1], ev2);
        // 时间线可回放：第二次的 prev_hash == 第一次的 new_hash
        assert_eq!(all[1].prev_hash, all[0].new_hash);
    }
}
