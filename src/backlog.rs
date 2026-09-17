//! src/backlog.rs — backlog 水位治理（drain）纯函数模块
//!
//! 借鉴：yologdev/yoyo-evolve — evolve.sh 的 backlog drain 设计
//! （receipts expire 自动归档 + slot rule 花在最老条目而非最新）。
//! → q-body 对应改法：backlog 条目带日期，PENDING > 7 天未消费自动归档到
//!   backlog.archive.md；Cron D 消化时优先消费最老 PENDING 条目，防止只追加不排水。
//!
//! standalone 先例（08-05 learnings.rs / 09-02 clamp.rs / 09-04 darkroom.rs）：
//! 纯函数 + 单测先行落 standalone 模块；真实 backlog 消费接线（读
//! ~/.hermes/q-body-backlog.md 的运行时路径）合入 main 后一行接线。

use chrono::NaiveDate;

/// 默认水位线：PENDING 条目超过 7 天未消费即过期归档
pub const DEFAULT_MAX_AGE_DAYS: i64 = 7;

/// 条目消费状态（backlog.md 中实际出现的状态的子集）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    /// **状态**：PENDING
    Pending,
    /// DONE-D @ YYYY-MM-DD（Cron D 已消化）
    DoneD,
    /// DONE-B @ YYYY-MM-DD（Cron B 已合批）
    DoneB,
    /// DEFERRED-*（延后到 Cron B / interactive session）
    Deferred,
    /// 其他/无法识别
    Other,
}

impl EntryStatus {
    /// 从 `**状态**：` 之后的文本解析，大小写不敏感。
    /// 裸 "DONE"（无后缀）按 D 消化处理（D 是默认消费者）。
    pub fn parse(raw: &str) -> Self {
        let u = raw.trim().trim_matches('*').trim().to_ascii_uppercase();
        if u.starts_with("PENDING") {
            EntryStatus::Pending
        } else if u.starts_with("DONE-D") || u == "DONE" || u.starts_with("DONE ") {
            EntryStatus::DoneD
        } else if u.starts_with("DONE-B") {
            EntryStatus::DoneB
        } else if u.starts_with("DEFERRED") {
            EntryStatus::Deferred
        } else {
            EntryStatus::Other
        }
    }

    pub fn is_pending(self) -> bool {
        self == EntryStatus::Pending
    }
}

/// 一条 backlog 条目（只关心日期 / 标题 / 状态三个水位治理字段）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogEntry {
    pub date: NaiveDate,
    pub title: String,
    pub status: EntryStatus,
}

/// 从文本中提取第一个 YYYY-MM-DD 日期。
/// 守卫：窗口前一个字符不能是数字（避免从更长的数字串中部截出假日期）。
pub fn extract_date(text: &str) -> Option<NaiveDate> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 10 {
        return None;
    }
    for i in 0..=(chars.len() - 10) {
        if i > 0 && chars[i - 1].is_ascii_digit() {
            continue;
        }
        // 窗口必须覆盖完整日期：后一字符也不能是数字，
        // 否则 "%Y-%m-%d" 会接受被截断的窗口（chrono %m/%d 吃单数字，" 2026-08-3" → 08-03）
        if i + 10 < chars.len() && chars[i + 10].is_ascii_digit() {
            continue;
        }
        let w: String = chars[i..i + 10].iter().collect();
        if let Ok(d) = NaiveDate::parse_from_str(&w, "%Y-%m-%d") {
            return Some(d);
        }
    }
    None
}

/// 行是否是已消化的划线条目：`- ~~P0: ...~~ [DONE-D @ ...]`
fn is_resolved_item(item: &str) -> bool {
    let s = item.trim_start();
    s.starts_with("~~") || s.contains("[DONE-D @") || s.contains("[DONE-B @")
}

/// 从列表项/段落文本提炼标题：剥前缀装饰，切掉借鉴来源尾巴。
fn inline_title(item: &str) -> String {
    let s = item
        .trim()
        .trim_start_matches("~~")
        .trim_start_matches("- ")
        .trim_start_matches("**")
        .trim_start();
    // 切借鉴尾巴：只在 "借鉴" 后跟 ：/: /空格/串尾 时才视为分隔符，
    // 避免误切内容词（如 "无借鉴尾巴"）
    let cut = s
        .match_indices("借鉴")
        .find(|(i, m)| {
            let end = i + m.len();
            end >= s.len()
                || s[end..].starts_with('：')
                || s[end..].starts_with(':')
                || s[end..].starts_with(' ')
        })
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    // match_indices 返回 char boundary 索引
    let head = s[..cut].trim_end();
    let head = head.trim_end_matches(&['—', '-', '>', '~', ' ', '，'][..]).trim_end();
    head.to_string()
}

/// 解析 backlog markdown，只返回 PENDING 条目（水位治理只关心未排水的水）。
///
/// 识别两种形态：
/// 1. 内联列表项 `- P0/P1: ... — 借鉴：...`（日期优先取行内日期，回退 section 标题日期）
/// 2. 加粗状态块 `**状态**：PENDING` + 段落描述（日期取 section 标题日期）
///
/// 划线已消化项（`~~...~~ [DONE-*]`）与非 PENDING section 一律跳过；
/// section 内已有内联 PENDING 项时不再产出 section 级条目（去重）。
pub fn parse_backlog(md: &str) -> Vec<BacklogEntry> {
    let mut out = Vec::new();
    let mut section_date: Option<NaiveDate> = None;
    let mut section_status = EntryStatus::Other;
    let mut section_title: Option<String> = None;
    let mut section_inline_pending = 0usize;

    let flush_section = |out: &mut Vec<BacklogEntry>,
                         status: EntryStatus,
                         date: Option<NaiveDate>,
                         title: &Option<String>,
                         inline: usize| {
        // section 级 PENDING 且没有内联项 → 产出 section 级条目
        if status.is_pending() && inline == 0 {
            if let Some(d) = date {
                let title = title.clone().unwrap_or_else(|| "(section 级 PENDING)".into());
                if !title.is_empty() {
                    out.push(BacklogEntry { date: d, title, status: EntryStatus::Pending });
                }
            }
        }
    };

    for raw_line in md.lines() {
        let line = raw_line.trim_end();
        let t = line.trim().trim_start_matches('|').trim();

        if line.starts_with("### ") {
            flush_section(&mut out, section_status, section_date, &section_title, section_inline_pending);
            section_date = extract_date(line);
            section_status = EntryStatus::Other;
            section_title = None;
            section_inline_pending = 0;
            continue;
        }
        if t.starts_with("---") {
            // 分隔线 = section 结束
            flush_section(&mut out, section_status, section_date, &section_title, section_inline_pending);
            section_status = EntryStatus::Other;
            section_title = None;
            section_inline_pending = 0;
            continue;
        }
        if let Some(rest) = t.strip_prefix("**状态**").or_else(|| t.strip_prefix("**Status**")) {
            let rest = rest.trim_start_matches(|c| c == ':' || c == '：').trim();
            section_status = EntryStatus::parse(rest);
            continue;
        }
        if let Some(rest) = t.strip_prefix("- ") {
            // 划线已消化项跳过；非 PENDING section 的内联项不收
            // （防 DONE-D section 里的划线/备注行被误判成新水）
            if is_resolved_item(rest) || !section_status.is_pending() {
                continue;
            }
            let date = extract_date(rest).or(section_date);
            let title = inline_title(rest);
            if let (Some(d), false) = (date, title.is_empty()) {
                out.push(BacklogEntry { date: d, title, status: EntryStatus::Pending });
                section_inline_pending += 1;
            }
            continue;
        }
        // 段落行：作为 section 级标题候选（跳过加粗元信息行与标题行）
        if !t.is_empty() && !t.starts_with('#') && !t.starts_with("**") && section_title.is_none() {
            let title = inline_title(t);
            if !title.is_empty() {
                section_title = Some(title);
            }
        }
    }
    flush_section(&mut out, section_status, section_date, &section_title, section_inline_pending);
    out
}

/// 条目是否过期：`today - entry_date > max_age_days`（恰好第 N 天不算过期）。
pub fn is_expired(entry_date: NaiveDate, today: NaiveDate, max_age_days: i64) -> bool {
    (today - entry_date).num_days() > max_age_days
}

/// 排水计划：过期 PENDING → archive，其余 keep，consume = 最老未过期 PENDING。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DrainPlan {
    pub archive: Vec<BacklogEntry>,
    pub keep: Vec<BacklogEntry>,
    pub consume: Option<BacklogEntry>,
}

/// slot rule：优先花在最老条目（而非最新）。
/// 并列日期取先出现者（输入顺序即 backlog 原文顺序，稳定裁决）。
pub fn plan_drain(entries: &[BacklogEntry], today: NaiveDate, max_age_days: i64) -> DrainPlan {
    let mut plan = DrainPlan::default();
    for e in entries {
        if e.status != EntryStatus::Pending {
            continue;
        }
        if is_expired(e.date, today, max_age_days) {
            plan.archive.push(e.clone());
        } else {
            plan.keep.push(e.clone());
        }
    }
    let mut consume: Option<BacklogEntry> = plan.keep.first().cloned();
    for e in &plan.keep {
        if let Some(c) = &consume {
            if e.date < c.date {
                consume = Some(e.clone());
            }
        }
    }
    plan.consume = consume;
    plan
}

/// 归档分节渲染：追加到 backlog.archive.md 的 markdown 片段。
pub fn archive_markdown(entries: &[BacklogEntry], today: NaiveDate) -> String {
    let mut out = format!("\n## Archived @ {} — 超期未消费（drain）\n\n", today);
    for e in entries {
        out.push_str(&format!("- [{}，archived @ {}] {}\n", e.date, today, e.title));
    }
    out
}

#[cfg(test)]
mod backlog_tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn test_status_parse_variants() {
        assert_eq!(EntryStatus::parse("PENDING"), EntryStatus::Pending);
        assert_eq!(EntryStatus::parse("DONE-D @ 2026-09-02"), EntryStatus::DoneD);
        assert_eq!(EntryStatus::parse("DONE-B @ 2026-08-18"), EntryStatus::DoneB);
        assert_eq!(EntryStatus::parse("DEFERRED-to-Cron-B (deadline)"), EntryStatus::Deferred);
        assert_eq!(EntryStatus::parse("DONE (09-04 自检执行)"), EntryStatus::DoneD);
        assert_eq!(EntryStatus::parse("不知道"), EntryStatus::Other);
    }

    #[test]
    fn test_extract_date_basic_and_guard() {
        assert_eq!(extract_date("### 2026-08-30 — P0"), Some(d(2026, 8, 30)));
        assert_eq!(extract_date("无日期行"), None);
        assert_eq!(extract_date("id=12026-08-30x"), None, "长数字串中部不得截出假日期");
        assert_eq!(extract_date("borrow 备注含 2026-09-04 日期"), Some(d(2026, 9, 4)));
    }

    #[test]
    fn test_is_expired_boundary() {
        // 恰好第 7 天：不过期；第 8 天：过期
        assert!(!is_expired(d(2026, 8, 29), d(2026, 9, 5), 7));
        assert!(is_expired(d(2026, 8, 28), d(2026, 9, 5), 7));
    }

    const FIXTURE: &str = "\
# Q-Body Backlog

### 2026-08-29 — P0

**来源**：c197a87c / x.md

**状态**：PENDING

- P1: skills.rs census 测试 — 借鉴：yoo #857
- ~~P0: 已消化项~~ [DONE-D @ 2026-08-31] — 见 issue #94

---

### 2026-08-31 — P0

**状态**：PENDING

- P0: Cron D 新鲜度校验前置，输入不新鲜直接 exit — 借鉴：yoo #866

---

### 2026-09-04 — P0

**状态**：PENDING

P0: backlog 水位治理 — 只追加不排水，借鉴 yoyo drain 模式

**借鉴**: yoo drain

---

### 2026-09-01 — P0

**状态**：DONE-D @ 2026-09-02

- P0: 已消化 section 内联项 — 借鉴：yoo #873
";

    #[test]
    fn test_parse_backlog_pending_only_and_dedup() {
        let entries = parse_backlog(FIXTURE);
        // 08-29 内联 1 条（划线跳过）+ 08-31 内联 1 条 + 09-04 section 级 1 条；09-01 DONE 跳过
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].date, d(2026, 8, 29));
        assert_eq!(entries[0].title, "P1: skills.rs census 测试");
        assert_eq!(entries[1].date, d(2026, 8, 31));
        assert_eq!(entries[1].title, "P0: Cron D 新鲜度校验前置，输入不新鲜直接 exit");
        assert_eq!(entries[2].date, d(2026, 9, 4));
        assert_eq!(entries[2].title, "P0: backlog 水位治理 — 只追加不排水");
        assert!(entries.iter().all(|e| e.status == EntryStatus::Pending));
    }

    #[test]
    fn test_plan_drain_archive_keep_consume() {
        let entries = parse_backlog(FIXTURE);
        let today = d(2026, 9, 5);
        let plan = plan_drain(&entries, today, 7);
        // 08-29 age=7 不过期；无过期项
        assert!(plan.archive.is_empty());
        assert_eq!(plan.keep.len(), 3);
        // slot rule：花在最老条目 08-29（并列取先出现者）
        assert_eq!(plan.consume.as_ref().unwrap().date, d(2026, 8, 29));
    }

    #[test]
    fn test_plan_drain_expired_goes_to_archive() {
        let entries = parse_backlog(FIXTURE);
        let today = d(2026, 9, 8); // 08-29 age=10 过期，08-31 age=8 过期
        let plan = plan_drain(&entries, today, 7);
        assert_eq!(plan.archive.len(), 2);
        assert_eq!(plan.keep.len(), 1);
        assert_eq!(plan.consume.as_ref().unwrap().date, d(2026, 9, 4));
        let md = archive_markdown(&plan.archive, today);
        assert!(md.contains("archived @ 2026-09-08"));
        assert!(md.contains("skills.rs census"));
    }

    #[test]
    fn test_plan_drain_all_expired_means_no_consume() {
        let entries = vec![BacklogEntry {
            date: d(2026, 8, 1),
            title: "老条目".into(),
            status: EntryStatus::Pending,
        }];
        let plan = plan_drain(&entries, d(2026, 9, 5), 7);
        assert!(plan.consume.is_none());
        assert_eq!(plan.archive.len(), 1);
        assert!(plan.keep.is_empty());
    }

    #[test]
    fn test_plan_drain_empty_input() {
        let plan = plan_drain(&[], d(2026, 9, 5), 7);
        assert_eq!(plan, DrainPlan::default());
    }

    #[test]
    fn test_inline_title_strips_decorations() {
        assert_eq!(inline_title("~~P0: 划线标题 ~~"), "P0: 划线标题");
        assert_eq!(inline_title("P1: 失败链 — 借鉴：a/b — failover"), "P1: 失败链");
        assert_eq!(inline_title("- P0: 无借鉴尾巴"), "P0: 无借鉴尾巴");
    }
}
