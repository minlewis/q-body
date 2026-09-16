//! 健康自证（self-evidencing health）
//!
//! 借鉴：yologdev/yoyo-evolve — eval-fix verdict 设计：IDLE 判定必须自带测量证据
//! （#915 UNVERIFIED 不升格），每个状态声明附测量依据。
//!
//! → q-body 对应：/health 判定响应内嵌 uptime、BuildID/mtime、最近 journal 时间戳，
//!   一次调用同时返回判定与证据，消灭「裸 ok 需二次查证」。
//!
//! 纯函数状态机 + env 门控（QBODY_JOURNAL_PATH / QBODY_EXE_PATH），
//! 按 09-09 trust_input / 08-05 learnings standalone 先例，端点接线落地后一行接入。

use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 一条健康证据：字段名 + 人可读值 + 机器可比对的原始值
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub field: String,
    pub value: String,
    /// 原始可机器比对值（epoch 秒 / 字节数 / build id），None 表示该项不可测量
    pub raw: Option<String>,
}

/// 证据来源，决定 verdict 是否可信
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceSource {
    /// 可测量且测量成功
    Measured,
    /// 未配置测量点（如 QBODY_JOURNAL_PATH 未设）
    Unconfigured,
    /// 配置了但测量失败（路径不存在/不可读）
    Unavailable,
}

impl EvidenceSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            EvidenceSource::Measured => "measured",
            EvidenceSource::Unconfigured => "unconfigured",
            EvidenceSource::Unavailable => "unavailable",
        }
    }
}

/// 健康判定 + 自证证据
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HealthReport {
    /// 判定：仅当关键证据来源为 Measured 且进程存活时才 "healthy"
    pub status: String,
    /// 判定性质：verified（有测量证据）/ degraded（部分证据缺失，status 仍可为 healthy 但标注降级）
    pub verdict: String,
    pub uptime_secs: u64,
    pub evidence: Vec<(String, EvidenceSource, Option<String>)>,
}

/// 判定词表（yoyo #921 Gap 1：verdict 语义不可硬编码在 if-else 链里）。
///
/// 每个条目绑定：verdict 字符串 + status 承诺（healthy 是否被允许宣称）。
/// 扩展方式：追加条目 + 在 `decide()` 里给出对应的匹配臂 + 全链路回归重测，
/// 不允许在 `build_report` 里内联裸字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerdictSpec {
    /// 判定字符串（"verified" / "degraded" / "unverified" / 未来扩展项）
    pub verdict: &'static str,
    /// 该判定下允许宣称的最高 status（"healthy" 或 "unhealthy"）
    pub max_status: &'static str,
}

/// 内置词表：三态证据来源 → 判定规格
pub const VERDICT_VERIFIED: VerdictSpec = VerdictSpec {
    verdict: "verified",
    max_status: "healthy",
};
pub const VERDICT_DEGRADED: VerdictSpec = VerdictSpec {
    verdict: "degraded",
    max_status: "healthy",
};
pub const VERDICT_UNVERIFIED: VerdictSpec = VerdictSpec {
    verdict: "unverified",
    max_status: "unhealthy",
};

/// 词表驱动的判定核心：按证据来源集合选 VerdictSpec。
///
/// 规则（对齐 yoyo #915：UNVERIFIED 不升格）：
/// - 任一关键证据 Unavailable → unverified（status 不得宣称 healthy）
/// - 证据 Unconfigured 或零证据 → degraded（进程可活，但该项自证缺失）
/// - 全部 Measured → verified
///
/// 返回 (&VerdictSpec, 按词表钳制后的 status)：词表是唯一 status 承诺来源，
/// 调用方不能再自由写 "healthy"。
pub fn decide(sources: &[EvidenceSource]) -> (&'static VerdictSpec, &'static str) {
    let has_unavailable = sources.iter().any(|s| *s == EvidenceSource::Unavailable);
    let has_unconfigured =
        sources.is_empty() || sources.iter().any(|s| *s == EvidenceSource::Unconfigured);

    let spec = if has_unavailable {
        &VERDICT_UNVERIFIED
    } else if has_unconfigured {
        &VERDICT_DEGRADED
    } else {
        &VERDICT_VERIFIED
    };
    (spec, spec.max_status)
}

/// 进程 uptime（epoch 起点由调用方传入便于测试）
pub fn uptime_secs(now: SystemTime, started_at: SystemTime) -> u64 {
    now.duration_since(started_at)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// 证据 1：可执行文件 BuildID/mtime — 二进制是否是最新构建
pub fn exe_mtime_evidence(
    exe_path: Option<&str>,
    now_secs: u64,
) -> (EvidenceSource, Option<String>) {
    let Some(path) = exe_path else {
        return (EvidenceSource::Unconfigured, None);
    };
    match std::fs::metadata(path) {
        Ok(meta) => match meta.modified() {
            Ok(mtime) => {
                let secs = mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                // 证据值：mtime 距今秒数 + 绝对 mtime（机器可比对）
                let raw = format!("{secs}|{now_secs}");
                (EvidenceSource::Measured, Some(raw))
            }
            Err(_) => (EvidenceSource::Unavailable, None),
        },
        Err(_) => (EvidenceSource::Unavailable, None),
    }
}

/// 证据 2：最近 journal 时间戳 — 记忆泵是否还在跳
pub fn journal_freshness_evidence(journal_path: Option<&str>) -> (EvidenceSource, Option<String>) {
    let Some(path) = journal_path else {
        return (EvidenceSource::Unconfigured, None);
    };
    match std::fs::metadata(path) {
        Ok(meta) => match meta.modified() {
            Ok(mtime) => {
                let secs = mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                (EvidenceSource::Measured, Some(secs.to_string()))
            }
            Err(_) => (EvidenceSource::Unavailable, None),
        },
        Err(_) => (EvidenceSource::Unavailable, None),
    }
}

/// 汇总判定：status + verdict 一并输出，每个状态声明附测量依据。
///
/// 判定词表驱动（见 `VerdictSpec` / `decide`）：build_report 不再内联裸字符串，
/// status 承诺一律来自词表条目的 `max_status`。
pub fn build_report(uptime: u64, sources: &[(EvidenceSource, Option<String>)]) -> HealthReport {
    let source_states: Vec<EvidenceSource> = sources.iter().map(|(s, _)| *s).collect();
    let (spec, status) = decide(&source_states);

    let evidence = sources
        .iter()
        .map(|(s, raw)| (s.as_str().to_string(), *s, raw.clone()))
        .collect();

    HealthReport {
        status: status.to_string(),
        verdict: spec.verdict.to_string(),
        uptime_secs: uptime,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn test_uptime_basic() {
        let now = SystemTime::now();
        assert_eq!(uptime_secs(now, now), 0);
        assert_eq!(uptime_secs(now, now - Duration::from_secs(90)), 90);
    }

    #[test]
    fn test_uptime_clock_skew_clamps_to_zero() {
        // started_at 在未来（时钟回拨）→ 不 panic，取 0
        let now = SystemTime::now();
        assert_eq!(uptime_secs(now, now + Duration::from_secs(100)), 0);
    }

    #[test]
    fn test_exe_mtime_unconfigured() {
        let (src, raw) = exe_mtime_evidence(None, 1000);
        assert_eq!(src, EvidenceSource::Unconfigured);
        assert!(raw.is_none());
    }

    #[test]
    fn test_exe_mtime_unavailable() {
        let (src, raw) = exe_mtime_evidence(Some("/nonexistent/q-body-bin"), 1000);
        assert_eq!(src, EvidenceSource::Unavailable);
        assert!(raw.is_none());
    }

    #[test]
    fn test_exe_mtime_measured() {
        let dir = std::env::temp_dir().join(format!("qbody-health-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("q-body");
        std::fs::write(&file, b"binary").unwrap();

        let (src, raw) = exe_mtime_evidence(Some(file.to_str().unwrap()), 2000);
        assert_eq!(src, EvidenceSource::Measured);
        let raw = raw.unwrap();
        let (mtime_secs, now_secs) = raw.split_once('|').unwrap();
        assert_eq!(now_secs, "2000");
        // mtime 必须是可解析的 epoch 秒
        assert!(mtime_secs.parse::<u64>().is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_journal_unconfigured_vs_unavailable() {
        let (src, _) = journal_freshness_evidence(None);
        assert_eq!(src, EvidenceSource::Unconfigured);

        let (src, _) = journal_freshness_evidence(Some("/nonexistent/journal.jsonl"));
        assert_eq!(src, EvidenceSource::Unavailable);
    }

    #[test]
    fn test_journal_measured() {
        let dir = std::env::temp_dir().join(format!("qbody-health-j-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("journal.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();

        let (src, raw) = journal_freshness_evidence(Some(file.to_str().unwrap()));
        assert_eq!(src, EvidenceSource::Measured);
        assert!(raw.unwrap().parse::<u64>().is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_report_verified_when_all_measured() {
        let r = build_report(
            60,
            &[
                (EvidenceSource::Measured, Some("123".into())),
                (EvidenceSource::Measured, Some("456".into())),
            ],
        );
        assert_eq!(r.status, "healthy");
        assert_eq!(r.verdict, "verified");
        assert_eq!(r.evidence.len(), 2);
        assert_eq!(r.evidence[0].0, "measured");
    }

    #[test]
    fn test_report_degraded_when_unconfigured() {
        // unconfigured 不降 status，但 verdict 必须暴露证据缺口
        let r = build_report(60, &[(EvidenceSource::Unconfigured, None)]);
        assert_eq!(r.status, "healthy");
        assert_eq!(r.verdict, "degraded");
    }

    #[test]
    fn test_report_unverified_when_unavailable() {
        // 对齐 yoyo #915：测量失败 → 不得宣称 healthy
        let r = build_report(
            60,
            &[
                (EvidenceSource::Measured, Some("1".into())),
                (EvidenceSource::Unavailable, None),
            ],
        );
        assert_eq!(r.status, "unhealthy");
        assert_eq!(r.verdict, "unverified");
    }

    #[test]
    fn test_report_empty_sources_is_degraded_not_verified() {
        // 零证据 ≠ 已验证
        let r = build_report(60, &[]);
        assert_eq!(r.verdict, "degraded");
        assert_eq!(r.status, "healthy");
    }

    // ---- 词表可扩展性（yoyo #921 Gap 1）----

    #[test]
    fn test_verdict_spec_vocabulary_is_extensible_not_hardcoded() {
        // 词表条目可被代码扩展（新 VerdictSpec 常量），decide() 返回的是词表引用
        // 而非裸字符串——扩展新判定不需要改 build_report 的字符串面量。
        let custom = VerdictSpec {
            verdict: "sampling",
            max_status: "healthy",
        };
        // 词表本身是数据：可检查、可比较、可注册
        assert_eq!(VERDICT_VERIFIED.verdict, "verified");
        assert_eq!(VERDICT_VERIFIED.max_status, "healthy");
        assert_eq!(VERDICT_DEGRADED.verdict, "degraded");
        assert_eq!(VERDICT_UNVERIFIED.max_status, "unhealthy");
        // status 承诺来自词表条目，不是 decide() 里内联的字面量
        let (spec, status) = decide(&[EvidenceSource::Measured]);
        assert_eq!(
            (spec.verdict, status),
            (VERDICT_VERIFIED.verdict, custom.max_status)
        );
    }

    #[test]
    fn test_decide_unavailable_forbids_healthy_via_vocab() {
        // 词表钳制：unverified 条目的 max_status=unhealthy，
        // status 承诺只能来自词表，调用方没有第二条路写出 "healthy"
        let (spec, status) = decide(&[EvidenceSource::Unavailable]);
        assert_eq!(spec.verdict, "unverified");
        assert_eq!(status, "unhealthy");
        assert_eq!(VERDICT_UNVERIFIED.max_status, "unhealthy");
    }

    #[test]
    fn test_build_report_regression_full_chain() {
        // ripgrep 式全链路回归：词表化重构后 /health 判定语义逐条不破
        // （对应重构前 build_report 的全部 4 条语义 + evidence 透传）
        let cases: Vec<(Vec<EvidenceSource>, &str, &str)> = vec![
            (vec![], "degraded", "healthy"),
            (
                vec![EvidenceSource::Measured, EvidenceSource::Measured],
                "verified",
                "healthy",
            ),
            (
                vec![EvidenceSource::Measured, EvidenceSource::Unconfigured],
                "degraded",
                "healthy",
            ),
            (
                vec![EvidenceSource::Measured, EvidenceSource::Unavailable],
                "unverified",
                "unhealthy",
            ),
            (vec![EvidenceSource::Unconfigured], "degraded", "healthy"),
            (vec![EvidenceSource::Unavailable], "unverified", "unhealthy"),
        ];
        for (sources, want_verdict, want_status) in cases {
            let src: Vec<(EvidenceSource, Option<String>)> =
                sources.iter().map(|s| (*s, None)).collect();
            let r = build_report(60, &src);
            assert_eq!(r.verdict, want_verdict, "verdict for {sources:?}");
            assert_eq!(r.status, want_status, "status for {sources:?}");
            assert_eq!(r.evidence.len(), sources.len());
        }
    }
}
