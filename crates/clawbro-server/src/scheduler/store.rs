use super::models::{
    AgentTurnTarget, CreateJobRequest, CreateTargetRequest, DeliveryMessageTarget,
    RequestedTargetKind, RunStatus, ScheduleKind, ScheduleSpec, ScheduledJob, ScheduledRun,
    ScheduledTarget, SessionTargetRequest, SourceKind, TriggerReason,
};
use super::schedule::{
    default_timezone, initial_next_run_at, next_run_after, normalize_schedule_input,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub default_timezone: String,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            default_timezone: default_timezone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobUpdate {
    pub enabled: Option<bool>,
    pub target: Option<ScheduledTarget>,
}

#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub job: ScheduledJob,
    pub lease_token: String,
    pub claimed_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub trigger_reason: TriggerReason,
}

pub struct SchedulerStore {
    conn: Mutex<Connection>,
    config: StoreConfig,
}

impl SchedulerStore {
    pub fn in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?, StoreConfig::default())
    }

    pub fn open(path: &Path, config: StoreConfig) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::with_connection(conn, config)
    }

    fn with_connection(conn: Connection, config: StoreConfig) -> Result<Self> {
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            config,
        })
    }

    fn init(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL,
                schedule_kind TEXT NOT NULL,
                schedule_expr TEXT NOT NULL,
                timezone TEXT NOT NULL,
                target_payload_json TEXT NOT NULL,
                next_run_at TEXT,
                last_scheduled_at TEXT,
                last_run_at TEXT,
                last_success_at TEXT,
                run_now_requested_at TEXT,
                max_retries INTEGER NOT NULL DEFAULT 0,
                lease_token TEXT,
                lease_expires_at TEXT,
                running_since TEXT,
                source_kind TEXT NOT NULL,
                source_actor TEXT NOT NULL,
                source_session_key TEXT,
                created_via TEXT NOT NULL,
                requested_by_role TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS scheduled_job_runs (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                scheduled_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                status TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                error TEXT,
                result_summary TEXT,
                trigger_reason TEXT NOT NULL,
                executor_session_key TEXT,
                executor_agent TEXT,
                FOREIGN KEY(job_id) REFERENCES scheduled_jobs(id)
            );
            CREATE INDEX IF NOT EXISTS idx_scheduled_jobs_due
                ON scheduled_jobs (enabled, next_run_at, run_now_requested_at);
            CREATE INDEX IF NOT EXISTS idx_scheduled_job_runs_job_id
                ON scheduled_job_runs (job_id, started_at);
            "#,
        )?;
        Ok(())
    }

    pub fn create_job(&self, req: CreateJobRequest, now: DateTime<Utc>) -> Result<ScheduledJob> {
        let CreateJobRequest {
            name,
            schedule,
            timezone,
            target,
            max_retries,
            source_kind,
            source_actor,
            source_session_key,
            created_via,
            requested_by_role,
        } = req;
        let target = resolve_create_target(target)?;
        let timezone = timezone.unwrap_or_else(|| self.config.default_timezone.clone());
        let schedule = normalize_schedule_input(schedule, now)?;
        let next_run_at = initial_next_run_at(&schedule, &timezone, now)?;
        let job = ScheduledJob {
            id: Uuid::new_v4().to_string(),
            name,
            enabled: true,
            schedule,
            timezone,
            target,
            next_run_at,
            last_scheduled_at: None,
            last_run_at: None,
            last_success_at: None,
            run_now_requested_at: None,
            max_retries,
            lease_token: None,
            lease_expires_at: None,
            running_since: None,
            source_kind,
            source_actor,
            source_session_key,
            created_via,
            requested_by_role,
            created_at: now,
            updated_at: now,
        };
        self.insert_job(&job)?;
        Ok(job)
    }

    pub fn update_job(
        &self,
        id: &str,
        update: JobUpdate,
        now: DateTime<Utc>,
    ) -> Result<Option<ScheduledJob>> {
        let mut job = match self.get_job(id)? {
            Some(job) => job,
            None => return Ok(None),
        };
        if let Some(enabled) = update.enabled {
            job.enabled = enabled;
        }
        if let Some(target) = update.target {
            job.target = target;
        }
        job.updated_at = now;
        self.replace_job(&job)?;
        Ok(Some(job))
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJob>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, enabled, schedule_kind, schedule_expr, timezone, target_payload_json,
                   next_run_at, last_scheduled_at, last_run_at, last_success_at, run_now_requested_at,
                   max_retries, lease_token, lease_expires_at, running_since,
                   source_kind, source_actor, source_session_key, created_via, requested_by_role,
                   created_at, updated_at
              FROM scheduled_jobs
          ORDER BY name
            "#,
        )?;
        let rows = stmt.query_map([], |row| read_job_row(row))?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?);
        }
        Ok(jobs)
    }

    pub fn get_job(&self, id: &str) -> Result<Option<ScheduledJob>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT id, name, enabled, schedule_kind, schedule_expr, timezone, target_payload_json,
                   next_run_at, last_scheduled_at, last_run_at, last_success_at, run_now_requested_at,
                   max_retries, lease_token, lease_expires_at, running_since,
                   source_kind, source_actor, source_session_key, created_via, requested_by_role,
                   created_at, updated_at
              FROM scheduled_jobs
             WHERE id = ?1
            "#,
            params![id],
            read_job_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn pause_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE scheduled_jobs SET enabled = 0, updated_at = ?2 WHERE id = ?1",
            params![id, now.to_rfc3339()],
        )?;
        Ok(changed > 0)
    }

    pub fn resume_job(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE scheduled_jobs SET enabled = 1, updated_at = ?2 WHERE id = ?1",
            params![id, now.to_rfc3339()],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_job(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM scheduled_job_runs WHERE job_id = ?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM scheduled_jobs WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn request_run_now(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE scheduled_jobs SET run_now_requested_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![id, now.to_rfc3339()],
        )?;
        Ok(changed > 0)
    }

    pub fn claim_due_jobs(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<ClaimedJob>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(
            r#"
            SELECT id, name, enabled, schedule_kind, schedule_expr, timezone, target_payload_json,
                   next_run_at, last_scheduled_at, last_run_at, last_success_at, run_now_requested_at,
                   max_retries, lease_token, lease_expires_at, running_since,
                   source_kind, source_actor, source_session_key, created_via, requested_by_role,
                   created_at, updated_at
              FROM scheduled_jobs
             WHERE enabled = 1
               AND (
                    (next_run_at IS NOT NULL AND next_run_at <= ?1)
                 OR (run_now_requested_at IS NOT NULL AND run_now_requested_at <= ?1)
               )
          ORDER BY COALESCE(run_now_requested_at, next_run_at) ASC
             LIMIT ?2
            "#,
        )?;
        let candidates = stmt.query_map(params![now.to_rfc3339(), limit as i64], |row| {
            read_job_row(row)
        })?;
        let mut claimed = Vec::new();
        for row in candidates {
            let job = row?;
            if has_live_lease(&job, now) {
                continue;
            }
            let lease_token = Uuid::new_v4().to_string();
            let lease_expires_at = now + chrono::Duration::seconds(lease_secs);
            let trigger_reason = if job.run_now_requested_at.is_some_and(|at| at <= now) {
                TriggerReason::RunNow
            } else {
                TriggerReason::Due
            };
            let scheduled_at = job.run_now_requested_at.unwrap_or_else(|| {
                job.next_run_at
                    .expect("due jobs must have next_run_at or run_now_requested_at")
            });
            let changed = tx.execute(
                r#"
                UPDATE scheduled_jobs
                   SET lease_token = ?2,
                       lease_expires_at = ?3,
                       running_since = ?1,
                       run_now_requested_at = CASE
                           WHEN run_now_requested_at IS NOT NULL AND run_now_requested_at <= ?1 THEN NULL
                           ELSE run_now_requested_at
                       END,
                       updated_at = ?1
                 WHERE id = ?4
                   AND enabled = 1
                   AND (lease_expires_at IS NULL OR lease_expires_at <= ?1)
                "#,
                params![
                    now.to_rfc3339(),
                    lease_token,
                    lease_expires_at.to_rfc3339(),
                    job.id,
                ],
            )?;
            if changed == 1 {
                let mut claimed_job = job;
                claimed_job.lease_token = Some(lease_token.clone());
                claimed_job.lease_expires_at = Some(lease_expires_at);
                claimed_job.running_since = Some(now);
                if matches!(trigger_reason, TriggerReason::RunNow) {
                    claimed_job.run_now_requested_at = None;
                }
                claimed.push(ClaimedJob {
                    job: claimed_job,
                    lease_token,
                    claimed_at: now,
                    scheduled_at,
                    trigger_reason,
                });
            }
        }
        drop(stmt);
        tx.commit()?;
        Ok(claimed)
    }

    pub fn start_run(&self, claim: &ClaimedJob, attempt: u32) -> Result<String> {
        let run_id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO scheduled_job_runs (
                id, job_id, scheduled_at, started_at, finished_at, status,
                attempt, error, result_summary, trigger_reason, executor_session_key, executor_agent
            ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, NULL, NULL, ?7, ?8, ?9)
            "#,
            params![
                run_id,
                claim.job.id,
                claim.scheduled_at.to_rfc3339(),
                claim.claimed_at.to_rfc3339(),
                run_status_to_str(RunStatus::Running),
                attempt,
                trigger_reason_to_str(claim.trigger_reason),
                claim.job.target.session_key(),
                claim.job.target.executor_agent(),
            ],
        )?;
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE scheduled_job_runs
               SET finished_at = ?2,
                   status = ?3,
                   error = ?4,
                   result_summary = ?5
             WHERE id = ?1
            "#,
            params![
                run_id,
                finished_at.to_rfc3339(),
                run_status_to_str(status),
                error,
                result_summary,
            ],
        )?;

        let mut job = self
            .get_job_tx(&tx, &claim.job.id)?
            .context("job disappeared while finishing run")?;
        let next_run_at = match status {
            RunStatus::Succeeded | RunStatus::Skipped => {
                next_run_after(&job.schedule, &job.timezone, finished_at)?
            }
            RunStatus::Failed | RunStatus::Running => job.next_run_at,
        };
        job.next_run_at = next_run_at;
        job.last_scheduled_at = Some(claim.scheduled_at);
        job.last_run_at = Some(finished_at);
        if matches!(status, RunStatus::Succeeded) {
            job.last_success_at = Some(finished_at);
        }
        job.lease_token = None;
        job.lease_expires_at = None;
        job.running_since = None;
        job.updated_at = finished_at;
        self.replace_job_tx(&tx, &job)?;
        tx.commit()?;
        Ok(Some(job))
    }

    pub fn list_run_history(&self, job_id: Option<&str>) -> Result<Vec<ScheduledRun>> {
        let conn = self.conn.lock().unwrap();
        let sql = if job_id.is_some() {
            r#"
            SELECT id, job_id, scheduled_at, started_at, finished_at, status, attempt, error,
                   result_summary, trigger_reason, executor_session_key, executor_agent
              FROM scheduled_job_runs
             WHERE job_id = ?1
          ORDER BY scheduled_at DESC
            "#
        } else {
            r#"
            SELECT id, job_id, scheduled_at, started_at, finished_at, status, attempt, error,
                   result_summary, trigger_reason, executor_session_key, executor_agent
              FROM scheduled_job_runs
          ORDER BY scheduled_at DESC
            "#
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(job_id) = job_id {
            stmt.query_map(params![job_id], read_run_row)?
        } else {
            stmt.query_map([], read_run_row)?
        };
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    fn insert_job(&self, job: &ScheduledJob) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        insert_job_row(&conn, job)
    }

    fn replace_job(&self, job: &ScheduledJob) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        self.replace_job_tx(&conn, job)
    }

    fn replace_job_tx<T: std::ops::Deref<Target = Connection>>(
        &self,
        conn: &T,
        job: &ScheduledJob,
    ) -> Result<()> {
        conn.execute(
            r#"
            UPDATE scheduled_jobs
               SET name = ?2,
                   enabled = ?3,
                   schedule_kind = ?4,
                   schedule_expr = ?5,
                   timezone = ?6,
                   target_payload_json = ?7,
                   next_run_at = ?8,
                   last_scheduled_at = ?9,
                   last_run_at = ?10,
                   last_success_at = ?11,
                   run_now_requested_at = ?12,
                   max_retries = ?13,
                   lease_token = ?14,
                   lease_expires_at = ?15,
                   running_since = ?16,
                   source_kind = ?17,
                   source_actor = ?18,
                   source_session_key = ?19,
                   created_via = ?20,
                   requested_by_role = ?21,
                   created_at = ?22,
                   updated_at = ?23
             WHERE id = ?1
            "#,
            params_from_iter(job_params(job)),
        )?;
        Ok(())
    }

    fn get_job_tx(&self, tx: &Transaction<'_>, id: &str) -> Result<Option<ScheduledJob>> {
        tx.query_row(
            r#"
            SELECT id, name, enabled, schedule_kind, schedule_expr, timezone, target_payload_json,
                   next_run_at, last_scheduled_at, last_run_at, last_success_at, run_now_requested_at,
                   max_retries, lease_token, lease_expires_at, running_since,
                   source_kind, source_actor, source_session_key, created_via, requested_by_role,
                   created_at, updated_at
              FROM scheduled_jobs
             WHERE id = ?1
            "#,
            params![id],
            read_job_row,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn resolve_create_target(req: CreateTargetRequest) -> Result<ScheduledTarget> {
    match req {
        CreateTargetRequest::Session(target) => resolve_session_target(target),
    }
}

fn resolve_session_target(target: SessionTargetRequest) -> Result<ScheduledTarget> {
    match target.requested_kind {
        RequestedTargetKind::Auto => {
            if target.agent.is_some() || !target.preconditions.is_empty() {
                Ok(ScheduledTarget::AgentTurn(AgentTurnTarget {
                    session_key: target.session_key,
                    prompt: target.prompt,
                    agent: target.agent,
                    preconditions: target.preconditions,
                }))
            } else {
                Ok(ScheduledTarget::DeliveryMessage(DeliveryMessageTarget {
                    session_key: target.session_key,
                    message: target.prompt,
                }))
            }
        }
        RequestedTargetKind::AgentTurn => Ok(ScheduledTarget::AgentTurn(AgentTurnTarget {
            session_key: target.session_key,
            prompt: target.prompt,
            agent: target.agent,
            preconditions: target.preconditions,
        })),
        RequestedTargetKind::DeliveryMessage => {
            if target.agent.is_some() {
                anyhow::bail!("delivery_message targets cannot set agent");
            }
            if !target.preconditions.is_empty() {
                anyhow::bail!("delivery_message targets cannot set preconditions");
            }
            Ok(ScheduledTarget::DeliveryMessage(DeliveryMessageTarget {
                session_key: target.session_key,
                message: target.prompt,
            }))
        }
    }
}

fn insert_job_row(conn: &Connection, job: &ScheduledJob) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO scheduled_jobs (
            id, name, enabled, schedule_kind, schedule_expr, timezone, target_payload_json,
            next_run_at, last_scheduled_at, last_run_at, last_success_at, run_now_requested_at,
            max_retries, lease_token, lease_expires_at, running_since,
            source_kind, source_actor, source_session_key, created_via, requested_by_role,
            created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
        "#,
        params_from_iter(job_params(job)),
    )?;
    Ok(())
}

fn job_params(job: &ScheduledJob) -> Vec<rusqlite::types::Value> {
    vec![
        job.id.clone().into(),
        job.name.clone().into(),
        i64::from(job.enabled).into(),
        schedule_kind_to_str(job.schedule.kind()).to_string().into(),
        schedule_expr_string(&job.schedule).into(),
        job.timezone.clone().into(),
        serde_json::to_string(&job.target).unwrap().into(),
        job.next_run_at.map(|dt| dt.to_rfc3339()).into(),
        job.last_scheduled_at.map(|dt| dt.to_rfc3339()).into(),
        job.last_run_at.map(|dt| dt.to_rfc3339()).into(),
        job.last_success_at.map(|dt| dt.to_rfc3339()).into(),
        job.run_now_requested_at.map(|dt| dt.to_rfc3339()).into(),
        i64::from(job.max_retries).into(),
        job.lease_token.clone().into(),
        job.lease_expires_at.map(|dt| dt.to_rfc3339()).into(),
        job.running_since.map(|dt| dt.to_rfc3339()).into(),
        source_kind_to_str(job.source_kind).to_string().into(),
        job.source_actor.clone().into(),
        job.source_session_key.clone().into(),
        job.created_via.clone().into(),
        job.requested_by_role.clone().into(),
        job.created_at.to_rfc3339().into(),
        job.updated_at.to_rfc3339().into(),
    ]
}

fn read_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJob> {
    let kind = schedule_kind_from_str(&row.get::<_, String>(3)?)?;
    let expr = row.get::<_, String>(4)?;
    let target_json = row.get::<_, String>(6)?;
    let target: ScheduledTarget =
        serde_json::from_str(&target_json).map_err(|e| to_sql_conversion_err(e.to_string()))?;
    Ok(ScheduledJob {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        schedule: schedule_from_parts(kind, &expr)?,
        timezone: row.get(5)?,
        target,
        next_run_at: parse_optional_datetime(row.get(7)?)?,
        last_scheduled_at: parse_optional_datetime(row.get(8)?)?,
        last_run_at: parse_optional_datetime(row.get(9)?)?,
        last_success_at: parse_optional_datetime(row.get(10)?)?,
        run_now_requested_at: parse_optional_datetime(row.get(11)?)?,
        max_retries: row.get::<_, i64>(12)? as u32,
        lease_token: row.get(13)?,
        lease_expires_at: parse_optional_datetime(row.get(14)?)?,
        running_since: parse_optional_datetime(row.get(15)?)?,
        source_kind: source_kind_from_str(&row.get::<_, String>(16)?)?,
        source_actor: row.get(17)?,
        source_session_key: row.get(18)?,
        created_via: row.get(19)?,
        requested_by_role: row.get(20)?,
        created_at: parse_datetime(row.get(21)?)?,
        updated_at: parse_datetime(row.get(22)?)?,
    })
}

fn read_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledRun> {
    Ok(ScheduledRun {
        id: row.get(0)?,
        job_id: row.get(1)?,
        scheduled_at: parse_datetime(row.get(2)?)?,
        started_at: parse_optional_datetime(row.get(3)?)?,
        finished_at: parse_optional_datetime(row.get(4)?)?,
        status: run_status_from_str(&row.get::<_, String>(5)?)?,
        attempt: row.get::<_, i64>(6)? as u32,
        error: row.get(7)?,
        result_summary: row.get(8)?,
        trigger_reason: trigger_reason_from_str(&row.get::<_, String>(9)?)?,
        executor_session_key: row.get(10)?,
        executor_agent: row.get(11)?,
    })
}

fn schedule_kind_to_str(kind: ScheduleKind) -> &'static str {
    match kind {
        ScheduleKind::Cron => "cron",
        ScheduleKind::At => "at",
        ScheduleKind::Every => "every",
    }
}

fn schedule_kind_from_str(kind: &str) -> rusqlite::Result<ScheduleKind> {
    match kind {
        "cron" => Ok(ScheduleKind::Cron),
        "at" => Ok(ScheduleKind::At),
        "every" => Ok(ScheduleKind::Every),
        other => Err(to_sql_conversion_err(format!(
            "unknown schedule_kind '{other}'"
        ))),
    }
}

fn schedule_expr_string(schedule: &ScheduleSpec) -> String {
    match schedule {
        ScheduleSpec::Cron { expr } => expr.clone(),
        ScheduleSpec::At { run_at } => run_at.to_rfc3339(),
        ScheduleSpec::Every { interval_ms } => interval_ms.to_string(),
    }
}

fn schedule_from_parts(kind: ScheduleKind, expr: &str) -> rusqlite::Result<ScheduleSpec> {
    match kind {
        ScheduleKind::Cron => Ok(ScheduleSpec::Cron {
            expr: expr.to_string(),
        }),
        ScheduleKind::At => Ok(ScheduleSpec::At {
            run_at: parse_datetime(expr.to_string())?,
        }),
        ScheduleKind::Every => Ok(ScheduleSpec::Every {
            interval_ms: expr
                .parse()
                .map_err(|e| to_sql_conversion_err(format!("bad every interval '{expr}': {e}")))?,
        }),
    }
}

fn source_kind_to_str(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::HumanCli => "human_cli",
        SourceKind::HumanChat => "human_chat",
        SourceKind::AgentTool => "agent_tool",
        SourceKind::SystemInternal => "system_internal",
    }
}

fn source_kind_from_str(kind: &str) -> rusqlite::Result<SourceKind> {
    match kind {
        "human_cli" => Ok(SourceKind::HumanCli),
        "human_chat" => Ok(SourceKind::HumanChat),
        "agent_tool" => Ok(SourceKind::AgentTool),
        "system_internal" => Ok(SourceKind::SystemInternal),
        other => Err(to_sql_conversion_err(format!(
            "unknown source_kind '{other}'"
        ))),
    }
}

fn trigger_reason_to_str(reason: TriggerReason) -> &'static str {
    match reason {
        TriggerReason::Due => "due",
        TriggerReason::RunNow => "run_now",
        TriggerReason::MisfireRecovery => "misfire_recovery",
    }
}

fn trigger_reason_from_str(reason: &str) -> rusqlite::Result<TriggerReason> {
    match reason {
        "due" => Ok(TriggerReason::Due),
        "run_now" => Ok(TriggerReason::RunNow),
        "misfire_recovery" => Ok(TriggerReason::MisfireRecovery),
        other => Err(to_sql_conversion_err(format!(
            "unknown trigger_reason '{other}'"
        ))),
    }
}

fn run_status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Skipped => "skipped",
    }
}

fn run_status_from_str(status: &str) -> rusqlite::Result<RunStatus> {
    match status {
        "running" => Ok(RunStatus::Running),
        "succeeded" => Ok(RunStatus::Succeeded),
        "failed" => Ok(RunStatus::Failed),
        "skipped" => Ok(RunStatus::Skipped),
        other => Err(to_sql_conversion_err(format!(
            "unknown run status '{other}'"
        ))),
    }
}

fn parse_datetime(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| to_sql_conversion_err(format!("invalid datetime '{raw}': {e}")))
}

fn parse_optional_datetime(raw: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    raw.map(parse_datetime).transpose()
}

fn has_live_lease(job: &ScheduledJob, now: DateTime<Utc>) -> bool {
    job.lease_expires_at.is_some_and(|at| at > now)
}

fn to_sql_conversion_err(msg: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::<dyn std::error::Error + Send + Sync>::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            msg.into(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{
        CreateTargetRequest, ExecutionPrecondition, RequestedTargetKind, ScheduleInput,
        SessionTargetRequest,
    };
    use chrono::Duration;

    fn create_req(name: &str) -> CreateJobRequest {
        CreateJobRequest {
            name: name.to_string(),
            schedule: ScheduleInput::Every {
                interval_ms: 60_000,
            },
            timezone: Some("UTC".to_string()),
            target: CreateTargetRequest::Session(SessionTargetRequest {
                requested_kind: RequestedTargetKind::AgentTurn,
                session_key: "cron:test".to_string(),
                prompt: "ping".to_string(),
                agent: Some("default".to_string()),
                preconditions: vec![ExecutionPrecondition::IdleGtSeconds {
                    threshold_seconds: 30,
                }],
            }),
            max_retries: 0,
            source_kind: SourceKind::HumanCli,
            source_actor: "tester".to_string(),
            source_session_key: Some("session:test".to_string()),
            created_via: "cli".to_string(),
            requested_by_role: Some("user".to_string()),
        }
    }

    #[test]
    fn creating_jobs_populates_next_run_and_provenance() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store.create_job(create_req("a"), now).unwrap();
        assert_eq!(job.source_kind, SourceKind::HumanCli);
        assert_eq!(job.source_actor, "tester");
        assert!(job.next_run_at.is_some());
        assert!(matches!(job.target, ScheduledTarget::AgentTurn(_)));
    }

    #[test]
    fn disabled_jobs_are_skipped_by_due_claims() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store.create_job(create_req("a"), now).unwrap();
        assert!(store.pause_job(&job.id, now).unwrap());
        let claims = store
            .claim_due_jobs(now + Duration::minutes(2), 10, 60)
            .unwrap();
        assert!(claims.is_empty());
    }

    #[test]
    fn run_now_surfaces_immediate_jobs() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store.create_job(create_req("a"), now).unwrap();
        store.request_run_now(&job.id, now).unwrap();
        let claims = store.claim_due_jobs(now, 10, 60).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].trigger_reason, TriggerReason::RunNow);
    }

    #[test]
    fn due_job_claiming_writes_lease_and_prevents_second_claim() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let _job = store.create_job(create_req("a"), now).unwrap();
        let first = store
            .claim_due_jobs(now + Duration::minutes(2), 10, 60)
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = store
            .claim_due_jobs(now + Duration::minutes(2), 10, 60)
            .unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn finish_run_records_history_and_recomputes_next_run() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store.create_job(create_req("a"), now).unwrap();
        let claim = store
            .claim_due_jobs(now + Duration::minutes(2), 10, 60)
            .unwrap()
            .remove(0);
        let run_id = store.start_run(&claim, 1).unwrap();
        let finished_at = now + Duration::minutes(2);
        let updated = store
            .finish_run(
                &claim,
                &run_id,
                RunStatus::Succeeded,
                finished_at,
                None,
                Some("ok".to_string()),
            )
            .unwrap()
            .unwrap();
        assert!(updated.next_run_at > Some(job.next_run_at.unwrap()));
        let history = store.list_run_history(Some(&job.id)).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, RunStatus::Succeeded);
    }

    #[test]
    fn one_shot_jobs_stop_scheduling_after_success() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let req = CreateJobRequest {
            schedule: ScheduleInput::At {
                run_at: now + Duration::minutes(1),
            },
            ..create_req("once")
        };
        let job = store.create_job(req, now).unwrap();
        let claim = store
            .claim_due_jobs(now + Duration::minutes(1), 10, 60)
            .unwrap()
            .remove(0);
        let run_id = store.start_run(&claim, 1).unwrap();
        let updated = store
            .finish_run(
                &claim,
                &run_id,
                RunStatus::Succeeded,
                now + Duration::minutes(1),
                None,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.id, job.id);
        assert!(updated.next_run_at.is_none());
    }

    #[test]
    fn deleting_job_also_removes_run_history() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store.create_job(create_req("delete-me"), now).unwrap();
        let claim = store
            .claim_due_jobs(now + Duration::minutes(2), 10, 60)
            .unwrap()
            .remove(0);
        let run_id = store.start_run(&claim, 1).unwrap();
        store
            .finish_run(
                &claim,
                &run_id,
                RunStatus::Succeeded,
                now + Duration::minutes(2),
                None,
                Some("done".into()),
            )
            .unwrap();
        assert!(store.delete_job(&job.id).unwrap());
        assert!(store.get_job(&job.id).unwrap().is_none());
        assert!(store.list_run_history(Some(&job.id)).unwrap().is_empty());
    }

    #[test]
    fn auto_target_without_agent_or_preconditions_becomes_delivery_message() {
        let store = SchedulerStore::in_memory().unwrap();
        let now = Utc::now();
        let job = store
            .create_job(
                CreateJobRequest {
                    name: "reminder".to_string(),
                    schedule: ScheduleInput::Delay { delay_ms: 60_000 },
                    timezone: Some("UTC".to_string()),
                    target: CreateTargetRequest::Session(SessionTargetRequest {
                        requested_kind: RequestedTargetKind::Auto,
                        session_key: "dingtalk:user:alice".to_string(),
                        prompt: "刷牙时间到了".to_string(),
                        agent: None,
                        preconditions: vec![],
                    }),
                    max_retries: 0,
                    source_kind: SourceKind::HumanCli,
                    source_actor: "tester".to_string(),
                    source_session_key: None,
                    created_via: "cli".to_string(),
                    requested_by_role: Some("user".to_string()),
                },
                now,
            )
            .unwrap();
        assert!(matches!(job.target, ScheduledTarget::DeliveryMessage(_)));
    }
}
