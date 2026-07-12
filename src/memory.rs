//! Active Memory 再生机制
//!
//! 每次新 task 创建时，从持久化存储加载记忆到上下文，
//! 让 LLM 推理时能感知之前的进化历史。
//!
//! 借鉴：yologdev/yoyo-evolve — Day 133 `synthesize: regenerate active memory context`

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 单条记忆项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// 记忆键（如 "last_assessment", "current_cycle"）
    pub key: String,
    /// 记忆值
    pub value: String,
    /// 记录时间
    pub recorded_at: DateTime<Utc>,
}

/// 活跃记忆上下文
///
/// 持有一组 key-value 记忆项，每次新 task 时 prewarm 到推理上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContext {
    /// 记忆项集合（按 key 索引）
    items: HashMap<String, MemoryItem>,
    /// 最后更新时间
    pub last_updated: DateTime<Utc>,
}

impl Default for MemoryContext {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryContext {
    /// 创建一个空的 MemoryContext
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            last_updated: Utc::now(),
        }
    }

    /// 添加或更新一条记忆项
    pub fn add_item(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let item = MemoryItem {
            key: key.into(),
            value: value.into(),
            recorded_at: Utc::now(),
        };
        self.items.insert(item.key.clone(), item);
        self.last_updated = Utc::now();
    }

    /// 获取一条记忆项的值
    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(|i| i.value.as_str())
    }

    /// 获取所有记忆项的数量
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 从 JSON 文件加载持久化记忆
    ///
    /// 文件不存在时返回空的 MemoryContext（首次运行）。
    pub fn load_from_json(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        if !path.exists() {
            tracing::info!("Memory file not found at {:?}, starting fresh", path);
            return Self::new();
        }

        match fs::read_to_string(path) {
            Ok(content) => {
                if content.trim().is_empty() {
                    return Self::new();
                }
                match serde_json::from_str::<MemoryContext>(&content) {
                    Ok(ctx) => {
                        tracing::info!("Loaded {} memory items from {:?}", ctx.len(), path);
                        ctx
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to parse memory file {:?}: {}. Starting fresh.",
                            path,
                            e
                        );
                        Self::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read memory file {:?}: {}. Starting fresh.", path, e);
                Self::new()
            }
        }
    }

    /// 将记忆格式化为 LLM 系统 prompt 上下文块
    ///
    /// 返回格式化的字符串，空记忆时返回空字符串。
    pub fn format_context(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = vec!["--- Active Memory ---".to_string()];
        let mut keys: Vec<&String> = self.items.keys().collect();
        keys.sort();

        for key in keys {
            if let Some(item) = self.items.get(key) {
                lines.push(format!("{}: {}", key, item.value));
            }
        }

        lines.push("---".to_string());
        lines.join("\n")
    }

    /// 持久化记忆到 JSON 文件
    ///
    /// 创建父目录（如果不存在）。
    pub fn persist_to_json(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        tracing::info!("Persisted {} memory items to {:?}", self.len(), path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("q-body-memory-test");
        // 确保父目录存在
        let _ = fs::create_dir_all(&dir);
        // 清理旧文件
        let _ = fs::remove_file(dir.join(name));
        dir.join(name)
    }

    #[test]
    fn test_new_memory_is_empty() {
        let ctx = MemoryContext::new();
        assert!(ctx.is_empty());
        assert_eq!(ctx.len(), 0);
        assert_eq!(ctx.format_context(), "");
    }

    #[test]
    fn test_add_and_get_item() {
        let mut ctx = MemoryContext::new();
        ctx.add_item("last_direction", "focus on dedup");
        assert!(!ctx.is_empty());
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx.get("last_direction"), Some("focus on dedup"));
        assert_eq!(ctx.get("nonexistent"), None);
    }

    #[test]
    fn test_add_updates_existing() {
        let mut ctx = MemoryContext::new();
        ctx.add_item("key", "old_value");
        ctx.add_item("key", "new_value");
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx.get("key"), Some("new_value"));
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let path = test_path("nonexistent.json");
        let ctx = MemoryContext::load_from_json(&path);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_load_from_empty_file() {
        let path = test_path("empty.json");
        fs::write(&path, "").unwrap();
        let ctx = MemoryContext::load_from_json(&path);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_persist_and_reload() {
        let path = test_path("persist_test.json");

        // 写入
        let mut ctx = MemoryContext::new();
        ctx.add_item("cycle", "3");
        ctx.add_item("assessment", "good progress");
        ctx.persist_to_json(&path).unwrap();

        // 重载
        let loaded = MemoryContext::load_from_json(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("cycle"), Some("3"));
        assert_eq!(loaded.get("assessment"), Some("good progress"));
    }

    #[test]
    fn test_format_context() {
        let mut ctx = MemoryContext::new();
        assert_eq!(ctx.format_context(), "");

        ctx.add_item("direction", "refactor handler");
        ctx.add_item("cycle", "5");
        let formatted = ctx.format_context();
        assert!(formatted.starts_with("--- Active Memory ---"));
        assert!(formatted.contains("direction: refactor handler"));
        assert!(formatted.contains("cycle: 5"));
        assert!(formatted.ends_with("---"));
    }

    #[test]
    fn test_multiple_items() {
        let mut ctx = MemoryContext::new();
        ctx.add_item("a", "1");
        ctx.add_item("b", "2");
        ctx.add_item("c", "3");
        assert_eq!(ctx.len(), 3);
        assert_eq!(ctx.get("a"), Some("1"));
        assert_eq!(ctx.get("b"), Some("2"));
        assert_eq!(ctx.get("c"), Some("3"));
    }
}