use super::models::{RunStatus, ScheduledJob};
use super::service::SchedulerService;
use anyhow::Result;
use chrono::Utc;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub type ExecutionResult = Result<ExecutionOutcome>;
pub type ExecutionFuture = Pin<Box<dyn Future<Output = ExecutionResult> + Send>>;
pub type ExecutionFn = Arc<dyn Fn(ScheduledJob) -> ExecutionFuture + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutcome {
    pub status: RunStatus,
    pub summary: Option<String>,
    pub error: Option<String>,
}

impl ExecutionOutcome {
    pub fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            status: RunStatus::Succeeded,
            summary: Some(summary.into()),
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub poll_interval: Duration,
    pub max_fetch_per_tick: usize,
    pub max_concurrent: usize,
    pub lease_secs: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(15),
            max_fetch_per_tick: 64,
            max_concurrent: 4,
            lease_secs: 120,
        }
    }
}

pub struct Scheduler {
    service: SchedulerService,
    executor: ExecutionFn,
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(service: SchedulerService, executor: ExecutionFn, config: SchedulerConfig) -> Self {
        Self {
            service,
            executor,
            config,
        }
    }

    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.config.poll_interval);
        loop {
            interval.tick().await;
            if let Err(err) = self.tick_once().await {
                tracing::error!("scheduler tick failed: {err:#}");
            }
        }
    }

    pub async fn tick_once(&self) -> Result<usize> {
        let now = Utc::now();
        let claims = self.service.claim_due_jobs(
            now,
            self.config.max_fetch_per_tick,
            self.config.lease_secs,
        )?;
        let to_run = claims
            .into_iter()
            .take(self.config.max_concurrent)
            .collect::<Vec<_>>();
        let count = to_run.len();
        for claim in to_run {
            let run_id = self.service.start_run(&claim, 1)?;
            let outcome = match (self.executor)(claim.job.clone()).await {
                Ok(outcome) => outcome,
                Err(err) => ExecutionOutcome {
                    status: RunStatus::Failed,
                    summary: None,
                    error: Some(err.to_string()),
                },
            };
            self.service.finish_run(
                &claim,
                &run_id,
                outcome.status,
                Utc::now(),
                outcome.error,
                outcome.summary,
            )?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{
        CreateJobRequest, CreateTargetRequest, RequestedTargetKind, ScheduleInput,
        SessionTargetRequest, SourceKind,
    };
    use crate::scheduler::{SchedulerStore, StoreConfig};
    use anyhow::anyhow;
    use chrono::Duration as ChronoDuration;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_service() -> SchedulerService {
        SchedulerService::new(Arc::new(
            SchedulerStore::open(std::path::Path::new(":memory:"), StoreConfig::default()).unwrap(),
        ))
    }

    fn req(name: &str, schedule: ScheduleInput) -> CreateJobRequest {
        CreateJobRequest {
            name: name.to_string(),
            schedule,
            timezone: Some("UTC".to_string()),
            target: CreateTargetRequest::Session(SessionTargetRequest {
                requested_kind: RequestedTargetKind::AgentTurn,
                session_key: "cron:test".to_string(),
                prompt: "ping".to_string(),
                agent: Some("default".to_string()),
                preconditions: vec![],
            }),
            max_retries: 0,
            source_kind: SourceKind::HumanCli,
            source_actor: "tester".to_string(),
            source_session_key: None,
            created_via: "cli".to_string(),
            requested_by_role: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn due_one_shot_job_fires_once() {
        let service = make_service();
        let now = Utc::now();
        let job = service
            .create_job(
                req(
                    "once",
                    ScheduleInput::At {
                        run_at: now - ChronoDuration::seconds(1),
                    },
                ),
                now,
            )
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let scheduler = Scheduler::new(
            service.clone(),
            Arc::new(move |_| {
                let calls = calls_clone.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(ExecutionOutcome::succeeded("ok"))
                })
            }),
            SchedulerConfig::default(),
        );
        scheduler.tick_once().await.unwrap();
        scheduler.tick_once().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let job = service
            .list_jobs()
            .unwrap()
            .into_iter()
            .find(|j| j.id == job.id)
            .unwrap();
        assert!(job.next_run_at.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_job_reschedules_itself() {
        let service = make_service();
        let now = Utc::now();
        let job = service
            .create_job(
                req("every", ScheduleInput::Every { interval_ms: 50 }),
                now - ChronoDuration::seconds(1),
            )
            .unwrap();
        let scheduler = Scheduler::new(
            service.clone(),
            Arc::new(move |_| Box::pin(async move { Ok(ExecutionOutcome::succeeded("ok")) })),
            SchedulerConfig::default(),
        );
        scheduler.tick_once().await.unwrap();
        let updated = service
            .list_jobs()
            .unwrap()
            .into_iter()
            .find(|j| j.id == job.id)
            .unwrap();
        assert!(updated.next_run_at > job.next_run_at);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn paused_job_does_not_execute() {
        let service = make_service();
        let now = Utc::now();
        let job = service
            .create_job(
                req(
                    "paused",
                    ScheduleInput::At {
                        run_at: now - ChronoDuration::seconds(1),
                    },
                ),
                now,
            )
            .unwrap();
        service.pause_job(&job.id, now).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let scheduler = Scheduler::new(
            service.clone(),
            Arc::new({
                let calls = calls.clone();
                move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(ExecutionOutcome::succeeded("ok"))
                    })
                }
            }),
            SchedulerConfig::default(),
        );
        scheduler.tick_once().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_now_causes_immediate_execution() {
        let service = make_service();
        let now = Utc::now();
        let job = service
            .create_job(
                req(
                    "run-now",
                    ScheduleInput::At {
                        run_at: now + ChronoDuration::hours(1),
                    },
                ),
                now,
            )
            .unwrap();
        service.request_run_now(&job.id, now).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let scheduler = Scheduler::new(
            service.clone(),
            Arc::new({
                let calls = calls.clone();
                move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(ExecutionOutcome::succeeded("ok"))
                    })
                }
            }),
            SchedulerConfig::default(),
        );
        scheduler.tick_once().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn overlapping_claims_do_not_execute_twice() {
        let service = make_service();
        let now = Utc::now();
        service
            .create_job(
                req(
                    "due",
                    ScheduleInput::At {
                        run_at: now - ChronoDuration::seconds(1),
                    },
                ),
                now,
            )
            .unwrap();
        let claims = service.claim_due_jobs(now, 10, 60).unwrap();
        assert_eq!(claims.len(), 1);
        let second = service.claim_due_jobs(now, 10, 60).unwrap();
        assert!(second.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_lease_can_be_reclaimed_after_crash() {
        let service = make_service();
        let now = Utc::now();
        service
            .create_job(
                req(
                    "lease",
                    ScheduleInput::At {
                        run_at: now - ChronoDuration::seconds(1),
                    },
                ),
                now,
            )
            .unwrap();
        let first = service.claim_due_jobs(now, 10, 1).unwrap();
        assert_eq!(first.len(), 1);
        let reclaimed = service
            .claim_due_jobs(now + ChronoDuration::seconds(2), 10, 1)
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_executor_marks_run_failed() {
        let service = make_service();
        let now = Utc::now();
        let job = service
            .create_job(
                req(
                    "fail",
                    ScheduleInput::At {
                        run_at: now - ChronoDuration::seconds(1),
                    },
                ),
                now,
            )
            .unwrap();
        let scheduler = Scheduler::new(
            service.clone(),
            Arc::new(move |_| Box::pin(async move { Err(anyhow!("boom")) })),
            SchedulerConfig::default(),
        );
        scheduler.tick_once().await.unwrap();
        let history = service.list_run_history(Some(&job.id)).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, RunStatus::Failed);
    }
}
