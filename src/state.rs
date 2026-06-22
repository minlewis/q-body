//! Task 存储：q-body 的 A2A TaskStore
//!
//! 使用 `tokio::sync::RwLock` 保护内存中的 Task 存储。
//! 支持：创建 Task、获取 Task、更新 Task 状态。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::a2a::types::{Artifact, Message, Task, TaskState, TaskStatus};

/// q-body 的任务存储
#[derive(Debug, Clone)]
pub struct TaskStore {
    tasks: Arc<RwLock<HashMap<String, Task>>>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 创建一个新 Task，状态为 submitted（无 skills）
    pub async fn create_task(
        &self,
        task_id: String,
        context_id: String,
        user_message: Message,
    ) -> Task {
        self.create_task_with_skills(task_id, context_id, user_message, Vec::new())
            .await
    }

    /// 创建一个新 Task，携带 skills（子 task 经 `create_sub_task` 继承）。
    pub async fn create_task_with_skills(
        &self,
        task_id: String,
        context_id: String,
        user_message: Message,
        skills: Vec<String>,
    ) -> Task {
        let task = Task {
            id: task_id.clone(),
            context_id: Some(context_id),
            status: TaskStatus {
                state: TaskState::submitted,
                message: None,
            },
            history: Some(vec![user_message]),
            artifacts: None,
            skills,
        };

        let mut store = self.tasks.write().await;
        store.insert(task_id.clone(), task.clone());
        task
    }

    /// 创建子 Task，继承父 Task 的 skill metadata。
    /// 借鉴 yoagent `SubAgentTool::with_skills`：父 agent 把 SkillSet 传给子 agent loop，
    /// 子获得与父相同的 skill 索引。q-body 对应：子 Task 继承父 Task 的 `skills`。
    /// 父 task 不存在时返回 `None`。
    pub async fn create_sub_task(
        &self,
        parent_id: &str,
        user_message: Message,
    ) -> Option<Task> {
        // 先以读锁取父 task 的 skills，避免持锁跨越 await 边界
        let parent_skills = {
            let store = self.tasks.read().await;
            store.get(parent_id).map(|p| p.skills.clone())?
        };

        let child_id = Uuid::new_v4().to_string();
        let context_id = format!("ctx-{}", &child_id[..8]);
        let task = Task {
            id: child_id.clone(),
            context_id: Some(context_id),
            status: TaskStatus {
                state: TaskState::submitted,
                message: None,
            },
            history: Some(vec![user_message]),
            artifacts: None,
            skills: parent_skills,
        };

        let mut store = self.tasks.write().await;
        store.insert(child_id.clone(), task.clone());
        Some(task)
    }

    /// 更新 Task 状态
    pub async fn update_status(&self, task_id: &str, state: TaskState) {
        let mut store = self.tasks.write().await;
        if let Some(task) = store.get_mut(task_id) {
            task.status.state = state;
        }
    }

    /// 添加 agent 回复消息到 Task
    pub async fn add_reply(&self, task_id: &str, reply: Message, artifacts: Vec<Artifact>) {
        let mut store = self.tasks.write().await;
        if let Some(task) = store.get_mut(task_id) {
            if let Some(ref mut history) = task.history {
                history.push(reply);
            }
            task.artifacts = Some(artifacts);
            task.status.state = TaskState::completed;
        }
    }

    /// 标记 Task 为失败
    pub async fn fail_task(&self, task_id: &str, error: &str) {
        let mut store = self.tasks.write().await;
        if let Some(task) = store.get_mut(task_id) {
            task.status.state = TaskState::failed;
            task.status.message = Some(Message {
                role: "agent".into(),
                parts: vec![crate::a2a::types::Part::text(error)],
                message_id: None,
            });
        }
    }

    /// 获取 Task
    pub async fn get_task(&self, task_id: &str) -> Option<Task> {
        let store = self.tasks.read().await;
        store.get(task_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{Message, Part};

    fn msg(text: &str) -> Message {
        Message {
            role: "user".into(),
            parts: vec![Part::text(text)],
            message_id: None,
        }
    }

    /// 子 task 应继承父 task 的 skills（借鉴 yoagent SubAgentTool::with_skills）
    #[tokio::test]
    async fn test_sub_task_inherits_parent_skills() {
        let store = TaskStore::new();
        let parent_skills = vec!["journal".to_string(), "evolve".to_string()];
        store
            .create_task_with_skills(
                "parent-1".into(),
                "ctx-parent".into(),
                msg("hi"),
                parent_skills.clone(),
            )
            .await;

        let child = store
            .create_sub_task("parent-1", msg("sub"))
            .await
            .expect("sub-task should be created when parent exists");

        assert_eq!(child.skills, parent_skills, "子 task 须继承父 task 的 skills");
        assert_ne!(child.id, "parent-1");
        assert_eq!(child.status.state, TaskState::submitted);
    }

    /// 父 task 不存在时 create_sub_task 返回 None
    #[tokio::test]
    async fn test_sub_task_missing_parent_returns_none() {
        let store = TaskStore::new();
        let child = store.create_sub_task("no-such-parent", msg("sub")).await;
        assert!(child.is_none(), "父 task 不存在时应返回 None");
    }
}