//! TaskRegistry — SQLite 任务状态机
//!
//! SQLite 是唯一真相源（single source of truth）。
//! TASKS.md 是定期从 SQLite 导出的只读快照（由 TeamSession::write_tasks_md 调用）。
//!
//! 状态机：Pending → Claimed → Submitted → Accepted
//! 兼容旧路径：Pending → Claimed → Done / Failed
//!   - Pending  → Claimed  : try_claim()（乐观锁，原子 UPDATE）
//!   - Claimed  → Pending  : reset_claim()（超时后由 Heartbeat 重置）
//!   - Claimed  → Done     : mark_done()
//!   - Claimed  → Submitted: submit_task_result()
//!   - Submitted → Accepted: accept_task()
//!   - Submitted/Accepted/Done → Pending: reopen_task()
//!   - Claimed  → Failed   : mark_failed()（retry_count >= 3）

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

// ─── 类型 ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Pending,
    Claimed { agent: String, at: DateTime<Utc> },
    Submitted { agent: String, at: DateTime<Utc> },
    Accepted { by: String, at: DateTime<Utc> },
    Done,
    Failed(String),
    Retrying(u32),
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// 编码为字符串：
    /// "pending" |
    /// "claimed:{agent}:{iso8601}" |
    /// "submitted:{agent}:{iso8601}" |
    /// "accepted:{by}:{iso8601}" |
    /// "done" |
    /// "failed:{msg}" |
    /// "retrying:{n}"
    pub status_raw: String,
    /// JSON 数组 ["T001", "T002"]
    pub deps_json: String,
    pub assignee_hint: Option<String>,
    pub retry_count: i32,
    pub timeout_secs: i32,
    pub spec: Option<String>,
    pub success_criteria: Option<String>,
    pub completion_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub done_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn status_parsed(&self) -> TaskStatus {
        let s = &self.status_raw;
        if s == "pending" {
            TaskStatus::Pending
        } else if s == "done" {
            TaskStatus::Done
        } else if let Some(rest) = s.strip_prefix("submitted:") {
            let mut parts = rest.splitn(2, ':');
            let agent = parts.next().unwrap_or("unknown").to_string();
            let at = parts
                .next()
                .and_then(|ts| ts.parse::<DateTime<Utc>>().ok())
                .unwrap_or_else(Utc::now);
            TaskStatus::Submitted { agent, at }
        } else if let Some(rest) = s.strip_prefix("accepted:") {
            let mut parts = rest.splitn(2, ':');
            let by = parts.next().unwrap_or("leader").to_string();
            let at = parts
                .next()
                .and_then(|ts| ts.parse::<DateTime<Utc>>().ok())
                .unwrap_or_else(Utc::now);
            TaskStatus::Accepted { by, at }
        } else if let Some(rest) = s.strip_prefix("claimed:") {
            // "claimed:codex:2026-03-05T10:00:00Z"
            let mut parts = rest.splitn(2, ':');
            let agent = parts.next().unwrap_or("unknown").to_string();
            let at = parts
                .next()
                .and_then(|ts| ts.parse::<DateTime<Utc>>().ok())
                .unwrap_or_else(Utc::now);
            TaskStatus::Claimed { agent, at }
        } else if let Some(msg) = s.strip_prefix("failed:") {
            TaskStatus::Failed(msg.to_string())
        } else if let Some(n) = s.strip_prefix("retrying:") {
            TaskStatus::Retrying(n.parse().unwrap_or(0))
        } else {
            TaskStatus::Pending
        }
    }

    pub fn deps(&self) -> Vec<String> {
        serde_json::from_str::<Vec<String>>(&self.deps_json).unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct CreateTask {
    pub id: String,
    pub title: String,
    pub assignee_hint: Option<String>,
    pub deps: Vec<String>,
    pub timeout_secs: i32,
    pub spec: Option<String>,
    pub success_criteria: Option<String>,
}

// ─── TaskRegistry ────────────────────────────────────────────────────────────

pub struct TaskRegistry {
    conn: Arc<Mutex<Connection>>,
}

impl TaskRegistry {
    /// 打开（或创建）指定路径的 SQLite 数据库
    pub fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open task db: {}", db_path))?;
        let registry = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        registry.migrate()?;
        Ok(registry)
    }

    /// 内存数据库（用于测试）
    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory task db")?;
        let registry = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        registry.migrate()?;
        Ok(registry)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id               TEXT PRIMARY KEY,
                title            TEXT NOT NULL,
                status           TEXT NOT NULL DEFAULT 'pending',
                deps_json        TEXT NOT NULL DEFAULT '[]',
                assignee_hint    TEXT,
                retry_count      INTEGER NOT NULL DEFAULT 0,
                timeout_secs     INTEGER NOT NULL DEFAULT 1800,
                spec             TEXT,
                success_criteria TEXT,
                completion_note  TEXT,
                created_at       TEXT NOT NULL,
                done_at          TEXT
            );",
        )
        .context("task table migration failed")
    }

    /// 创建任务（INSERT OR IGNORE，幂等）
    pub fn create_task(&self, input: CreateTask) -> Result<String> {
        let deps_json = serde_json::to_string(&input.deps)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO tasks
             (id, title, status, deps_json, assignee_hint, timeout_secs, spec, success_criteria, created_at)
             VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                input.id,
                input.title,
                deps_json,
                input.assignee_hint,
                input.timeout_secs,
                input.spec,
                input.success_criteria,
                now,
            ],
        )?;
        Ok(input.id)
    }

    /// 原子认领任务（乐观锁：只有 status='pending' 时才能认领）
    /// 返回 true 表示认领成功，false 表示已被他人认领
    pub fn try_claim(&self, task_id: &str, agent: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let status = format!("claimed:{}:{}", agent, now);
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE tasks SET status = ?1, retry_count = retry_count + 1
             WHERE id = ?2 AND status = 'pending'",
            params![status, task_id],
        )?;
        Ok(rows == 1)
    }

    /// 标记任务完成（仅允许从 claimed 状态转换，且需校验认领者身份）
    ///
    /// `agent` — 声明完成的执行者名称。必须与 claimed:agent:ts 中的 agent 匹配。
    pub fn mark_done(&self, task_id: &str, agent: &str, note: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        // Build the expected status prefix "claimed:<agent>:"
        let claimed_prefix = format!("claimed:{}:", agent);
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE tasks SET status = 'done', completion_note = ?1, done_at = ?2 \
             WHERE id = ?3 AND status LIKE ?4",
            params![note, now, task_id, format!("{}%", claimed_prefix)],
        )?;
        if rows == 0 {
            // Diagnose: task not found, or claimed by another agent
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM tasks WHERE id = ?1",
                    params![task_id],
                    |r| r.get(0),
                )
                .ok();
            match status {
                None => anyhow::bail!("mark_done: task '{}' not found", task_id),
                Some(s) if s.starts_with("claimed:") => {
                    anyhow::bail!(
                        "mark_done: task '{}' was claimed by another agent (status: {}), cannot be completed by '{}'",
                        task_id, s, agent
                    )
                }
                Some(s) => anyhow::bail!(
                    "mark_done: task '{}' not in claimed state (current: {})",
                    task_id,
                    s
                ),
            }
        }
        Ok(())
    }

    /// 提交任务结果（新语义）：从 claimed -> submitted，等待 Lead 验收。
    pub fn submit_task_result(&self, task_id: &str, agent: &str, summary: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let claimed_prefix = format!("claimed:{}:", agent);
        let submitted = format!("submitted:{}:{}", agent, now);
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE tasks SET status = ?1, completion_note = ?2 \
             WHERE id = ?3 AND status LIKE ?4",
            params![submitted, summary, task_id, format!("{}%", claimed_prefix)],
        )?;
        if rows == 0 {
            anyhow::bail!(
                "submit_task_result: task '{}' not claimed by '{}'",
                task_id,
                agent
            );
        }
        Ok(())
    }

    /// 验收任务结果（新语义）：从 submitted -> accepted。
    pub fn accept_task(&self, task_id: &str, by: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let accepted = format!("accepted:{}:{}", by, now);
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE tasks SET status = ?1, done_at = ?2 \
             WHERE id = ?3 AND status LIKE 'submitted:%'",
            params![accepted, now, task_id],
        )?;
        if rows == 0 {
            anyhow::bail!("accept_task: task '{}' not in submitted state", task_id);
        }
        Ok(())
    }

    /// 重新打开任务（新语义）：将 submitted / accepted / done 退回 pending。
    pub fn reopen_task(&self, task_id: &str, reason: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE tasks
             SET status = 'pending', done_at = NULL, completion_note = COALESCE(completion_note, '') || ?2
             WHERE id = ?1 AND (
                 status LIKE 'submitted:%' OR
                 status LIKE 'accepted:%' OR
                 status = 'done'
             )",
            params![task_id, format!("\n[REOPEN] {}", reason)],
        )?;
        if rows == 0 {
            anyhow::bail!(
                "reopen_task: task '{}' not in submitted/accepted/done state",
                task_id
            );
        }
        Ok(())
    }

    /// Return all tasks ordered by creation time (for get_task_status snapshot).
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note,
                    created_at, done_at
             FROM tasks ORDER BY created_at ASC",
        )?;
        Self::rows_to_tasks(&mut stmt)
    }

    /// Re-assign a task to a new agent. Only valid when status = 'pending'.
    pub fn reassign_task(&self, task_id: &str, new_assignee: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows_changed = conn.execute(
            "UPDATE tasks SET assignee_hint = ?1 WHERE id = ?2 AND status = 'pending'",
            rusqlite::params![new_assignee, task_id],
        )?;
        if rows_changed == 0 {
            anyhow::bail!("Task {} not found or not in pending state", task_id);
        }
        Ok(())
    }

    /// 标记任务失败
    pub fn mark_failed(&self, task_id: &str, error: &str) -> Result<()> {
        let msg = format!("failed:{}", error);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1 WHERE id = ?2",
            params![msg, task_id],
        )?;
        Ok(())
    }

    /// Returns true if task is currently claimed by `agent`.
    /// The status format is `claimed:{agent}:{timestamp}`.
    pub fn is_claimed_by(&self, task_id: &str, agent: &str) -> Result<bool> {
        let expected_prefix = format!("claimed:{}:", agent);
        let conn = self.conn.lock().unwrap();
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM tasks WHERE id = ?1",
                params![task_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(status
            .map(|s| s.starts_with(&expected_prefix))
            .unwrap_or(false))
    }

    /// 重置超时任务（Pending，retry_count 已在 try_claim 时递增）
    pub fn reset_claim(&self, task_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'pending' WHERE id = ?1 AND status LIKE 'claimed%'",
            params![task_id],
        )?;
        Ok(())
    }

    /// 查找所有 Pending 且依赖全部完成的任务（可派发的任务）
    ///
    /// # Concurrency Safety
    ///
    /// Although the two SQL queries are not inside a single `BEGIN TRANSACTION`,
    /// they are safe: both execute while holding the same `Mutex<Connection>` guard
    /// (`conn = self.conn.lock().unwrap()`). Every other writer (mark_done, try_claim,
    /// reset_claim) also requires this mutex, so no modification can occur between
    /// the two queries. The Mutex provides equivalent isolation to a read transaction
    /// in this single-process, single-connection setup.
    pub fn find_ready_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note, created_at, done_at
             FROM tasks WHERE status = 'pending'",
        )?;
        let all_pending = Self::rows_to_tasks(&mut stmt)?;

        // 获取所有已完成任务 ID（在同一 conn guard 下，与上面查询等价于单事务）
        let mut done_stmt =
            conn.prepare("SELECT id FROM tasks WHERE status = 'done' OR status LIKE 'accepted:%'")?;
        let terminal_ids: Vec<String> = done_stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // 过滤：deps 都在 done_ids 中
        let ready = all_pending
            .into_iter()
            .filter(|t| {
                let deps = t.deps();
                deps.is_empty() || deps.iter().all(|d| terminal_ids.contains(d))
            })
            .collect();

        Ok(ready)
    }

    /// 查找超时的 Claimed 任务
    pub fn find_stale_claimed(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note, created_at, done_at
             FROM tasks WHERE status LIKE 'claimed:%'",
        )?;
        let claimed = Self::rows_to_tasks(&mut stmt)?;

        let now = Utc::now();
        let stale = claimed
            .into_iter()
            .filter(|t| {
                if let TaskStatus::Claimed { at, .. } = t.status_parsed() {
                    let elapsed = now.signed_duration_since(at).num_seconds();
                    elapsed > t.timeout_secs as i64
                } else {
                    false
                }
            })
            .collect();

        Ok(stale)
    }

    /// 获取单个任务
    pub fn get_task(&self, task_id: &str) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note, created_at, done_at
             FROM tasks WHERE id = ?1",
        )?;
        let mut tasks = Self::rows_to_tasks_with_params(&mut stmt, params![task_id])?;
        Ok(tasks.pop())
    }

    /// 导出 TASKS.md 快照（人类可读，Specialist 通过 task_reminder 获取任务，此文件为调试用）
    pub fn export_tasks_md(&self) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, assignee_hint, retry_count, done_at
             FROM tasks ORDER BY created_at",
        )?;

        let mut lines = vec!["# Team Tasks\n".to_string()];
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i32>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .for_each(|(id, title, status, assignee, retries, done_at)| {
            let icon = if status == "done" || status.starts_with("accepted:") {
                "✅"
            } else if status.starts_with("submitted:") {
                "📨"
            } else if status.starts_with("failed") {
                "❌"
            } else if status.starts_with("claimed") {
                "🔄"
            } else {
                "⏳"
            };
            let assignee_str = assignee.map(|a| format!(" [@{}]", a)).unwrap_or_default();
            let retry_str = if retries > 0 {
                format!(" (retry: {})", retries)
            } else {
                String::new()
            };
            let done_str = done_at.map(|d| format!(" ✓ {}", d)).unwrap_or_default();
            lines.push(format!(
                "- {} **{}** — {}{}{}{}",
                icon, id, title, assignee_str, retry_str, done_str
            ));
        });

        Ok(lines.join("\n"))
    }

    /// 检查是否所有任务都已完成
    ///
    /// 若任务表为空（尚未注册任何任务），返回 false（防止误广播 all_done 里程碑）。
    pub fn all_done(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
        if total == 0 {
            return Ok(false);
        }
        let not_done: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status != 'done' AND status NOT LIKE 'accepted:%'",
            [],
            |row| row.get(0),
        )?;
        Ok(not_done == 0)
    }

    /// 获取所有任务（用于 TeamOrchestrator 生成摘要）
    pub fn list_all(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, deps_json, assignee_hint, retry_count,
                    timeout_secs, spec, success_criteria, completion_note, created_at, done_at
             FROM tasks ORDER BY created_at",
        )?;
        Self::rows_to_tasks(&mut stmt)
    }

    /// 查找所有被 given_id 阻塞的任务 ID
    pub fn tasks_blocked_by(&self, given_id: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            match conn.prepare("SELECT id, deps_json FROM tasks WHERE status = 'pending'") {
                Ok(s) => s,
                Err(_) => return vec![],
            };
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .filter_map(|(id, deps_json)| {
            let deps: Vec<String> = serde_json::from_str(&deps_json).unwrap_or_default();
            if deps.contains(&given_id.to_string()) {
                Some(id)
            } else {
                None
            }
        })
        .collect()
    }

    // ── 内部辅助 ───────────────────────────────────────────────────────────

    fn rows_to_tasks(stmt: &mut rusqlite::Statement<'_>) -> Result<Vec<Task>> {
        Self::rows_to_tasks_with_params(stmt, [])
    }

    fn rows_to_tasks_with_params(
        stmt: &mut rusqlite::Statement<'_>,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Task>> {
        let tasks = stmt
            .query_map(params, |row| {
                let created_str: String = row.get(10)?;
                let done_str: Option<String> = row.get(11)?;
                let created_at = created_str
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                let done_at = done_str.and_then(|s| s.parse::<DateTime<Utc>>().ok());
                Ok(Task {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    status_raw: row.get(2)?,
                    deps_json: row.get(3)?,
                    assignee_hint: row.get(4)?,
                    retry_count: row.get(5)?,
                    timeout_secs: row.get(6)?,
                    spec: row.get(7)?,
                    success_criteria: row.get(8)?,
                    completion_note: row.get(9)?,
                    created_at,
                    done_at,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(tasks)
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_claim_task() {
        let registry = TaskRegistry::new_in_memory().unwrap();

        let task_id = registry
            .create_task(CreateTask {
                id: "T001".to_string(),
                title: "Define AuthToken struct".to_string(),
                assignee_hint: None,
                deps: vec![],
                timeout_secs: 1800,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(task_id, "T001");

        // 认领成功
        let claimed = registry.try_claim("T001", "codex").unwrap();
        assert!(claimed, "should successfully claim T001");

        // 重复认领失败（乐观锁）
        let claimed2 = registry.try_claim("T001", "claude").unwrap();
        assert!(!claimed2, "T001 already claimed, should fail");

        // 完成任务（传入认领者 "codex"）
        registry
            .mark_done("T001", "codex", "created src/auth/jwt.rs")
            .unwrap();
        let task = registry.get_task("T001").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Done));
    }

    #[test]
    fn test_submit_accept_and_reopen_flow() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T010".into(),
                title: "Implement JWT middleware".into(),
                ..Default::default()
            })
            .unwrap();

        registry.try_claim("T010", "codex").unwrap();
        registry
            .submit_task_result("T010", "codex", "added jwt.rs and tests")
            .unwrap();
        let submitted = registry.get_task("T010").unwrap().unwrap();
        assert!(matches!(
            submitted.status_parsed(),
            TaskStatus::Submitted { ref agent, .. } if agent == "codex"
        ));
        assert_eq!(
            submitted.completion_note.as_deref(),
            Some("added jwt.rs and tests")
        );

        registry.accept_task("T010", "claude").unwrap();
        let accepted = registry.get_task("T010").unwrap().unwrap();
        assert!(matches!(
            accepted.status_parsed(),
            TaskStatus::Accepted { ref by, .. } if by == "claude"
        ));
        assert!(accepted.done_at.is_some());
        assert!(registry.all_done().unwrap());

        registry.reopen_task("T010", "needs edge-case fix").unwrap();
        let reopened = registry.get_task("T010").unwrap().unwrap();
        assert!(matches!(reopened.status_parsed(), TaskStatus::Pending));
        assert!(reopened.done_at.is_none());
        assert!(reopened
            .completion_note
            .as_deref()
            .unwrap_or("")
            .contains("[REOPEN] needs edge-case fix"));
    }

    #[test]
    fn test_accepted_dependency_unblocks_downstream_tasks() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T011".into(),
                title: "base".into(),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "T012".into(),
                title: "dependent".into(),
                deps: vec!["T011".into()],
                ..Default::default()
            })
            .unwrap();

        registry.try_claim("T011", "codex").unwrap();
        registry
            .submit_task_result("T011", "codex", "ready for review")
            .unwrap();
        let ready_before_accept = registry.find_ready_tasks().unwrap();
        assert!(!ready_before_accept.iter().any(|t| t.id == "T012"));

        registry.accept_task("T011", "leader").unwrap();
        let ready_after_accept = registry.find_ready_tasks().unwrap();
        assert!(ready_after_accept.iter().any(|t| t.id == "T012"));
    }

    #[test]
    fn test_idempotent_create() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "first".into(),
                ..Default::default()
            })
            .unwrap();
        // 重复创建不报错（INSERT OR IGNORE）
        let result = registry.create_task(CreateTask {
            id: "T001".into(),
            title: "second".into(),
            ..Default::default()
        });
        assert!(result.is_ok());
        // 原始 title 保留
        let task = registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.title, "first");
    }

    #[test]
    fn test_deps_gate() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "base".into(),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "T002".into(),
                title: "dependent".into(),
                deps: vec!["T001".to_string()],
                ..Default::default()
            })
            .unwrap();

        // T001 未完成，T002 不可认领
        let ready = registry.find_ready_tasks().unwrap();
        assert!(
            !ready.iter().any(|t| t.id == "T002"),
            "T002 blocked by T001"
        );
        assert!(ready.iter().any(|t| t.id == "T001"), "T001 should be ready");

        // T001 完成后，T002 可认领
        registry.try_claim("T001", "claude").unwrap();
        registry.mark_done("T001", "claude", "done").unwrap();
        let ready2 = registry.find_ready_tasks().unwrap();
        assert!(ready2.iter().any(|t| t.id == "T002"), "T002 now unblocked");
    }

    #[test]
    fn test_stale_claimed_detection() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "stale test".into(),
                timeout_secs: -1, // 负数 → 立即超时
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T001", "codex").unwrap();

        let stale = registry.find_stale_claimed().unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "T001");

        registry.reset_claim("T001").unwrap();
        let task = registry.get_task("T001").unwrap().unwrap();
        assert!(matches!(task.status_parsed(), TaskStatus::Pending));
    }

    #[test]
    fn test_export_tasks_md() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "Setup DB".into(),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T001", "codex").unwrap();
        registry.mark_done("T001", "codex", "done").unwrap();

        let md = registry.export_tasks_md().unwrap();
        assert!(md.contains("T001"));
        assert!(md.contains("Setup DB"));
        assert!(md.contains("✅"));
    }

    #[test]
    fn test_tasks_blocked_by() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "base".into(),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "T002".into(),
                title: "downstream".into(),
                deps: vec!["T001".to_string()],
                ..Default::default()
            })
            .unwrap();

        let blocked = registry.tasks_blocked_by("T001");
        assert!(blocked.contains(&"T002".to_string()));
    }

    #[test]
    fn test_all_done_empty_table_returns_false() {
        // 空表不应误触发 all_done 里程碑
        let registry = TaskRegistry::new_in_memory().unwrap();
        assert!(!registry.all_done().unwrap());
    }

    #[test]
    fn test_all_tasks_returns_all() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "A".into(),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "T002".into(),
                title: "B".into(),
                ..Default::default()
            })
            .unwrap();
        let tasks = registry.all_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "T001");
        assert_eq!(tasks[1].id, "T002");
    }

    #[test]
    fn test_reassign_task_pending_only() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "A".into(),
                ..Default::default()
            })
            .unwrap();
        // Should succeed for pending task
        registry.reassign_task("T001", "claude").unwrap();
        let task = registry.get_task("T001").unwrap().unwrap();
        assert_eq!(task.assignee_hint.as_deref(), Some("claude"));
        // Should fail for claimed task
        registry.try_claim("T001", "codex").unwrap();
        let result = registry.reassign_task("T001", "gemini");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_claimed_by_returns_true_for_current_claimer() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T1".into(),
                title: "t".into(),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T1", "alice").unwrap();
        assert!(registry.is_claimed_by("T1", "alice").unwrap());
        assert!(!registry.is_claimed_by("T1", "bob").unwrap());
    }

    #[test]
    fn test_is_claimed_by_returns_false_for_pending_task() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T2".into(),
                title: "t".into(),
                ..Default::default()
            })
            .unwrap();
        // Task is pending (not claimed), so is_claimed_by must return false
        assert!(!registry.is_claimed_by("T2", "alice").unwrap());
    }

    // ─── 功能测试：状态机完整路径 ─────────────────────────────────────────────

    /// 验证 Pending → Claimed → Submitted → Accepted 完整路径
    /// 每个中间状态都显式断言，确保无状态跳跃
    #[test]
    fn test_full_state_machine_path_pending_to_accepted() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "SM01".into(),
                title: "Auth flow".into(),
                ..Default::default()
            })
            .unwrap();

        // Pending
        let t = registry.get_task("SM01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Pending),
            "initial state must be Pending"
        );

        // Claimed
        assert!(registry.try_claim("SM01", "codex").unwrap());
        let t = registry.get_task("SM01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Claimed { ref agent, .. } if agent == "codex"),
            "after claim, state must be Claimed by codex"
        );

        // Submitted
        registry
            .submit_task_result("SM01", "codex", "impl done")
            .unwrap();
        let t = registry.get_task("SM01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Submitted { ref agent, .. } if agent == "codex"),
            "after submit, state must be Submitted"
        );
        assert_eq!(t.completion_note.as_deref(), Some("impl done"));

        // Accepted
        registry.accept_task("SM01", "lead").unwrap();
        let t = registry.get_task("SM01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Accepted { ref by, .. } if by == "lead"),
            "after accept, state must be Accepted by lead"
        );
        assert!(t.done_at.is_some(), "done_at must be set after acceptance");
        assert!(
            registry.all_done().unwrap(),
            "all_done() must return true when only task is Accepted"
        );
    }

    /// 验证非法状态转换被拒绝：对 Pending 任务调用 submit/accept 应报错
    #[test]
    fn test_invalid_state_transition_rejected() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "INV01".into(),
                title: "test invalid transitions".into(),
                ..Default::default()
            })
            .unwrap();

        // submit on Pending (not Claimed) must fail
        let err = registry.submit_task_result("INV01", "codex", "premature submit");
        assert!(
            err.is_err(),
            "submit on Pending task must return Err, not Ok"
        );

        // Task state must be unchanged after failed submit
        let t = registry.get_task("INV01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Pending),
            "task must still be Pending after failed submit, got: {}",
            t.status_raw
        );

        // accept on Pending (not Submitted) must fail
        let err2 = registry.accept_task("INV01", "lead");
        assert!(
            err2.is_err(),
            "accept on Pending task must return Err, not Ok"
        );

        // Claim then try accept without submit — accept on Claimed must also fail
        registry.try_claim("INV01", "codex").unwrap();
        let err3 = registry.accept_task("INV01", "lead");
        assert!(
            err3.is_err(),
            "accept on Claimed (not Submitted) task must return Err"
        );

        let t = registry.get_task("INV01").unwrap().unwrap();
        assert!(
            matches!(t.status_parsed(), TaskStatus::Claimed { .. }),
            "task must still be Claimed after failed accept, got: {}",
            t.status_raw
        );
    }

    /// 验证依赖链：T_A done → T_B 解锁；T_B done → all_done
    #[test]
    fn test_dep_chain_unlocks_sequentially() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "DA01".into(),
                title: "setup DB".into(),
                assignee_hint: Some("codex".into()),
                ..Default::default()
            })
            .unwrap();
        registry
            .create_task(CreateTask {
                id: "DA02".into(),
                title: "seed data".into(),
                assignee_hint: Some("claude".into()),
                deps: vec!["DA01".into()],
                ..Default::default()
            })
            .unwrap();

        // Before DA01 done: DA02 must not be in ready list
        let ready = registry.find_ready_tasks().unwrap();
        assert!(
            !ready.iter().any(|t| t.id == "DA02"),
            "DA02 must be blocked while DA01 is pending"
        );
        assert!(
            ready.iter().any(|t| t.id == "DA01"),
            "DA01 must be ready initially"
        );

        // Complete DA01 (old direct-done path)
        registry.try_claim("DA01", "codex").unwrap();
        registry
            .mark_done("DA01", "codex", "schema created")
            .unwrap();

        // After DA01 done: DA02 must be in ready list
        let ready2 = registry.find_ready_tasks().unwrap();
        assert!(
            ready2.iter().any(|t| t.id == "DA02"),
            "DA02 must be unlocked after DA01 done"
        );

        // all_done must still be false
        assert!(
            !registry.all_done().unwrap(),
            "all_done must be false while DA02 is pending"
        );

        // Complete DA02
        registry.try_claim("DA02", "claude").unwrap();
        registry.mark_done("DA02", "claude", "data seeded").unwrap();

        assert!(
            registry.all_done().unwrap(),
            "all_done must be true after both tasks done"
        );
    }
}
