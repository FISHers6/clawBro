use crate::models::{CreateJobRequest, RunStatus, ScheduledJob, ScheduledRun};
use crate::store::{ClaimedJob, JobUpdate, SchedulerStore};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct JobQuery {
    pub name: Option<String>,
    pub name_contains: Option<String>,
    pub session_key: Option<String>,
}

#[derive(Clone)]
pub struct SchedulerService {
    store: Arc<SchedulerStore>,
}

impl SchedulerService {
    pub fn new(store: Arc<SchedulerStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<SchedulerStore> {
        &self.store
    }

    pub fn create_job(&self, req: CreateJobRequest, now: DateTime<Utc>) -> Result<ScheduledJob> {
        self.store.create_job(req, now)
    }

    pub fn update_job(
        &self,
        id: &str,
        update: JobUpdate,
        now: DateTime<Utc>,
    ) -> Result<Option<ScheduledJob>> {
        self.store.update_job(id, update, now)
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJob>> {
        self.store.list_jobs()
    }

    pub fn list_jobs_matching(&self, query: &JobQuery) -> Result<Vec<ScheduledJob>> {
        let jobs = self.store.list_jobs()?;
        Ok(filter_jobs(jobs, query))
    }

    pub fn pause_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        self.store.pause_job(id, now)
    }

    pub fn resume_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        self.store.resume_job(id, now)
    }

    pub fn delete_job(&self, id: &str) -> Result<bool> {
        self.store.delete_job(id)
    }

    pub fn delete_jobs_matching(&self, query: &JobQuery) -> Result<Vec<ScheduledJob>> {
        let jobs = self.list_jobs_matching(query)?;
        for job in &jobs {
            self.store.delete_job(&job.id)?;
        }
        Ok(jobs)
    }

    pub fn request_run_now(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        self.store.request_run_now(id, now)
    }

    pub fn claim_due_jobs(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<ClaimedJob>> {
        self.store.claim_due_jobs(now, limit, lease_secs)
    }

    pub fn start_run(&self, claim: &ClaimedJob, attempt: u32) -> Result<String> {
        self.store.start_run(claim, attempt)
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
        self.store
            .finish_run(claim, run_id, status, finished_at, error, result_summary)
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
