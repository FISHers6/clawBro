use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

type TaskFn = Box<dyn FnOnce() + Send + 'static>;

/// 每个 Session 维护一个串行执行队列，防止同一会话并发调用 Agent
pub struct LaneQueue {
    queues: DashMap<Uuid, Arc<Mutex<mpsc::UnboundedSender<TaskFn>>>>,
}

impl LaneQueue {
    pub fn new() -> Self {
        Self {
            queues: DashMap::new(),
        }
    }

    /// 将 async 任务提交到指定 session 的串行队列
    pub fn submit<F, Fut>(&self, session_id: Uuid, f: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let entry = self.queues.entry(session_id).or_insert_with(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<TaskFn>();
            tokio::spawn(async move {
                while let Some(task) = rx.recv().await {
                    task();
                }
            });
            Arc::new(Mutex::new(tx))
        });
        let tx = entry.value().clone();
        let task: TaskFn = Box::new(move || {
            tokio::spawn(async move { f().await });
        });
        tokio::spawn(async move {
            let guard = tx.lock().await;
            let _ = guard.send(task);
        });
    }
}

impl Default for LaneQueue {
    fn default() -> Self {
        Self::new()
    }
}
