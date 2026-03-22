use super::models::{CreateJobRequest, RunStatus, ScheduledJob, ScheduledRun};
use super::store::{ClaimedJob, JobUpdate, SchedulerStore};
use crate::protocol::DashboardEvent;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Default)]
pub struct JobQuery {
    pub name: Option<String>,
    pub name_contains: Option<String>,
    pub session_key: Option<String>,
}

#[derive(Clone)]
pub struct SchedulerService {
    store: Arc<SchedulerStore>,
    dashboard_tx: Arc<OnceLock<broadcast::Sender<DashboardEvent>>>,
}

impl SchedulerService {
    pub fn new(store: Arc<SchedulerStore>) -> Self {
        Self {
            store,
            dashboard_tx: Arc::new(OnceLock::new()),
        }
    }

    pub fn store(&self) -> &Arc<SchedulerStore> {
        &self.store
    }

    pub fn set_dashboard_sender(&self, tx: broadcast::Sender<DashboardEvent>) {
        let _ = self.dashboard_tx.set(tx);
    }

    fn emit_dashboard_event(&self, event: DashboardEvent) {
        if let Some(tx) = self.dashboard_tx.get() {
            let _ = tx.send(event);
        }
    }

    fn emit_job_updated(&self, job: ScheduledJob) {
        self.emit_dashboard_event(DashboardEvent::SchedulerJobUpdated { job });
    }

    fn emit_run_updated(&self, run: ScheduledRun) {
        self.emit_dashboard_event(DashboardEvent::SchedulerRunUpdated { run });
    }

    fn load_run(&self, job_id: &str, run_id: &str) -> Result<Option<ScheduledRun>> {
        Ok(self
            .store
            .list_run_history(Some(job_id))?
            .into_iter()
            .find(|run| run.id == run_id))
    }

    pub fn create_job(&self, req: CreateJobRequest, now: DateTime<Utc>) -> Result<ScheduledJob> {
        let job = self.store.create_job(req, now)?;
        self.emit_job_updated(job.clone());
        Ok(job)
    }

    pub fn update_job(
        &self,
        id: &str,
        update: JobUpdate,
        now: DateTime<Utc>,
    ) -> Result<Option<ScheduledJob>> {
        let job = self.store.update_job(id, update, now)?;
        if let Some(job) = &job {
            self.emit_job_updated(job.clone());
        }
        Ok(job)
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJob>> {
        self.store.list_jobs()
    }

    pub fn get_job_by_id(&self, id: &str) -> Result<Option<ScheduledJob>> {
        self.store.get_job(id)
    }

    pub fn list_jobs_matching(&self, query: &JobQuery) -> Result<Vec<ScheduledJob>> {
        let jobs = self.store.list_jobs()?;
        Ok(filter_jobs(jobs, query))
    }

    pub fn pause_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let changed = self.store.pause_job(id, now)?;
        if changed {
            if let Some(job) = self.store.get_job(id)? {
                self.emit_job_updated(job);
            }
        }
        Ok(changed)
    }

    pub fn resume_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let changed = self.store.resume_job(id, now)?;
        if changed {
            if let Some(job) = self.store.get_job(id)? {
                self.emit_job_updated(job);
            }
        }
        Ok(changed)
    }

    pub fn delete_job(&self, id: &str) -> Result<bool> {
        let existed = self.store.get_job(id)?.is_some();
        let changed = self.store.delete_job(id)?;
        if changed && existed {
            self.emit_dashboard_event(DashboardEvent::SchedulerJobDeleted {
                job_id: id.to_string(),
            });
        }
        Ok(changed)
    }

    pub fn delete_jobs_matching(&self, query: &JobQuery) -> Result<Vec<ScheduledJob>> {
        let jobs = self.list_jobs_matching(query)?;
        for job in &jobs {
            self.store.delete_job(&job.id)?;
            self.emit_dashboard_event(DashboardEvent::SchedulerJobDeleted {
                job_id: job.id.clone(),
            });
        }
        Ok(jobs)
    }

    pub fn request_run_now(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let changed = self.store.request_run_now(id, now)?;
        if changed {
            if let Some(job) = self.store.get_job(id)? {
                self.emit_job_updated(job);
            }
        }
        Ok(changed)
    }

    pub fn claim_due_jobs(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<ClaimedJob>> {
        let claims = self.store.claim_due_jobs(now, limit, lease_secs)?;
        for claim in &claims {
            self.emit_job_updated(claim.job.clone());
        }
        Ok(claims)
    }

    pub fn start_run(&self, claim: &ClaimedJob, attempt: u32) -> Result<String> {
        let run_id = self.store.start_run(claim, attempt)?;
        if let Some(run) = self.load_run(&claim.job.id, &run_id)? {
            self.emit_run_updated(run);
        }
        Ok(run_id)
    }

    pub fn finish_run(
        &self,
        claim: &ClaimedJob,
        run_id: &str,
        status: RunStatus,
        finished_at: DateTime<Utc>,
        error: Option<String>,
        result_summary: Option<String>,
    ) -> Result<Option<ScheduledJob>> {
        let job =
            self.store
                .finish_run(claim, run_id, status, finished_at, error, result_summary)?;
        if let Some(run) = self.load_run(&claim.job.id, run_id)? {
            self.emit_run_updated(run);
        }
        if let Some(job) = &job {
            self.emit_job_updated(job.clone());
        }
        Ok(job)
    }

    pub fn list_run_history(&self, job_id: Option<&str>) -> Result<Vec<ScheduledRun>> {
        self.store.list_run_history(job_id)
    }
}

fn filter_jobs(jobs: Vec<ScheduledJob>, query: &JobQuery) -> Vec<ScheduledJob> {
    jobs.into_iter()
        .filter(|job| {
            query
                .name
                .as_ref()
                .is_none_or(|name| job.name.trim() == name.trim())
        })
        .filter(|job| {
            query.name_contains.as_ref().is_none_or(|needle| {
                let needle = needle.trim();
                !needle.is_empty() && job.name.contains(needle)
            })
        })
        .filter(|job| {
            query
                .session_key
                .as_ref()
                .is_none_or(|session_key| job.target.session_key() == session_key)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{CreateTargetRequest, ScheduleInput, SourceKind};

    fn make_service() -> SchedulerService {
        SchedulerService::new(Arc::new(
            SchedulerStore::in_memory().expect("in-memory scheduler store"),
        ))
    }

    fn attach_dashboard(service: &SchedulerService) -> broadcast::Receiver<DashboardEvent> {
        let (tx, rx) = broadcast::channel(32);
        service.set_dashboard_sender(tx);
        rx
    }

    fn create_req(name: &str, schedule: ScheduleInput) -> CreateJobRequest {
        CreateJobRequest {
            name: name.to_string(),
            schedule,
            timezone: None,
            target: CreateTargetRequest::Session(crate::scheduler::SessionTargetRequest {
                requested_kind: crate::scheduler::RequestedTargetKind::DeliveryMessage,
                session_key: "lark:group:oc_demo".to_string(),
                prompt: "hello".to_string(),
                agent: None,
                preconditions: vec![],
            }),
            max_retries: 0,
            source_kind: SourceKind::HumanCli,
            source_actor: "tester".to_string(),
            source_session_key: None,
            created_via: "test".to_string(),
            requested_by_role: None,
        }
    }

    #[test]
    fn emits_job_events_for_create_and_delete() {
        let service = make_service();
        let mut rx = attach_dashboard(&service);
        let now = Utc::now();

        let job = service
            .create_job(
                CreateJobRequest {
                    target: CreateTargetRequest::Session(crate::scheduler::SessionTargetRequest {
                        requested_kind: crate::scheduler::RequestedTargetKind::DeliveryMessage,
                        session_key: "lark:group:oc_demo".to_string(),
                        prompt: "hi".to_string(),
                        agent: None,
                        preconditions: vec![],
                    }),
                    ..create_req(
                        "job-events",
                        ScheduleInput::At {
                            run_at: now + chrono::Duration::minutes(5),
                        },
                    )
                },
                now,
            )
            .unwrap();

        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerJobUpdated { job: updated } => {
                assert_eq!(updated.id, job.id);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        assert!(service.delete_job(&job.id).unwrap());
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerJobDeleted { job_id } => assert_eq!(job_id, job.id),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn emits_run_and_job_events_for_run_lifecycle() {
        let service = make_service();
        let mut rx = attach_dashboard(&service);
        let now = Utc::now();
        let job = service
            .create_job(
                create_req(
                    "run-events",
                    ScheduleInput::At {
                        run_at: now + chrono::Duration::hours(1),
                    },
                ),
                now,
            )
            .unwrap();
        let _ = rx.try_recv();

        assert!(service.request_run_now(&job.id, now).unwrap());
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerJobUpdated { job: updated } => {
                assert_eq!(updated.id, job.id);
                assert!(updated.run_now_requested_at.is_some());
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let claims = service.claim_due_jobs(now, 1, 30).unwrap();
        assert_eq!(claims.len(), 1);
        let claim = &claims[0];
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerJobUpdated { job: updated } => {
                assert_eq!(updated.id, job.id);
                assert!(updated.running_since.is_some());
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let run_id = service.start_run(claim, 1).unwrap();
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerRunUpdated { run } => {
                assert_eq!(run.id, run_id);
                assert_eq!(run.job_id, job.id);
                assert_eq!(run.status, RunStatus::Running);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let finished_job = service
            .finish_run(
                claim,
                &run_id,
                RunStatus::Succeeded,
                now,
                None,
                Some("ok".to_string()),
            )
            .unwrap()
            .unwrap();
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerRunUpdated { run } => {
                assert_eq!(run.id, run_id);
                assert_eq!(run.status, RunStatus::Succeeded);
                assert_eq!(run.result_summary.as_deref(), Some("ok"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        match rx.try_recv().unwrap() {
            DashboardEvent::SchedulerJobUpdated { job: updated } => {
                assert_eq!(updated.id, finished_job.id);
                assert!(updated.last_run_at.is_some());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
