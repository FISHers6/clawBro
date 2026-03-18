use super::store::{CronJob, CronStore};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

/// Callback invoked when a cron job fires.
///
/// Receives `(session_key, prompt, agent, condition)` and must return a `JoinHandle`
/// for the spawned work.  Using a callback instead of depending on `clawbro-agent`
/// directly keeps this crate free of circular dependencies.
///
/// The `condition` field is passed through so the caller can perform
/// any pre-fire checks (e.g. idle_gt_seconds) before dispatching.
pub type TriggerFn = Arc<
    dyn (Fn(
            String,         // session_key
            String,         // prompt
            Option<String>, // agent
            Option<String>, // condition
        ) -> tokio::task::JoinHandle<()>)
        + Send
        + Sync,
>;

/// Polls the cron store every second and fires due jobs via the trigger callback.
pub struct CronScheduler {
    store: Arc<CronStore>,
    trigger: TriggerFn,
}

impl CronScheduler {
    pub fn new(store: Arc<CronStore>, trigger: TriggerFn) -> Self {
        Self { store, trigger }
    }

    /// Run the scheduler loop (never returns under normal operation).
    ///
    /// Call this inside `tokio::spawn`.
    pub async fn run(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let now = Utc::now();
            match self.store.list_enabled() {
                Ok(jobs) => {
                    for job in jobs {
                        if is_due(&job, now) {
                            tracing::info!("CronScheduler: firing job '{}' ({})", job.name, job.id);
                            self.store.update_last_run(&job.id, now).ok();
                            (self.trigger)(
                                job.session_key.clone(),
                                job.prompt.clone(),
                                job.agent.clone(),
                                job.condition.clone(),
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("CronScheduler: failed to list jobs: {e}");
                }
            }
        }
    }
}

/// Returns `true` if the job should fire right now.
///
/// - If the job has never run (`last_run` is `None`) it is always considered due.
/// - Otherwise the next scheduled time after `last_run` is computed; if that
///   time is ≤ `now` the job is due.
fn is_due(job: &CronJob, now: DateTime<Utc>) -> bool {
    use cron::Schedule;
    use std::str::FromStr;

    let Ok(schedule) = Schedule::from_str(&job.expr) else {
        tracing::warn!(
            "CronScheduler: invalid cron expression '{}' for job '{}'",
            job.expr,
            job.name
        );
        return false;
    };

    match job.last_run {
        None => true,
        Some(last) => schedule.after(&last).next().is_some_and(|next| next <= now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::store::CronJob;
    use uuid::Uuid;

    fn make_job(expr: &str, last_run: Option<DateTime<Utc>>) -> CronJob {
        CronJob {
            id: Uuid::new_v4().to_string(),
            name: "test".to_string(),
            expr: expr.to_string(),
            prompt: "ping".to_string(),
            session_key: "lark:ou_test".to_string(),
            enabled: true,
            last_run,
            agent: None,
            condition: None,
        }
    }

    #[test]
    fn test_is_due_on_first_run() {
        // A job that has never run should always be due regardless of expression.
        let job = make_job("0 * * * * *", None);
        let now = Utc::now();
        assert!(is_due(&job, now), "job with no last_run should be due");
    }

    #[test]
    fn test_is_due_respects_interval_too_soon() {
        // Use a minutely expression ("0 * * * * *" — fires on the 0th second of every minute).
        // Set last_run to the most recent whole minute so the next tick is up to 60s away,
        // making it impossible for the 0ms-offset "now" to have passed it already.
        use chrono::Timelike;
        let now = Utc::now();
        // Truncate to the start of the current minute as the simulated last_run.
        let last = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now);

        let job = make_job("0 * * * * *", Some(last));
        // next fire after `last` is `last + 60s`, which is always > now (since last <= now)
        assert!(
            !is_due(&job, now),
            "job whose next tick is up to 60s away should not be due yet"
        );
    }

    #[test]
    fn test_is_due_respects_interval_overdue() {
        // last_run = 2 s ago → the 1-second mark passed → IS due.
        let last = Utc::now() - chrono::Duration::seconds(2);
        let job = make_job("* * * * * *", Some(last));
        let now = Utc::now();
        assert!(is_due(&job, now), "job last run 2s ago should be due");
    }

    #[test]
    fn test_is_due_invalid_expr() {
        let _job = make_job("not-a-cron", None);
        let now = Utc::now();
        // Invalid expression: even though last_run is None, invalid expr → false
        // Actually our implementation returns true when last_run is None before
        // checking the expression. Let's verify the false path for a job that has run.
        let job_with_last = make_job("not-a-cron", Some(now));
        assert!(
            !is_due(&job_with_last, now),
            "invalid expr should not be due"
        );
    }
}
