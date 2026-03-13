//! OrchestratorHeartbeat — 任务调度心跳
//!
//! 每隔 `interval` 做两件事：
//!   1. 检测超时的 Claimed 任务：retry_count < 3 → 重置为 Pending；否则 → Failed
//!   2. 派发 Ready 任务（Pending + deps_done）给对应的专才 agent

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use super::registry::{Task, TaskRegistry};
use super::session::TeamSession;

// ─── 类型 ────────────────────────────────────────────────────────────────────

/// 派发函数签名：
///   (agent_name, task) → async Result<()>
///
/// 调用方（main.rs）实现此闭包：构建 InboundMsg、发到 SessionRegistry.handle()
pub type DispatchFn = Arc<
    dyn Fn(String, Task) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

// ─── OrchestratorHeartbeat ───────────────────────────────────────────────────

/// 永久失败通知回调：(task_id, reason) → fire-and-forget
pub type FailureNotifyFn = Arc<dyn Fn(String, String) + Send + Sync>;

pub struct OrchestratorHeartbeat {
    registry: Arc<TaskRegistry>,
    session: Arc<TeamSession>,
    dispatch_fn: DispatchFn,
    interval: Duration,
    max_retries: u32,
    /// Optional callback invoked when a task permanently fails (retries exhausted).
    on_permanent_failure: Option<FailureNotifyFn>,
    /// Limits concurrent task dispatches to avoid overwhelming downstream agents.
    dispatch_semaphore: Arc<Semaphore>,
}

impl OrchestratorHeartbeat {
    pub fn new(
        registry: Arc<TaskRegistry>,
        session: Arc<TeamSession>,
        dispatch_fn: DispatchFn,
        interval: Duration,
        max_parallel: usize,
    ) -> Self {
        Self {
            registry,
            session,
            dispatch_fn,
            interval,
            max_retries: 3,
            on_permanent_failure: None,
            dispatch_semaphore: Arc::new(Semaphore::new(max_parallel.max(1))),
        }
    }

    /// Set the callback invoked when a task permanently fails.
    pub fn with_failure_notify(mut self, f: FailureNotifyFn) -> Self {
        self.on_permanent_failure = Some(f);
        self
    }

    /// 主循环（在 tokio::spawn 中运行）
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;
            if let Err(e) = self.tick().await {
                tracing::error!("OrchestratorHeartbeat tick error: {:#}", e);
            }
        }
    }

    async fn tick(&self) -> Result<()> {
        // 1. 检测并处理超时任务
        self.handle_stale_tasks()?;

        // 2. 导出 TASKS.md 快照
        let _ = self.session.sync_tasks_md(&self.registry);

        // 3. 派发 Ready 任务
        self.dispatch_ready_tasks().await?;

        Ok(())
    }

    fn handle_stale_tasks(&self) -> Result<()> {
        let stale = self.registry.find_stale_claimed()?;
        for task in stale {
            if task.retry_count < self.max_retries as i32 {
                self.registry.reset_claim(&task.id)?;
                tracing::warn!(
                    task_id = %task.id,
                    retry = task.retry_count,
                    "Reset stale task, will retry"
                );
            } else {
                let reason = "max retries exceeded";
                self.registry.mark_failed(&task.id, reason)?;
                tracing::error!(
                    task_id = %task.id,
                    "Task failed after {} retries", self.max_retries
                );
                if let Some(ref f) = self.on_permanent_failure {
                    f(task.id.clone(), reason.to_string());
                }
            }
        }
        Ok(())
    }

    async fn dispatch_ready_tasks(&self) -> Result<()> {
        let ready = self.registry.find_ready_tasks()?;
        for task in ready {
            let agent = match &task.assignee_hint {
                Some(a) => a.clone(),
                None => {
                    tracing::debug!(task_id = %task.id, "No assignee_hint, skipping dispatch");
                    continue;
                }
            };

            // 乐观认领（并发安全）
            match self.registry.try_claim(&task.id, &agent) {
                Ok(true) => {
                    tracing::info!(task_id = %task.id, agent = %agent, "Dispatching task");
                    let dispatch_fn = Arc::clone(&self.dispatch_fn);
                    let sem = Arc::clone(&self.dispatch_semaphore);
                    tokio::spawn(async move {
                        // Acquire owned permit before dispatching (max 4 concurrent).
                        // acquire_owned() is 'static-safe and keeps the permit alive
                        // until the task completes (drops at end of block).
                        let _permit = match sem.acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => return, // semaphore closed (gateway shutting down)
                        };
                        if let Err(e) = dispatch_fn(agent, task).await {
                            tracing::error!("Dispatch error: {:#}", e);
                        }
                    });
                }
                Ok(false) => {
                    // 已被其他 Heartbeat 实例认领（正常情况）
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, "try_claim error: {:#}", e);
                }
            }
        }
        Ok(())
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::registry::CreateTask;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn make_heartbeat(
        registry: Arc<TaskRegistry>,
        dispatched: Arc<Mutex<Vec<(String, String)>>>,
        interval: Duration,
    ) -> Arc<OrchestratorHeartbeat> {
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));

        let dispatch_fn: DispatchFn = Arc::new(move |agent, task| {
            let dispatched = Arc::clone(&dispatched);
            Box::pin(async move {
                dispatched.lock().unwrap().push((agent, task.id.clone()));
                Ok(())
            })
        });

        Arc::new(OrchestratorHeartbeat::new(
            registry,
            session,
            dispatch_fn,
            interval,
            4,
        ))
    }

    #[tokio::test]
    async fn test_heartbeat_dispatches_ready_task() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(CreateTask {
                id: "T003".into(),
                title: "JWT generation".into(),
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();

        let dispatched = Arc::new(Mutex::new(vec![]));
        let hb = make_heartbeat(
            Arc::clone(&registry),
            Arc::clone(&dispatched),
            Duration::from_millis(50),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        let d = dispatched.lock().unwrap();
        assert_eq!(d.len(), 1, "should dispatch exactly once");
        assert_eq!(d[0].0, "codex");
        assert_eq!(d[0].1, "T003");
    }

    #[tokio::test]
    async fn test_heartbeat_honors_configured_dispatch_limit() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));
        let dispatch_fn: DispatchFn =
            Arc::new(move |_agent, _task| Box::pin(async move { Ok(()) }));
        let hb =
            OrchestratorHeartbeat::new(registry, session, dispatch_fn, Duration::from_secs(1), 1);
        assert_eq!(hb.dispatch_semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn test_heartbeat_dispatches_tasks_unblocked_by_accepted_dependency() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(CreateTask {
                id: "T_BASE".into(),
                title: "base".into(),
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "T_NEXT".into(),
                title: "next".into(),
                assignee_hint: Some("claude".to_string()),
                deps: vec!["T_BASE".into()],
                ..Default::default()
            })
            .unwrap();

        registry.try_claim("T_BASE", "codex").unwrap();
        registry
            .submit_task_result("T_BASE", "codex", "ready")
            .unwrap();
        registry.accept_task("T_BASE", "leader").unwrap();

        let dispatched = Arc::new(Mutex::new(vec![]));
        let hb = make_heartbeat(
            Arc::clone(&registry),
            Arc::clone(&dispatched),
            Duration::from_millis(50),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        let d = dispatched.lock().unwrap();
        assert!(d
            .iter()
            .any(|(agent, task_id)| agent == "claude" && task_id == "T_NEXT"));
    }

    #[tokio::test]
    async fn test_heartbeat_resets_stale_tasks() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "stale".into(),
                timeout_secs: -1, // 立即超时
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T001", "codex").unwrap();

        let dispatched = Arc::new(Mutex::new(vec![]));
        let hb = make_heartbeat(
            Arc::clone(&registry),
            Arc::clone(&dispatched),
            Duration::from_millis(50),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        // 应该被重置为 pending（retry_count = 1 < 3）
        let task = registry.get_task("T001").unwrap().unwrap();
        // 重置后会被再次 dispatch，所以状态可能是 claimed 或 pending
        // 关键是 retry_count > 0（已经被重置过至少一次）
        assert!(task.retry_count > 0, "task should have been retried");
    }

    #[tokio::test]
    async fn test_heartbeat_marks_failed_after_max_retries() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        // 直接设置 retry_count = 3（模拟已重试 3 次）
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "exhausted".into(),
                timeout_secs: -1,
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();
        // 认领 3 次（每次 retry_count + 1）
        registry.try_claim("T001", "codex").unwrap(); // retry_count = 1
        registry.reset_claim("T001").unwrap();
        registry.try_claim("T001", "codex").unwrap(); // retry_count = 2
        registry.reset_claim("T001").unwrap();
        registry.try_claim("T001", "codex").unwrap(); // retry_count = 3
                                                      // 此时 retry_count = 3，下一次 heartbeat 应该标记 Failed

        let dispatched = Arc::new(Mutex::new(vec![]));
        let hb = make_heartbeat(
            Arc::clone(&registry),
            Arc::clone(&dispatched),
            Duration::from_millis(50),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        let task = registry.get_task("T001").unwrap().unwrap();
        assert!(
            task.status_raw.starts_with("failed"),
            "should be failed after max retries, got: {}",
            task.status_raw
        );
    }

    #[tokio::test]
    async fn test_heartbeat_does_not_redispatch_held_task() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(CreateTask {
                id: "T_HELD".into(),
                title: "held".into(),
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T_HELD", "codex").unwrap();
        registry
            .hold_claim("T_HELD", "codex", "missing_completion")
            .unwrap();

        let dispatched = Arc::new(Mutex::new(vec![]));
        let hb = make_heartbeat(
            Arc::clone(&registry),
            Arc::clone(&dispatched),
            Duration::from_millis(50),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        let d = dispatched.lock().unwrap();
        assert!(
            d.iter().all(|(_, id)| id != "T_HELD"),
            "held task must not be redispatched"
        );
    }

    /// 验证信号量确实限制并发派发数量：max_parallel=2 时，同时在飞行的 dispatch 不超过 2
    #[tokio::test]
    async fn test_heartbeat_semaphore_limits_concurrent_dispatches() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        // 创建 5 个任务，确保有足够的任务同时触发
        for i in 0..5u8 {
            registry
                .create_task(CreateTask {
                    id: format!("TC{:02}", i),
                    title: format!("concurrent task {}", i),
                    assignee_hint: Some("codex".into()),
                    ..Default::default()
                })
                .unwrap();
        }

        let inflight = Arc::new(AtomicUsize::new(0));
        let max_inflight = Arc::new(AtomicUsize::new(0));
        let inflight_c = Arc::clone(&inflight);
        let max_c = Arc::clone(&max_inflight);

        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir(
            "test-concurrent",
            tmp.path().to_path_buf(),
        ));

        let dispatch_fn: DispatchFn = Arc::new(move |_agent, _task| {
            let inflight = Arc::clone(&inflight_c);
            let max = Arc::clone(&max_c);
            Box::pin(async move {
                let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                // 原子更新最大并发数
                let mut prev = max.load(Ordering::SeqCst);
                while current > prev {
                    match max.compare_exchange(prev, current, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => break,
                        Err(x) => prev = x,
                    }
                }
                // 模拟 50ms 的工作耗时，让并发冲突暴露出来
                tokio::time::sleep(Duration::from_millis(50)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let hb = Arc::new(OrchestratorHeartbeat::new(
            Arc::clone(&registry),
            session,
            dispatch_fn,
            Duration::from_millis(20), // 快速 tick
            2,                         // max_parallel = 2
        ));

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        // 运行足够长时间让所有 5 个任务都被调度
        tokio::time::sleep(Duration::from_millis(500)).await;
        handle.abort();

        let peak = max_inflight.load(Ordering::SeqCst);
        assert!(
            peak <= 2,
            "max concurrent dispatches should be <= 2 (max_parallel), got peak = {}",
            peak
        );
        assert!(peak >= 1, "at least 1 task should have been dispatched");
    }

    #[tokio::test]
    async fn test_heartbeat_failure_callback_fires_on_max_retries() {
        let registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
        registry
            .create_task(CreateTask {
                id: "T_CB".into(),
                title: "will exhaust retries".into(),
                timeout_secs: -1,
                assignee_hint: Some("codex".to_string()),
                ..Default::default()
            })
            .unwrap();
        // Exhaust retries: claim 3 times to set retry_count = 3
        registry.try_claim("T_CB", "codex").unwrap();
        registry.reset_claim("T_CB").unwrap();
        registry.try_claim("T_CB", "codex").unwrap();
        registry.reset_claim("T_CB").unwrap();
        registry.try_claim("T_CB", "codex").unwrap(); // retry_count = 3

        let notified_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let notified_clone = Arc::clone(&notified_ids);

        let dispatched = Arc::new(Mutex::new(vec![]));
        let tmp = tempdir().unwrap();
        let session = Arc::new(TeamSession::from_dir("test-team", tmp.path().to_path_buf()));

        let dispatch_fn: DispatchFn = Arc::new(move |_agent, _task| {
            let dispatched = Arc::clone(&dispatched);
            Box::pin(async move {
                dispatched.lock().unwrap().push((_agent, _task.id));
                Ok(())
            })
        });

        let hb = Arc::new(
            OrchestratorHeartbeat::new(
                Arc::clone(&registry),
                session,
                dispatch_fn,
                Duration::from_millis(50),
                4,
            )
            .with_failure_notify(Arc::new(move |task_id, _reason| {
                notified_clone.lock().unwrap().push(task_id);
            })),
        );

        let hb_clone = Arc::clone(&hb);
        let handle = tokio::spawn(async move { hb_clone.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        // Callback must have fired for T_CB
        let ids = notified_ids.lock().unwrap();
        assert!(
            ids.contains(&"T_CB".to_string()),
            "failure_notify callback must fire when task exceeds max retries; got: {:?}",
            ids
        );
    }
}
