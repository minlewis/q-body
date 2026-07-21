//! Proactive Memory Trigger — 借鉴 PMA (arXiv:2607.08716) 的双阶段记忆注入
//!
//! # 核心思想
//! 记忆不是每步都塞给 LLM，而是"该提醒时才提醒"。
//! Phase 1 (trigger): 判断当前步骤是否需要记忆注入
//! Phase 2 (inject): 从记忆库中召回相关条目，合成 `<context_for_action>` 或 `<no_intervention/>`
//!
//! # 与 PMA 的对应关系
//! | PMA (Python) | q-body (Rust) |
//! |---|---|
//! | `MemoryAgentTrigger` | `MemoryTrigger` |
//! | `UniversalMemory` | `MemoryStore` |
//! | `Phase 2` 决定 inject/noop | `RecallDecision` |
//! | `Terminal-Bench` runner | `TaskStore` 内嵌 |
//!
//! # 设计约束（老板校验维度）
//! - **最小改动**: 新文件，不动现有 handler/state 接口
//! - **不影响上游**: 独立模块，merge 时不会和 upstream 冲突
//! - **扩展性**: MemoryStore 可换成 SQLite/向量库（trait 化）
//! - **鲁棒性**: 无 LLM 时降级为 `<no_intervention/>`（不 panic）

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// 记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub created_at: String,
    pub access_count: u32,
}

/// 召回决策（Phase 2 的输出）
#[derive(Debug, Clone, PartialEq)]
pub enum RecallDecision {
    /// 不干预（默认值）
    NoIntervention,
    /// 注入上下文提醒
    Inject { context: String },
}

/// 记忆触发器（Phase 1）
/// 
/// 简化版规则（不依赖 LLM）：
/// - 首次对话（step == 1）必触发
/// - 每 interval 步触发一次
/// - 如果当前消息包含 "remember" / "memory" / "之前" / "上次" 关键词 → 强制触发
#[derive(Debug, Clone)]
pub struct MemoryTrigger {
    pub interval: u32,
    last_triggered_step: u32,
}

impl MemoryTrigger {
    pub fn new(interval: u32) -> Self {
        Self {
            interval,
            last_triggered_step: 0,
        }
    }

    /// 判断是否该触发记忆召回
    pub fn should_trigger(&mut self, step_count: u32, user_text: &str) -> bool {
        if step_count == 1 {
            self.last_triggered_step = step_count;
            return true;
        }
        
        // 关键词强制触发
        let trigger_keywords = ["remember", "memory", "之前", "上次", "记得", "忘了"];
        if trigger_keywords.iter().any(|kw| user_text.to_lowercase().contains(kw)) {
            self.last_triggered_step = step_count;
            return true;
        }
        
        // 周期性触发
        if step_count - self.last_triggered_step >= self.interval {
            self.last_triggered_step = step_count;
            return true;
        }
        
        false
    }
}

/// 记忆库（简化版：内存 HashMap，trait 化后可替换）
#[derive(Debug, Clone)]
pub struct MemoryStore {
    knowledge: Arc<RwLock<HashMap<String, MemoryEntry>>>,
    procedural: Arc<RwLock<HashMap<String, MemoryEntry>>>,
    status: Arc<RwLock<String>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            knowledge: Arc::new(RwLock::new(HashMap::new())),
            procedural: Arc::new(RwLock::new(HashMap::new())),
            status: Arc::new(RwLock::new(String::new())),
        }
    }

    /// 保存知识类记忆
    pub async fn save_knowledge(&self, content: String) -> String {
        let id = format!("k-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let entry = MemoryEntry {
            id: id.clone(),
            content,
            created_at: chrono::Utc::now().to_rfc3339(),
            access_count: 0,
        };
        let mut store = self.knowledge.write().await;
        store.insert(id.clone(), entry);
        id
    }

    /// 保存程序类记忆（错误模式、修复经验）
    pub async fn save_procedural(&self, content: String) -> String {
        let id = format!("p-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let entry = MemoryEntry {
            id: id.clone(),
            content,
            created_at: chrono::Utc::now().to_rfc3339(),
            access_count: 0,
        };
        let mut store = self.procedural.write().await;
        store.insert(id.clone(), entry);
        id
    }

    /// 更新内部状态（不展示给 action agent）
    pub async fn update_status(&self, content: String) {
        let mut status = self.status.write().await;
        *status = content;
    }

    /// 删除记忆
    pub async fn delete(&self, memory_id: &str) -> bool {
        let mut k = self.knowledge.write().await;
        let mut p = self.procedural.write().await;
        k.remove(memory_id).is_some() || p.remove(memory_id).is_some()
    }

    /// 简单 BM25 风格召回（关键词匹配 + access_count 加权）
    /// 
    /// 真实实现可替换为向量相似度（用 sqlite-vec 或 tencentdb）
    pub async fn recall(&self, query: &str, top_k: usize) -> Vec<MemoryEntry> {
        let k = self.knowledge.read().await;
        let p = self.procedural.read().await;
        
        let query_lower = query.to_lowercase();
        let mut candidates: Vec<_> = k.values()
            .chain(p.values())
            .filter(|e| e.content.to_lowercase().contains(&query_lower))
            .cloned()
            .collect();
        
        // 按 access_count 降序（最近被访问的排前面）
        candidates.sort_by_key(|e| std::cmp::Reverse(e.access_count));
        candidates.truncate(top_k);
        candidates
    }

    /// 统计记忆数量
    pub async fn stats(&self) -> (usize, usize) {
        let k = self.knowledge.read().await;
        let p = self.procedural.read().await;
        (k.len(), p.len())
    }
}

/// 合成注入上下文（Phase 2）
/// 
/// 从召回的记忆中合成 `<context_for_action>` 或 `<no_intervention/>`
/// 简化版：拼接召回的记忆条目 + 当前状态
pub fn synthesize_context(entries: &[MemoryEntry], status: &str) -> RecallDecision {
    if entries.is_empty() {
        return RecallDecision::NoIntervention;
    }
    
    let mut context = String::from("<context_for_action>\n");
    if !status.is_empty() {
        context.push_str(&format!("Status: {}\n", status));
    }
    for entry in entries {
        context.push_str(&format!("- [{}] {}\n", entry.id, entry.content));
    }
    context.push_str("</context_for_action>");
    
    RecallDecision::Inject { context }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trigger_first_step_always_fires() {
        let mut trigger = MemoryTrigger::new(5);
        assert!(trigger.should_trigger(1, "hello"));
    }

    #[tokio::test]
    async fn trigger_keyword_fires() {
        let mut trigger = MemoryTrigger::new(5);
        trigger.should_trigger(1, "hi"); // step 1
        assert!(trigger.should_trigger(2, "remember this"));
    }

    #[tokio::test]
    async fn trigger_interval_fires() {
        let mut trigger = MemoryTrigger::new(3);
        trigger.should_trigger(1, "hi");
        assert!(!trigger.should_trigger(2, "hello"));
        assert!(trigger.should_trigger(4, "hello")); // 1 + 3
    }

    #[tokio::test]
    async fn memory_store_save_and_recall() {
        let store = MemoryStore::new();
        let id = store.save_knowledge("Docker container has no systemd".to_string()).await;
        assert!(id.starts_with("k-"));
        
        let results = store.recall("docker", 5).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Docker container has no systemd");
    }

    #[tokio::test]
    async fn memory_store_delete_works() {
        let store = MemoryStore::new();
        let id = store.save_knowledge("test".to_string()).await;
        assert!(store.delete(&id).await);
        let results = store.recall("test", 5).await;
        assert!(results.is_empty());
    }

    #[test]
    fn synthesize_empty_returns_no_intervention() {
        let decision = synthesize_context(&[], "status");
        assert_eq!(decision, RecallDecision::NoIntervention);
    }

    #[test]
    fn synthesize_with_entries_returns_inject() {
        let entry = MemoryEntry {
            id: "k-1".to_string(),
            content: "API key expires in 3 days".to_string(),
            created_at: "2026-07-21".to_string(),
            access_count: 0,
        };
        let decision = synthesize_context(&[entry], "working on task");
        match decision {
            RecallDecision::Inject { context } => {
                assert!(context.contains("API key expires"));
                assert!(context.contains("<context_for_action>"));
            }
            _ => panic!("Expected Inject"),
        }
    }
}
