use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::sync::Mutex;

/// A scheduled cron job stored in SQLite.
#[derive(Debug, Clone)]
pub struct CronJob {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Human-readable name for the job.
    pub name: String,
    /// 6-field cron expression (seconds resolution), e.g. `"0 * * * * *"`.
    pub expr: String,
    /// The prompt text sent to the agent when this job fires.
    pub prompt: String,
    /// Identifies which session to run in, e.g. `"lark:ou_xxx"`.
    pub session_key: String,
    /// Whether the job is active.
    pub enabled: bool,
    /// Timestamp of the last successful run (None if never run).
    pub last_run: Option<DateTime<Utc>>,
    /// Optional target agent name (roster entry) for this job.
    pub agent: Option<String>,
    /// Optional condition string, e.g. `"idle_gt_seconds = 3600"`.
    /// When set, the scheduler evaluates the condition before firing.
    pub condition: Option<String>,
}

/// Persistent store for `CronJob` records backed by SQLite.
///
/// `Connection` is `!Send`, so we wrap it in a `Mutex` to allow the
/// `CronStore` to be shared across async tasks via `Arc<CronStore>`.
pub struct CronStore(Mutex<Connection>);

impl CronStore {
    /// Open an in-memory SQLite database (useful for tests).
    pub fn in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self(Mutex::new(conn)))
    }

    /// Open (or create) a SQLite database at the given path.
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self(Mutex::new(conn)))
    }

    /// Create the `cron_jobs` table if it does not already exist.
    fn init(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cron_jobs (
                id          TEXT    PRIMARY KEY,
                name        TEXT    NOT NULL UNIQUE,
                expr        TEXT    NOT NULL,
                prompt      TEXT    NOT NULL,
                session_key TEXT    NOT NULL,
                enabled     INTEGER NOT NULL DEFAULT 1,
                last_run    TEXT,
                agent       TEXT,
                condition   TEXT
            );",
        )?;
        Ok(())
    }

    /// Insert a new `CronJob` into the store.
    pub fn insert(&self, job: &CronJob) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, expr, prompt, session_key, enabled, last_run, agent, condition)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                job.id,
                job.name,
                job.expr,
                job.prompt,
                job.session_key,
                job.enabled as i64,
                job.last_run.map(|dt| dt.to_rfc3339()),
                job.agent,
                job.condition,
            ],
        )?;
        Ok(())
    }

    /// Return all jobs where `enabled = 1`.
    pub fn list_enabled(&self) -> anyhow::Result<Vec<CronJob>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, expr, prompt, session_key, enabled, last_run, agent, condition
             FROM cron_jobs WHERE enabled = 1",
        )?;
        let jobs = stmt.query_map([], |row| {
            let last_run_str: Option<String> = row.get(6)?;
            let agent: Option<String> = row.get(7)?;
            let condition: Option<String> = row.get(8)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                last_run_str,
                agent,
                condition,
            ))
        })?;

        let mut result = Vec::new();
        for job_res in jobs {
            let (id, name, expr, prompt, session_key, enabled, last_run_str, agent, condition) =
                job_res?;
            let last_run = last_run_str
                .map(|s| s.parse::<DateTime<Utc>>())
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::InvalidColumnType(
                        6,
                        format!("bad datetime: {e}"),
                        rusqlite::types::Type::Text,
                    )
                })?;
            result.push(CronJob {
                id,
                name,
                expr,
                prompt,
                session_key,
                enabled: enabled != 0,
                last_run,
                agent,
                condition,
            });
        }
        Ok(result)
    }

    /// Update the `last_run` timestamp for a given job.
    pub fn update_last_run(&self, id: &str, at: DateTime<Utc>) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "UPDATE cron_jobs SET last_run = ?1 WHERE id = ?2",
            params![at.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Upsert a cron job by name. Updates if name exists, inserts if not.
    /// On INSERT, sets `last_run = now` so the job does NOT fire immediately —
    /// it will wait until the next scheduled time. On UPDATE, `last_run` is
    /// preserved unchanged (scheduler continuity).
    #[allow(clippy::too_many_arguments)] // 7 job fields + &self; intentionally flat API for SQLite upsert
    pub fn upsert_by_name(
        &self,
        name: &str,
        expr: &str,
        prompt: &str,
        session_key: &str,
        enabled: bool,
        agent: Option<&str>,
        condition: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        // Initialize last_run to now so the scheduler does not fire the job immediately
        // on first startup. The first run will happen at the next scheduled interval.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, expr, prompt, session_key, enabled, last_run, agent, condition)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(name) DO UPDATE SET
                 expr        = excluded.expr,
                 prompt      = excluded.prompt,
                 session_key = excluded.session_key,
                 enabled     = excluded.enabled,
                 agent       = excluded.agent,
                 condition   = excluded.condition",
            params![id, name, expr, prompt, session_key, enabled as i64, now, agent, condition],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_job() -> CronJob {
        CronJob {
            id: Uuid::new_v4().to_string(),
            name: "test-job".to_string(),
            expr: "0 * * * * *".to_string(),
            prompt: "Hello from cron".to_string(),
            session_key: "lark:ou_test".to_string(),
            enabled: true,
            last_run: None,
            agent: None,
            condition: None,
        }
    }

    #[test]
    fn test_cron_store_insert_and_list() {
        let store = CronStore::in_memory().expect("in-memory store");
        let job = make_job();
        store.insert(&job).expect("insert");
        let jobs = store.list_enabled().expect("list_enabled");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, job.id);
        assert_eq!(jobs[0].name, "test-job");
        assert_eq!(jobs[0].prompt, "Hello from cron");
        assert!(jobs[0].last_run.is_none());
    }

    #[test]
    fn test_cron_store_disabled_job_not_listed() {
        let store = CronStore::in_memory().expect("in-memory store");
        let mut job = make_job();
        job.enabled = false;
        store.insert(&job).expect("insert");
        let jobs = store.list_enabled().expect("list_enabled");
        assert_eq!(
            jobs.len(),
            0,
            "disabled job should not appear in list_enabled"
        );
    }

    #[test]
    fn test_cron_store_update_last_run() {
        let store = CronStore::in_memory().expect("in-memory store");
        let job = make_job();
        store.insert(&job).expect("insert");

        let now = Utc::now();
        store
            .update_last_run(&job.id, now)
            .expect("update_last_run");

        let jobs = store.list_enabled().expect("list_enabled");
        assert_eq!(jobs.len(), 1);
        let last_run = jobs[0]
            .last_run
            .expect("last_run should be Some after update");
        // Allow up to 1 second of rounding from RFC3339 serialization
        let diff = (last_run - now).num_milliseconds().abs();
        assert!(
            diff < 1000,
            "last_run should be close to now, diff={diff}ms"
        );
    }

    #[test]
    fn test_upsert_by_name_inserts_new_job() {
        let store = CronStore::in_memory().unwrap();
        store
            .upsert_by_name(
                "test-job",
                "0 * * * * *",
                "hello",
                "ch:sc",
                true,
                None,
                None,
            )
            .unwrap();
        let jobs = store.list_enabled().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "test-job");
        assert_eq!(jobs[0].prompt, "hello");
        // last_run must be initialized to now so the job does NOT fire immediately.
        assert!(
            jobs[0].last_run.is_some(),
            "new job should have last_run set to prevent immediate firing"
        );
    }

    #[test]
    fn test_upsert_by_name_updates_existing_without_resetting_last_run() {
        let store = CronStore::in_memory().unwrap();
        // Insert initial job
        store
            .upsert_by_name(
                "my-job",
                "0 * * * * *",
                "old prompt",
                "ch:sc",
                true,
                None,
                None,
            )
            .unwrap();
        let jobs_before = store.list_enabled().unwrap();
        let job_id_before = jobs_before[0].id.clone();
        // Set a last_run
        let now = chrono::Utc::now();
        store.update_last_run(&job_id_before, now).unwrap();
        // Upsert with updated fields
        store
            .upsert_by_name(
                "my-job",
                "0 9 * * * *",
                "new prompt",
                "ch:sc2",
                true,
                None,
                None,
            )
            .unwrap();
        let jobs_after = store.list_enabled().unwrap();
        assert_eq!(jobs_after.len(), 1);
        assert_eq!(jobs_after[0].expr, "0 9 * * * *");
        assert_eq!(jobs_after[0].prompt, "new prompt");
        assert_eq!(jobs_after[0].session_key, "ch:sc2");
        // last_run must NOT be reset
        assert!(
            jobs_after[0].last_run.is_some(),
            "last_run should be preserved"
        );
    }

    #[test]
    fn test_cron_job_stores_agent_field() {
        let store = CronStore::in_memory().unwrap();
        store
            .upsert_by_name(
                "digest",
                "0 0 8 * * *",
                "Summarize",
                "lark:ou_x",
                true,
                Some("reviewer"),
                None,
            )
            .unwrap();
        let jobs = store.list_enabled().unwrap();
        assert_eq!(jobs[0].agent, Some("reviewer".to_string()));
    }

    #[test]
    fn test_cron_job_agent_none_by_default() {
        let store = CronStore::in_memory().unwrap();
        store
            .upsert_by_name(
                "task",
                "0 * * * * *",
                "Do it",
                "lark:ou_x",
                true,
                None,
                None,
            )
            .unwrap();
        let jobs = store.list_enabled().unwrap();
        assert_eq!(jobs[0].agent, None);
    }

    #[test]
    fn test_cron_job_stores_condition_field() {
        let store = CronStore::in_memory().unwrap();
        store
            .upsert_by_name(
                "h",
                "* * * * * *",
                "check",
                "lark:x",
                true,
                None,
                Some("idle_gt_seconds = 3600"),
            )
            .unwrap();
        let jobs = store.list_enabled().unwrap();
        assert_eq!(
            jobs[0].condition,
            Some("idle_gt_seconds = 3600".to_string())
        );
    }

    #[test]
    fn test_cron_job_condition_none_by_default() {
        let store = CronStore::in_memory().unwrap();
        store
            .upsert_by_name("plain", "0 * * * * *", "ping", "lark:x", true, None, None)
            .unwrap();
        let jobs = store.list_enabled().unwrap();
        assert_eq!(jobs[0].condition, None);
    }
}
