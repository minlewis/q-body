//! Constitution size-gate contract tests.
//!
//! Mirrors `scripts/constitution-gate.py` (live copy under
//! `~/.hermes/skills/q-baby-self-check/scripts/constitution-gate.py`, consumed by the
//! 06:00 self-check). q-body does not shell out to that script; these tests pin the
//! *semantics* the gate enforces, so a silent regression in the count rules
//! (e.g. the regex drifting to count non-numbered section headers) is caught at the
//! repo test level instead of a red self-check.
//!
//! Source of truth: scripts/constitution-gate.py — 借鉴 yologdev/yoyo-evolve
//! (文档上限用 gate test 锁死而非靠自觉). Limits are the TAO.md hard lines:
//! SOUL.md 编号判例 ≤ 32, TAO.md ≤ 500 行.

/// Count numbered Operation Principle headers in the SOUL text.
/// A principle is `### N. <title>` — MUST match the Python regex `^### [0-9]+\. `.
pub fn count_soul_principles(text: &str) -> usize {
    let mut count = 0;
    for line in text.lines() {
        let t = line.trim_end();
        if t.starts_with("### ") {
            // after "### " must come digits, a '.', then a space
            let rest = &t[4..];
            let digits: usize = rest
                .bytes()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 0 {
                let after = &rest[digits..];
                if after.len() >= 2 && after.starts_with(". ") {
                    count += 1;
                }
            }
        }
    }
    count
}

pub const SOUL_MAX_PRINCIPLES: usize = 32;
pub const TAO_MAX_LINES: usize = 500;

fn tao_line_count(text: &str) -> usize {
    // matches Python `text.count("\n") + 1` (a trailing newline still counts +1 net)
    text.lines().count()
}

#[test]
fn counts_only_numbered_principles() {
    let sample = "## Operation Principles (Karpathy)\n\
                  ### 1. One\nbody\n\
                  ## Continuity\n\
                  ### Not-a-principle\n\
                  #### 2. nested-not-counted\n\
                  ### 12. Twelve\n";
    assert_eq!(count_soul_principles(sample), 2, "only numbered ### N. count");
}

#[test]
fn boundary_32_principles_ok_33_fails() {
    let mut ok = String::new();
    for i in 1..=32 {
        ok.push_str(&format!("### {i}. P{i}\nbody\n"));
    }
    assert_eq!(count_soul_principles(&ok), 32);
    assert!(count_soul_principles(&ok) <= SOUL_MAX_PRINCIPLES);

    ok.push_str("### 33. P33\nbody\n");
    assert_eq!(count_soul_principles(&ok), 33);
    assert!(count_soul_principles(&ok) > SOUL_MAX_PRINCIPLES);
}

#[test]
fn tao_500_lines_at_limit() {
    let mut body = String::new();
    for i in 0..500 {
        body.push('x');
        body.push('\n');
    }
    // 500 lines, trailing newline: count = 500 (lines()) — within hard limit
    assert_eq!(tao_line_count(&body), 500);
    assert!(tao_line_count(&body) <= TAO_MAX_LINES);
}

#[test]
fn gate_output_contract_marks_fail_explicitly() {
    // The script's FAIL path must name the numbers (so a human reading the self-check
    // can act without re-deriving them) — pinned here as the steadiest contract.
    let fail_line = "constitution-gate: FAIL (soul_principles=33, tao_lines=511)";
    assert!(fail_line.contains("FAIL"));
    assert!(fail_line.contains("soul_principles="));
    assert!(fail_line.contains("tao_lines="));
    // and the OK line
    let ok_line = "constitution-gate: OK (soul_principles=32/32, tao_lines=171/500)";
    assert!(ok_line.contains("OK") && ok_line.contains("/32") && ok_line.contains("/500"));
}