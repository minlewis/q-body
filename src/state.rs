//! Task 存储：q-body 的 A2A TaskStore
//!
//! 使用 `tokio::sync::RwLock` 保护内存中的 Task 存储。
//! 支持：创建 Task、获取 Task、更新 Task 状态。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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

    /// 创建一个新 Task，状态为 submitted
    pub async fn create_task(
        &self,
        task_id: String,
        context_id: String,
        user_message: Message,
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
        };

        let mut store = self.tasks.write().await;
        store.insert(task_id.clone(), task.clone());
        task
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