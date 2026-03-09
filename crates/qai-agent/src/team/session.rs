//! TeamSession — Team Mode 的目录结构与工具函数
//!
//! 目录布局：
//!   ~/.quickai/team-sessions/{group_hash}-{team_id}/
//!     TEAM.md        — 团队职责宣言（Lead 在 /team start 时写入）
//!     CONTEXT.md     — 任务背景（Lead 维护，注入 Specialist 的 shared_memory）
//!     TASKS.md       — 任务快照（由 TaskRegistry::export_tasks_md 导出，只读）
//!     HEARTBEAT.md   — 可选的团队心跳检查清单（统一 context contract）
//!     events.jsonl   — 事件日志（调试用）

use anyhow::{Context, Result};
use qai_protocol::SessionKey;
use serde::Serialize;
use std::path::PathBuf;

use super::registry::{Task, TaskRegistry, TaskStatus};

#[derive(Debug, Clone, Serialize)]
pub struct TaskArtifactMeta {
    pub id: String,
    pub title: String,
    pub assignee_hint: Option<String>,
    pub status: String,
    pub deps: Vec<String>,
    pub success_criteria: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub done_at: Option<String>,
    pub claimed_by: Option<String>,
    pub submitted_by: Option<String>,
    pub accepted_by: Option<String>,
}

impl TaskArtifactMeta {
    pub fn from_task(task: &Task) -> Self {
        let (claimed_by, submitted_by, accepted_by, updated_at) = match task.status_parsed() {
            TaskStatus::Claimed { agent, at } => {
                (Some(agent), None, None, at.to_rfc3339())
            }
            TaskStatus::Submitted { agent, at } => {
                (None, Some(agent), None, at.to_rfc3339())
            }
            TaskStatus::Accepted { by, at } => {
                (None, None, Some(by), at.to_rfc3339())
            }
            TaskStatus::Done => (
                None,
                None,
                None,
                task.done_at
                    .as_ref()
                    .map(chrono::DateTime::to_rfc3339)
                    .unwrap_or_else(|| task.created_at.to_rfc3339()),
            ),
            _ => (
                None,
                None,
                None,
                task.created_at.to_rfc3339(),
            ),
        };

        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            assignee_hint: task.assignee_hint.clone(),
            status: task.status_raw.clone(),
            deps: task.deps(),
            success_criteria: task.success_criteria.clone(),
            created_at: task.created_at.to_rfc3339(),
            updated_at,
            done_at: task.done_at.as_ref().map(chrono::DateTime::to_rfc3339),
            claimed_by,
            submitted_by,
            accepted_by,
        }
    }
}

pub struct TeamSession {
    pub team_id: String,
    pub dir: PathBuf,
}

impl TeamSession {
    /// 创建 TeamSession（自动创建目录）
    ///
    /// `group_scope` — 群组 SessionKey 的 scope 字段（如 "group:oc_xxx"）
    /// `team_id`     — 本次团队协作的唯一 ID（UUID）
    pub fn new(group_scope: &str, team_id: &str) -> Result<Self> {
        // 将 group_scope 中的特殊字符替换为连字符，用于目录名
        let safe_scope: String = group_scope
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".quickai")
            .join("team-sessions")
            .join(format!("{}-{}", safe_scope, team_id));

        std::fs::create_dir_all(&base)
            .with_context(|| format!("Failed to create team session dir: {}", base.display()))?;

        Ok(Self {
            team_id: team_id.to_string(),
            dir: base,
        })
    }

    /// 从已有路径恢复 TeamSession（不创建目录）
    pub fn from_dir(team_id: &str, dir: PathBuf) -> Self {
        Self {
            team_id: team_id.to_string(),
            dir,
        }
    }

    // ── 派生 SessionKey ──────────────────────────────────────────────────────

    /// 生成 Specialist 的隔离 SessionKey
    ///
    /// 专才使用独立的 SessionKey，与主线群组 key 不同：
    ///   - 独立的 LaneQueue → 并行执行（不堵塞主线）
    ///   - 独立的 session history → 不包含群组噪音
    ///   - workspace_dir 指向 team-session 目录 → 读 CONTEXT.md/TASKS.md
    pub fn specialist_session_key(&self, agent_name: &str) -> SessionKey {
        // Uses "specialist" as the channel name — an internal routing identifier,
        // NOT an IM channel. Only lark/dingtalk are real IM channels.
        SessionKey::new("specialist", format!("{}:{}", self.team_id, agent_name))
    }

    // ── 读写文件 ─────────────────────────────────────────────────────────────

    pub fn write_team_md(&self, content: &str) -> Result<()> {
        self.write_file("TEAM.md", content)
    }

    pub fn write_context_md(&self, content: &str) -> Result<()> {
        self.write_file("CONTEXT.md", content)
    }

    pub fn write_agents_md(&self, content: &str) -> Result<()> {
        self.write_file("AGENTS.md", content)
    }

    pub fn write_heartbeat_md(&self, content: &str) -> Result<()> {
        self.write_file("HEARTBEAT.md", content)
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.dir.join("tasks")
    }

    pub fn task_dir(&self, task_id: &str) -> PathBuf {
        self.tasks_dir().join(task_id)
    }

    pub fn write_task_meta(&self, task_id: &str, meta: &TaskArtifactMeta) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        let body = serde_json::to_string_pretty(meta)?;
        self.write_task_file(task_id, "meta.json", &body)
    }

    pub fn write_task_spec(&self, task_id: &str, content: &str) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        self.write_task_file(task_id, "spec.md", content)
    }

    pub fn append_task_progress(&self, task_id: &str, content: &str) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        self.append_task_file(task_id, "progress.md", content)
    }

    pub fn write_task_result(&self, task_id: &str, content: &str) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        self.write_task_file(task_id, "result.md", content)
    }

    /// 从 TaskRegistry 导出任务快照到 TASKS.md
    pub fn sync_tasks_md(&self, registry: &TaskRegistry) -> Result<()> {
        let md = registry.export_tasks_md()?;
        self.write_file("TASKS.md", &md)
    }

    pub fn read_team_md(&self) -> String {
        self.read_file("TEAM.md")
    }

    /// 读取 CONTEXT.md（注入 Specialist 的 shared_memory）
    pub fn read_context_md(&self) -> String {
        self.read_file("CONTEXT.md")
    }

    pub fn read_tasks_md(&self) -> String {
        self.read_file("TASKS.md")
    }

    pub fn read_heartbeat_md(&self) -> String {
        self.read_file("HEARTBEAT.md")
    }

    /// 追加事件日志（JSONL 格式，调试用）
    pub fn append_event(&self, event: &str) -> Result<()> {
        use std::io::Write;
        let path = self.dir.join("events.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", event)?;
        Ok(())
    }

    /// Log a Specialist's reply text to events.jsonl (for observability/debugging).
    pub fn append_specialist_reply(&self, agent: &str, task_id: &str, reply: &str) -> Result<()> {
        // Use serde_json to safely escape all string fields (prevents JSON injection via reply text)
        let event = serde_json::json!({
            "event": "SPECIALIST_REPLY",
            "agent": agent,
            "task": task_id,
            "ts": chrono::Utc::now().to_rfc3339(),
            "text": reply,
        })
        .to_string();
        self.append_event(&event)
    }

    // ── task_reminder 构建 ───────────────────────────────────────────────────

    /// 构建注入 Specialist system prompt Layer 0 的任务提醒文本
    ///
    /// 对齐 hiClaw spec.md 格式：完整任务说明 + 成功标准 + 必须遵守的协议。
    pub fn build_task_reminder(&self, task: &Task, registry: &TaskRegistry) -> String {
        let blocking = registry.tasks_blocked_by(&task.id);
        let deps = task.deps();

        let deps_str = if deps.is_empty() {
            "无".to_string()
        } else {
            deps.join(", ")
        };
        let blocking_str = if blocking.is_empty() {
            "无".to_string()
        } else {
            blocking.join(", ")
        };

        // Collect upstream completion notes for completed dependency tasks
        let upstream_notes: Vec<String> = deps
            .iter()
            .filter_map(|dep_id| {
                registry.get_task(dep_id).ok().flatten().and_then(|t| {
                    t.completion_note.as_ref().map(|note| {
                        format!(
                            "[{}] {}（{}，已完成）：\n{}",
                            dep_id,
                            t.title,
                            t.assignee_hint.as_deref().unwrap_or("unknown"),
                            note
                        )
                    })
                })
            })
            .collect();

        let upstream_section = if upstream_notes.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n── 上游任务结果 ──────────────────────────\n{}\n─────────────────────────────────────────",
                upstream_notes.join("\n\n")
            )
        };

        format!(
            "══════ 当前任务（自动注入，最高优先级）══════\n\
             任务ID: {id}\n\
             标题: {title}\n\
             详细说明: {spec}\n\
             依赖（已完成）: {deps}\n\
             被阻塞的下游任务: {blocking}\n\
             \n\
             ── 成功标准 ──\n\
             {criteria}\n\
             \n\
             ── 必须遵守 ──\n\
             1. 阶段性进展可调用 `checkpoint_task(task_id, note)` 向 Lead 发送检查点\n\
             2. 完成任务后优先调用 `submit_task_result(task_id, summary)`，等待 Lead 验收\n\
             3. 遇到阻塞时调用 `block_task(task_id, reason)` 释放任务并上报；仅需协助时调用 `request_help(task_id, message)`，保留 claim\n\
             4. 兼容旧路径时仍可调用 `complete_task(task_id, note)`，但新语义优先使用 submit_task_result\n\
             5. 重要产出（文件路径、关键发现）写在 summary / note 参数中\n\
             ══════════════════════════════════════════{upstream_section}",
            id = task.id,
            title = task.title,
            spec = task.spec.as_deref().unwrap_or("（无详细说明）"),
            deps = deps_str,
            blocking = blocking_str,
            criteria = task
                .success_criteria
                .as_deref()
                .unwrap_or("完成任务说明中描述的工作"),
            upstream_section = upstream_section,
        )
    }

    // ── 归档 ────────────────────────────────────────────────────────────────

    /// 归档 team session（移动到 archived/ 子目录）
    pub fn archive(&self) -> Result<()> {
        let archived = self
            .dir
            .parent()
            .unwrap_or(&self.dir)
            .join("archived")
            .join(self.dir.file_name().unwrap_or_default());
        std::fs::create_dir_all(archived.parent().unwrap())?;
        std::fs::rename(&self.dir, &archived)?;
        Ok(())
    }

    // ── 内部辅助 ─────────────────────────────────────────────────────────────

    fn write_file(&self, name: &str, content: &str) -> Result<()> {
        let path = self.dir.join(name);
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))
    }

    fn ensure_task_dir(&self, task_id: &str) -> Result<()> {
        let dir = self.task_dir(task_id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create task artifact dir: {}", dir.display()))
    }

    fn write_task_file(&self, task_id: &str, name: &str, content: &str) -> Result<()> {
        let path = self.task_dir(task_id).join(name);
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))
    }

    fn append_task_file(&self, task_id: &str, name: &str, content: &str) -> Result<()> {
        use std::io::Write;
        let path = self.task_dir(task_id).join(name);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        writeln!(file, "{content}")?;
        Ok(())
    }

    fn read_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::registry::{CreateTask, TaskRegistry};
    use tempfile::tempdir;

    fn make_session() -> (TeamSession, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let session = TeamSession::from_dir("team-001", tmp.path().to_path_buf());
        (session, tmp)
    }

    #[test]
    fn test_specialist_session_key_format() {
        let (session, _tmp) = make_session();
        let key = session.specialist_session_key("codex");
        assert_eq!(key.channel, "specialist");
        assert_eq!(key.scope, "team-001:codex");
    }

    #[test]
    fn test_write_and_read_files() {
        let (session, _tmp) = make_session();
        session
            .write_team_md("Claude: Lead\nCodex: Backend")
            .unwrap();
        session.write_context_md("Task context here").unwrap();
        session.write_heartbeat_md("Check stale tasks").unwrap();

        assert_eq!(session.read_team_md(), "Claude: Lead\nCodex: Backend");
        assert_eq!(session.read_context_md(), "Task context here");
        assert_eq!(session.read_heartbeat_md(), "Check stale tasks");
    }

    #[test]
    fn test_sync_tasks_md() {
        let (session, _tmp) = make_session();
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "Setup project".into(),
                ..Default::default()
            })
            .unwrap();
        session.sync_tasks_md(&registry).unwrap();
        let md = session.read_tasks_md();
        assert!(md.contains("T001"));
        assert!(md.contains("Setup project"));
    }

    #[test]
    fn test_build_task_reminder_injects_upstream_notes() {
        let (session, _tmp) = make_session();
        let registry = TaskRegistry::new_in_memory().unwrap();

        // T001 is a dependency with a completion note
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "Design schema".into(),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T001", "codex").unwrap();
        registry
            .mark_done("T001", "codex", "Created users table with uuid pk")
            .unwrap();

        // T002 depends on T001
        registry
            .create_task(CreateTask {
                id: "T002".into(),
                title: "Implement model".into(),
                deps: vec!["T001".into()],
                ..Default::default()
            })
            .unwrap();

        let task = registry.get_task("T002").unwrap().unwrap();
        let reminder = session.build_task_reminder(&task, &registry);

        assert!(
            reminder.contains("上游任务结果"),
            "must have upstream section header"
        );
        assert!(reminder.contains("T001"), "must mention T001");
        assert!(
            reminder.contains("Created users table"),
            "must include T001 completion note"
        );
    }

    #[test]
    fn test_build_task_reminder_contains_done_marker() {
        let (session, _tmp) = make_session();
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T003".into(),
                title: "Implement JWT".into(),
                success_criteria: Some("JWT token is generated and verified".into()),
                ..Default::default()
            })
            .unwrap();
        let task = registry.get_task("T003").unwrap().unwrap();
        let reminder = session.build_task_reminder(&task, &registry);

        assert!(
            !reminder.contains("[DONE:"),
            "must NOT contain legacy [DONE:] text marker"
        );
        assert!(
            !reminder.contains("[BLOCKED:"),
            "must NOT contain legacy [BLOCKED:] text marker"
        );
        assert!(reminder.contains("Implement JWT"));
        assert!(reminder.contains("JWT token is generated"));
        assert!(
            reminder.contains("complete_task"),
            "must mention complete_task MCP tool"
        );
        assert!(
            reminder.contains("block_task"),
            "must mention block_task MCP tool"
        );
    }

    #[test]
    fn test_append_specialist_reply_creates_jsonl_entry() {
        let (session, _tmp) = make_session();
        session
            .append_specialist_reply("codex", "T001", "Created users table with UUID PK.")
            .unwrap();

        let path = _tmp.path().join("events.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("SPECIALIST_REPLY"));
        assert!(contents.contains("codex"));
        assert!(contents.contains("T001"));
        assert!(contents.contains("Created users table"));
        // Each entry must be on a single line (no literal newlines in content)
        assert_eq!(contents.lines().count(), 1);
    }

    #[test]
    fn test_append_specialist_reply_newlines_escaped() {
        let (session, _tmp) = make_session();
        session
            .append_specialist_reply("codex", "T002", "Line 1\nLine 2\nLine 3")
            .unwrap();
        let path = _tmp.path().join("events.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap();
        // Text content must not introduce extra lines
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.contains("\\n"));
    }

    #[test]
    fn test_task_artifact_helpers_write_expected_layout() {
        let (session, _tmp) = make_session();
        let meta = TaskArtifactMeta {
            id: "T010".into(),
            title: "Ship auth".into(),
            assignee_hint: Some("codex".into()),
            status: "pending".into(),
            deps: vec!["T001".into()],
            success_criteria: Some("Auth flow passes".into()),
            created_at: "2026-03-09T00:00:00Z".into(),
            updated_at: "2026-03-09T00:00:00Z".into(),
            done_at: None,
            claimed_by: None,
            submitted_by: None,
            accepted_by: None,
        };

        session.write_task_meta("T010", &meta).unwrap();
        session.write_task_spec("T010", "# Spec\nImplement auth").unwrap();
        session
            .append_task_progress("T010", "[checkpoint] schema drafted")
            .unwrap();
        session
            .write_task_result("T010", "# Result\nReady for review")
            .unwrap();

        let task_dir = _tmp.path().join("tasks").join("T010");
        assert!(task_dir.is_dir());
        assert!(task_dir.join("meta.json").is_file());
        assert!(task_dir.join("spec.md").is_file());
        assert!(task_dir.join("progress.md").is_file());
        assert!(task_dir.join("result.md").is_file());

        let meta_text = std::fs::read_to_string(task_dir.join("meta.json")).unwrap();
        assert!(meta_text.contains("\"id\": \"T010\""));
        assert!(meta_text.contains("\"status\": \"pending\""));
        let spec_text = std::fs::read_to_string(task_dir.join("spec.md")).unwrap();
        assert!(spec_text.contains("Implement auth"));
        let progress_text = std::fs::read_to_string(task_dir.join("progress.md")).unwrap();
        assert!(progress_text.contains("schema drafted"));
        let result_text = std::fs::read_to_string(task_dir.join("result.md")).unwrap();
        assert!(result_text.contains("Ready for review"));
    }

    #[test]
    fn test_append_task_progress_appends_instead_of_overwriting() {
        let (session, _tmp) = make_session();
        session.append_task_progress("T011", "checkpoint one").unwrap();
        session.append_task_progress("T011", "checkpoint two").unwrap();

        let progress = std::fs::read_to_string(_tmp.path().join("tasks").join("T011").join("progress.md"))
            .unwrap();
        assert!(progress.contains("checkpoint one"));
        assert!(progress.contains("checkpoint two"));
        assert_eq!(progress.lines().count(), 2);
    }

    #[test]
    fn test_task_artifact_meta_projects_registry_task_fields() {
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T012".into(),
                title: "Implement billing".into(),
                assignee_hint: Some("worker".into()),
                deps: vec!["T001".into(), "T002".into()],
                success_criteria: Some("Billing succeeds".into()),
                spec: Some("Implement billing workflow".into()),
                ..Default::default()
            })
            .unwrap();
        registry.try_claim("T012", "worker").unwrap();
        let task = registry.get_task("T012").unwrap().unwrap();

        let meta = TaskArtifactMeta::from_task(&task);
        assert_eq!(meta.id, "T012");
        assert_eq!(meta.title, "Implement billing");
        assert_eq!(meta.assignee_hint.as_deref(), Some("worker"));
        assert_eq!(meta.status, task.status_raw);
        assert_eq!(meta.deps, vec!["T001".to_string(), "T002".to_string()]);
        assert_eq!(meta.success_criteria.as_deref(), Some("Billing succeeds"));
        assert_eq!(meta.claimed_by.as_deref(), Some("worker"));
        assert!(meta.submitted_by.is_none());
        assert!(meta.accepted_by.is_none());
    }
}
