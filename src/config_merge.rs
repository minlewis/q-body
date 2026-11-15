//! 配置合并优先级 — 防 `[section]` 越权覆盖顶层（CLI > 顶层 > section）
//!
//! 借鉴：yologdev/yoyo-evolve — 防 section 越权覆盖顶层 → 配置合并优先级显式分层
//!   (#1184：把顶层 config map 显式构建，规定 `[section]` 下 key 不得覆盖
//!    model/provider/base_url；合并优先级按「CLI 覆写 > 顶层 > section」分层落测试)
//!
//! 本模块是 standalone 纯函数层：不依赖任何 config.toml 宿主（config.rs 在
//! 未合并分支 feat/v0.1.4-config-journal-health），只对传给它的 `&toml::Table`
//! 做分层合并判定，宿主落地 main 后一行接线。零新增依赖（复用 toml 0.8）。

use std::collections::BTreeSet;

/// 受保护顶层键 — section 子表不得覆盖这些键。
/// 对齐 yoyo #1184：model / provider / base_url 是「单一事实源」键，被 section
/// 遮蔽即构成配置越权。
pub const PROTECTED_TOP_KEYS: &[&str] =
    &["model", "provider", "base_url", "api_url", "api_key_env"];

/// 合并层级 — 显式标注来源，杜绝隐式覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeLayer {
    /// CLI 覆写（最高优先级）
    Cli,
    /// config.toml 顶层
    TopLevel,
    /// config.toml 的 `[section]` 子表（最低优先级）
    Section,
}

impl MergeLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeLayer::Cli => "cli",
            MergeLayer::TopLevel => "top-level",
            MergeLayer::Section => "section",
        }
    }
}

/// 一次越权覆盖事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowEvent {
    /// 被 section 遮蔽的受保护顶层键
    pub key: String,
    /// 该键在 section 表中的取值（可见可审计，不涉及凭据）
    pub value: String,
    /// 所在 section 名
    pub section: String,
}

/// 对 config.toml 做「section 不得遮蔽受保护顶层键」审计。
///
/// 遮蔽成立的前提 = 顶层**确实定义了**该受保护键（有值可被覆盖）。若顶层没有
/// 该键，section 里的定义就是唯一来源，不是遮蔽、不越权（对齐 yoyo #1184：
/// 「防 section 越权覆盖顶层」——无顶层可覆盖便无越权）。
/// 支持任意深度 section 路径（`provider.deepseek`），路径逐段拼点回溯。
pub fn audit_section_shadows(table: &toml::Table) -> Vec<ShadowEvent> {
    let protected: BTreeSet<&str> = PROTECTED_TOP_KEYS.iter().copied().collect();
    // 顶层真实存在的键（只算标量/数组，不含 table 键）
    let present_top: BTreeSet<&str> = table
        .iter()
        .filter(|(_, v)| !v.is_table())
        .map(|(k, _)| k.as_str())
        .collect();

    let mut events = Vec::new();
    walk_sections(table, "", &protected, &present_top, &mut events);
    events
}

/// 递归遍历所有 [section] 子表，检查其保护键是否遮蔽了顶层同名键。
/// `path` 为当前表的点分路径（如 `provider.deepseek`），根传空串。
fn walk_sections(
    tbl: &toml::Table,
    path: &str,
    protected: &BTreeSet<&str>,
    present_top: &BTreeSet<&str>,
    events: &mut Vec<ShadowEvent>,
) {
    for (name, val) in tbl {
        if let Some(sub) = val.as_table() {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{}.{}", path, name)
            };
            // 检查本表内部的受保护键 —— 顶层也定义了才算遮蔽
            for (key, v) in sub {
                if protected.contains(key.as_str()) && present_top.contains(key.as_str()) {
                    events.push(ShadowEvent {
                        key: key.clone(),
                        value: v
                            .as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string()),
                        section: child_path.clone(),
                    });
                }
            }
            // 递归更深层 section
            walk_sections(sub, &child_path, protected, present_top, events);
        }
    }
}

/// 判定是否存在 section 越权遮蔽受保护顶层键。
pub fn has_shadow_override(table: &toml::Table) -> bool {
    !audit_section_shadows(table).is_empty()
}

/// 分层合并 — CLI 覆写 > 顶层 > section。
///
/// 对每个来源（CLI 值 > 顶层值 > section 值），返回「谁赢」的层级判定。
/// 用于断言：给定同一 key，顶层值与 section 值并存时，顶层必须优先——
/// 不允许 section 把顶层钉死。
pub fn resolve_layer(
    cli: Option<&str>,
    top_level: Option<&str>,
    section: Option<&str>,
) -> MergeLayer {
    if cli.is_some() {
        MergeLayer::Cli
    } else if top_level.is_some() {
        MergeLayer::TopLevel
    } else if section.is_some() {
        MergeLayer::Section
    } else {
        // 三处皆无值：键缺失，无层级可归
        MergeLayer::Section
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> toml::Table {
        s.parse::<toml::Table>().expect("valid toml fixture")
    }

    #[test]
    fn section_cannot_shadow_protected_top_level() {
        let t = table(
            r#"
            api_url = "https://top.example.com"
            [llm]
            api_url = "https://section.example.com"
            "#,
        );
        let events = audit_section_shadows(&t);
        assert_eq!(events.len(), 1, "受保护键被 section 遮蔽必须被检出");
        assert_eq!(events[0].key, "api_url");
        assert_eq!(events[0].section, "llm");
        assert_eq!(events[0].value, "https://section.example.com");
        assert!(has_shadow_override(&t));
    }

    #[test]
    fn section_without_protected_key_is_clean() {
        let t = table(
            r#"
            api_url = "https://top.example.com"
            [llm]
            model = "some-model"
            temperature = 0.7
            "#,
        );
        // model 在 [llm] 下是合法的当段配置；顶层没有 model —— 遮蔽不成立
        let events = audit_section_shadows(&t);
        assert!(
            events.is_empty(),
            "无顶层受保护键并存时不越权: {:?}",
            events
        );
        assert!(!has_shadow_override(&t));
    }

    #[test]
    fn top_level_and_cli_win_over_section() {
        // CLI > 顶层 > section
        assert_eq!(
            resolve_layer(Some("cli"), Some("top"), Some("sec")),
            MergeLayer::Cli
        );
        assert_eq!(
            resolve_layer(None, Some("top"), Some("sec")),
            MergeLayer::TopLevel
        );
        assert_eq!(resolve_layer(None, None, Some("sec")), MergeLayer::Section);
        assert_eq!(resolve_layer(None, None, None), MergeLayer::Section);
    }

    #[test]
    fn own_priority_is_explicit_and_stable() {
        // 越权 = section 键覆盖了同名的顶层受保护键；CLI 覆写是唯一被允许的高层来源
        let t = table(
            r#"
            model = "deepseek-v4-flash"
            [agent]
            model = "M3"
            "#,
        );
        let ev = audit_section_shadows(&t);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].key, "model");
        // resolve_layer 语义：顶层存在时 section 绝不生效
        assert_eq!(
            resolve_layer(None, Some("deepseek-v4-flash"), Some("M3")),
            MergeLayer::TopLevel
        );
    }

    #[test]
    fn multiple_sections_multiple_shadows_all_reported() {
        let t = table(
            r#"
            api_url = "https://top"
            base_url = "https://base"
            [llm]
            api_url = "https://llm"
            [agent]
            base_url = "https://agent"
            "#,
        );
        let events = audit_section_shadows(&t);
        assert_eq!(events.len(), 2, "两个 section 各遮蔽一个键都要上报");
        let mut keys: Vec<_> = events.iter().map(|e| e.key.clone()).collect();
        keys.sort();
        assert_eq!(keys, vec!["api_url".to_string(), "base_url".to_string()]);
    }

    #[test]
    fn nested_section_path_reported() {
        let t = table(
            r#"
            api_key_env = "TOP_KEY"
            [provider.deepseek]
            api_key_env = "SECTION_KEY"
            "#,
        );
        let events = audit_section_shadows(&t);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].section, "provider.deepseek");
    }
}
