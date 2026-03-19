//! TeamSession — Team Mode 的目录结构与工具函数
//!
//! 目录布局：
//!   ~/.clawbro/team-sessions/{group_hash}-{team_id}/
//!     TEAM.md        — 团队职责宣言（Lead 在 /team start 时写入）
//!     CONTEXT.md     — 任务背景（Lead 维护，注入 Specialist 的 shared_memory）
//!     TASKS.md       — 任务快照（由 TaskRegistry::export_tasks_md 导出，只读）
//!     HEARTBEAT.md   — 可选的团队心跳检查清单（Team context，不是通用 scheduler 定义）
//!     events.jsonl   — 事件日志（调试用）

use anyhow::{Context, Result};
use chrono::Utc;
use clawbro_protocol::{normalize_conversation_identity, SessionKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

use super::completion_routing::TeamRoutingEnvelope;
use super::registry::{Task, TaskRegistry, TaskStatus};

const DELIVERY_DEDUPE_LEDGER_MAX_LINES: usize = 2_000;

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
    pub spec_path: String,
    pub plan_path: String,
    pub progress_path: String,
    pub result_path: String,
}

impl TaskArtifactMeta {
    pub fn from_task(task: &Task) -> Self {
        let (claimed_by, submitted_by, accepted_by, updated_at) = match task.status_parsed() {
            TaskStatus::Claimed { agent, at } => (Some(agent), None, None, at.to_rfc3339()),
            TaskStatus::Held { at, .. } => (None, None, None, at.to_rfc3339()),
            TaskStatus::Submitted { agent, at } => (None, Some(agent), None, at.to_rfc3339()),
            TaskStatus::Accepted { by, at } => (None, None, Some(by), at.to_rfc3339()),
            TaskStatus::Done => (
                None,
                None,
                None,
                task.done_at
                    .as_ref()
                    .map(chrono::DateTime::to_rfc3339)
                    .unwrap_or_else(|| task.created_at.to_rfc3339()),
            ),
            _ => (None, None, None, task.created_at.to_rfc3339()),
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
            spec_path: format!("tasks/{}/spec.md", task.id),
            plan_path: format!("tasks/{}/plan.md", task.id),
            progress_path: format!("tasks/{}/progress.md", task.id),
            result_path: format!("tasks/{}/result.md", task.id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderUpdateKind {
    PostUpdate,
    FinalAnswerFragment,
    SystemForward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderUpdateRecord {
    pub event_id: String,
    pub ts: String,
    pub team_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_channel_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_thread_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_turn_id: Option<String>,
    pub source_agent: String,
    pub kind: LeaderUpdateKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_send_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_message_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSendSourceKind {
    LeadText,
    Milestone,
    Progress,
    ToolPlaceholder,
    GatewayError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSendStatus {
    Sent,
    SendFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSendRecord {
    pub event_id: String,
    pub ts: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_channel_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_channel_instance: Option<String>,
    pub target_scope: String,
    pub team_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_channel_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_thread_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub source_kind: ChannelSendSourceKind,
    pub source_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    pub text: String,
    pub status: ChannelSendStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct TeamSession {
    pub team_id: String,
    pub dir: PathBuf,
    delivery_dedupe_lock: Mutex<()>,
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
            .join(".clawbro")
            .join("team-sessions")
            .join(format!("{}-{}", safe_scope, team_id));

        std::fs::create_dir_all(&base)
            .with_context(|| format!("Failed to create team session dir: {}", base.display()))?;

        Ok(Self {
            team_id: team_id.to_string(),
            dir: base,
            delivery_dedupe_lock: Mutex::new(()),
        })
    }

    /// 从已有路径恢复 TeamSession（不创建目录）
    pub fn from_dir(team_id: &str, dir: PathBuf) -> Self {
        Self {
            team_id: team_id.to_string(),
            dir,
            delivery_dedupe_lock: Mutex::new(()),
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

    pub fn read_agents_md(&self) -> String {
        self.read_file("AGENTS.md")
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

    pub fn archive_completed_cycle(&self, tasks: &[Task]) -> Result<Option<String>> {
        if tasks.is_empty() {
            return Ok(None);
        }

        let archive_rel = format!("cycles/cycle-{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
        let archive_dir = self.dir.join(&archive_rel);
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create {}", archive_dir.display()))?;

        let tasks_md = self.dir.join("TASKS.md");
        if tasks_md.is_file() {
            std::fs::rename(&tasks_md, archive_dir.join("TASKS.md"))
                .with_context(|| format!("Failed to move {} into archive", tasks_md.display()))?;
        }

        let tasks_root = self.tasks_dir();
        if tasks_root.is_dir() {
            std::fs::rename(&tasks_root, archive_dir.join("tasks"))
                .with_context(|| format!("Failed to move {} into archive", tasks_root.display()))?;
        }

        let manifest = serde_json::json!({
            "archived_at": Utc::now().to_rfc3339(),
            "team_id": self.team_id,
            "task_count": tasks.len(),
            "tasks": tasks.iter().map(|task| serde_json::json!({
                "id": task.id,
                "title": task.title,
                "status": task.status_raw,
                "created_at": task.created_at.to_rfc3339(),
                "done_at": task.done_at.as_ref().map(chrono::DateTime::to_rfc3339),
            })).collect::<Vec<_>>(),
        });
        std::fs::write(
            archive_dir.join("cycle.json"),
            serde_json::to_string_pretty(&manifest)?,
        )
        .with_context(|| {
            format!(
                "Failed to write {}",
                archive_dir.join("cycle.json").display()
            )
        })?;

        Ok(Some(archive_rel))
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

    pub fn write_task_plan(&self, task_id: &str, content: &str) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        self.write_task_file(task_id, "plan.md", content)
    }

    pub fn ensure_task_plan(&self, task_id: &str, content: &str) -> Result<()> {
        self.ensure_task_dir(task_id)?;
        let path = self.task_dir(task_id).join("plan.md");
        if path.is_file() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.trim().is_empty() {
                return Ok(());
            }
        }
        self.write_task_file(task_id, "plan.md", content)
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

    pub fn append_pending_completion(&self, envelope: &TeamRoutingEnvelope) -> Result<()> {
        use std::io::Write;
        let path = self.dir.join("pending-completions.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", serde_json::to_string(envelope)?)?;
        Ok(())
    }

    pub fn load_pending_completions(&self) -> Result<Vec<TeamRoutingEnvelope>> {
        let path = self.dir.join("pending-completions.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&path)?;
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn clear_pending_completions(&self) -> Result<()> {
        let path = self.dir.join("pending-completions.jsonl");
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn replace_pending_completions(&self, envelopes: &[TeamRoutingEnvelope]) -> Result<()> {
        let path = self.dir.join("pending-completions.jsonl");
        if envelopes.is_empty() {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }

        let tmp_path = self.dir.join("pending-completions.jsonl.tmp");
        let body = envelopes
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n");
        std::fs::write(&tmp_path, format!("{body}\n"))?;
        std::fs::rename(tmp_path, path)?;
        Ok(())
    }

    pub fn remove_pending_completion_by_run_id(&self, run_id: &str) -> Result<bool> {
        let pending = self.load_pending_completions()?;
        let original_len = pending.len();
        let remaining = pending
            .into_iter()
            .filter(|envelope| envelope.run_id != run_id)
            .collect::<Vec<_>>();
        let removed = remaining.len() != original_len;
        self.replace_pending_completions(&remaining)?;
        Ok(removed)
    }

    pub fn append_routing_outcome(&self, envelope: &TeamRoutingEnvelope) -> Result<()> {
        self.append_jsonl("routing-events.jsonl", envelope)
    }

    pub fn load_routing_outcomes(&self) -> Result<Vec<TeamRoutingEnvelope>> {
        let path = self.dir.join("routing-events.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&path)?;
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn mark_delivery_dedupe(&self, target_scope: &str, dedupe_key: &str) -> Result<bool> {
        let _guard = self.delivery_dedupe_lock.lock().unwrap();
        let path = self.dir.join("delivered-milestones.jsonl");
        let scoped_key = format!("{target_scope}:{dedupe_key}");

        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            if content.lines().any(|line| line.trim() == scoped_key) {
                return Ok(false);
            }
        }

        append_bounded_line(&path, &scoped_key, DELIVERY_DEDUPE_LEDGER_MAX_LINES)?;
        Ok(true)
    }

    pub fn record_delivery_dedupe_hit(&self, target_scope: &str, dedupe_key: &str) -> Result<()> {
        let _guard = self.delivery_dedupe_lock.lock().unwrap();
        let path = self.dir.join("delivery-dedupe-hits.jsonl");
        let scoped_key = format!("{target_scope}:{dedupe_key}");
        append_bounded_line(&path, &scoped_key, DELIVERY_DEDUPE_LEDGER_MAX_LINES)?;
        Ok(())
    }

    pub fn delivery_dedupe_ledger_size(&self) -> Result<usize> {
        let path = self.dir.join("delivered-milestones.jsonl");
        if !path.exists() {
            return Ok(0);
        }
        Ok(std::fs::read_to_string(&path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    pub fn delivery_dedupe_hit_count(&self) -> Result<usize> {
        let path = self.dir.join("delivery-dedupe-hits.jsonl");
        if !path.exists() {
            return Ok(0);
        }
        Ok(std::fs::read_to_string(&path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    pub fn clear_delivery_dedupe_ledgers(&self) -> Result<()> {
        for file in ["delivered-milestones.jsonl", "delivery-dedupe-hits.jsonl"] {
            let path = self.dir.join(file);
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
        Ok(())
    }

    pub fn record_leader_update(
        &self,
        lead_session_key: Option<&SessionKey>,
        lead_delivery_source: Option<&crate::turn_context::TurnDeliverySource>,
        source_agent: &str,
        kind: LeaderUpdateKind,
        text: &str,
        task_id: Option<&str>,
    ) -> Result<String> {
        let event_id = uuid::Uuid::new_v4().to_string();
        let record = LeaderUpdateRecord {
            event_id: event_id.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
            team_id: self.team_id.clone(),
            lead_session_channel: lead_session_key.map(|key| key.channel.clone()),
            lead_session_channel_instance: lead_session_key
                .and_then(|key| key.channel_instance.clone()),
            lead_session_scope: lead_session_key.map(|key| key.scope.clone()),
            lead_reply_to: lead_delivery_source.and_then(|source| source.reply_to.clone()),
            lead_thread_ts: lead_delivery_source.and_then(|source| source.thread_ts.clone()),
            lead_turn_id: None,
            source_agent: source_agent.to_string(),
            kind,
            text: text.to_string(),
            task_id: task_id.map(ToOwned::to_owned),
            channel_send_event_id: None,
            session_message_id: None,
        };
        self.append_jsonl("leader-updates.jsonl", &record)?;
        Ok(event_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_channel_send(
        &self,
        channel: &str,
        sender_channel_instance: Option<&str>,
        target_channel_instance: Option<&str>,
        target_scope: &str,
        lead_session_key: Option<&SessionKey>,
        lead_delivery_source: Option<&crate::turn_context::TurnDeliverySource>,
        reply_to: Option<&str>,
        thread_ts: Option<&str>,
        source_kind: ChannelSendSourceKind,
        source_agent: &str,
        task_id: Option<&str>,
        dedupe_key: Option<&str>,
        text: &str,
        status: ChannelSendStatus,
        error: Option<&str>,
    ) -> Result<String> {
        let event_id = uuid::Uuid::new_v4().to_string();
        let record = ChannelSendRecord {
            event_id: event_id.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
            channel: channel.to_string(),
            sender_channel_instance: sender_channel_instance.map(ToOwned::to_owned),
            target_channel_instance: target_channel_instance.map(ToOwned::to_owned),
            target_scope: target_scope.to_string(),
            team_id: self.team_id.clone(),
            lead_session_channel: lead_session_key.map(|key| key.channel.clone()),
            lead_session_channel_instance: lead_session_key
                .and_then(|key| key.channel_instance.clone()),
            lead_session_scope: lead_session_key.map(|key| key.scope.clone()),
            lead_reply_to: lead_delivery_source.and_then(|source| source.reply_to.clone()),
            lead_thread_ts: lead_delivery_source.and_then(|source| source.thread_ts.clone()),
            reply_to: reply_to.map(ToOwned::to_owned),
            thread_ts: thread_ts.map(ToOwned::to_owned),
            source_kind,
            source_agent: source_agent.to_string(),
            task_id: task_id.map(ToOwned::to_owned),
            dedupe_key: dedupe_key.map(ToOwned::to_owned),
            text: text.to_string(),
            status,
            provider_message_id: None,
            error: error.map(ToOwned::to_owned),
        };
        self.append_jsonl("channel-sends.jsonl", &record)?;
        Ok(event_id)
    }

    pub fn load_latest_leader_update(&self) -> Result<Option<LeaderUpdateRecord>> {
        self.load_latest_jsonl("leader-updates.jsonl")
    }

    pub fn load_latest_channel_send(&self) -> Result<Option<ChannelSendRecord>> {
        self.load_latest_jsonl("channel-sends.jsonl")
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

        // If the task is already Claimed, it means this is a resume after an interruption.
        let resume_note = match task.status_parsed() {
            crate::team::registry::TaskStatus::Claimed { agent, at } => {
                let elapsed = chrono::Utc::now().signed_duration_since(at);
                let mins = elapsed.num_minutes();
                if mins >= 2 {
                    format!(
                        "\n[⚠️ 任务恢复] 此任务之前已由 {} 领取（{}分钟前）并可能因 Gateway 重启而中断。\
                         请检查已完成的工作并从中断处继续。",
                        agent, mins
                    )
                } else {
                    format!("\n[任务进行中] 此任务已由 {} 领取，请继续执行。", agent)
                }
            }
            _ => String::new(),
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
             6. 在结束本轮前，必须至少调用一个 canonical team tool：`submit_task_result`、`complete_task`、`checkpoint_task`、`request_help` 或 `block_task`\n\
             ══════════════════════════════════════════{resume_note}{upstream_section}",
            id = task.id,
            title = task.title,
            spec = task.spec.as_deref().unwrap_or("（无详细说明）"),
            deps = deps_str,
            blocking = blocking_str,
            criteria = task
                .success_criteria
                .as_deref()
                .unwrap_or("完成任务说明中描述的工作"),
            resume_note = resume_note,
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

    fn append_jsonl<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        use std::io::Write;
        let path = self.dir.join(name);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        writeln!(file, "{}", serde_json::to_string(value)?)?;
        Ok(())
    }

    fn load_latest_jsonl<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.dir.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let Some(line) = content.lines().rev().find(|line| !line.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(line)?))
    }
}

fn append_bounded_line(path: &std::path::Path, line: &str, max_lines: usize) -> Result<()> {
    use std::io::Write;

    let mut lines = if path.exists() {
        std::fs::read_to_string(path)?
            .lines()
            .filter(|entry| !entry.trim().is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    lines.push(line.to_string());
    if lines.len() > max_lines {
        let drain_count = lines.len() - max_lines;
        lines.drain(0..drain_count);
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    for entry in lines {
        writeln!(file, "{entry}")?;
    }
    Ok(())
}

pub fn stable_team_id(channel: &str, scope: &str) -> String {
    let seed = format!("{channel}:{scope}");
    format!(
        "team-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, seed.as_bytes()).simple()
    )
}

pub fn stable_team_id_for_session_key(session_key: &SessionKey) -> String {
    let normalized = normalize_conversation_identity(session_key);
    let seed = match normalized.channel_instance.as_deref() {
        Some(instance) => format!("{}@{}:{}", normalized.channel, instance, normalized.scope),
        None => format!("{}:{}", normalized.channel, normalized.scope),
    };
    format!(
        "team-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, seed.as_bytes()).simple()
    )
}

pub fn parse_specialist_session_scope(scope: &str) -> Option<(&str, &str)> {
    scope.rsplit_once(':')
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::completion_routing::{
        RoutingDeliveryStatus, TeamRoutingEnvelope, TeamRoutingEvent,
    };
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
    fn test_stable_team_id_is_channel_aware_and_stable() {
        let lark_dm = stable_team_id("lark", "user:ou_same");
        let dingtalk_dm = stable_team_id("dingtalk", "user:ou_same");
        let lark_variant = stable_team_id("lark", "user/ou_same");

        assert_eq!(lark_dm, stable_team_id("lark", "user:ou_same"));
        assert_ne!(lark_dm, dingtalk_dm);
        assert_ne!(lark_dm, lark_variant);
    }

    #[test]
    fn test_stable_team_id_for_session_key_shares_group_across_instances() {
        let alpha = SessionKey::with_instance("lark", "alpha", "group:oc_1");
        let beta = SessionKey::with_instance("lark", "beta", "group:oc_1");
        assert_eq!(
            stable_team_id_for_session_key(&alpha),
            stable_team_id_for_session_key(&beta)
        );
    }

    #[test]
    fn test_stable_team_id_for_session_key_isolates_dm_by_instance() {
        let alpha = SessionKey::with_instance("lark", "alpha", "user:ou_1");
        let beta = SessionKey::with_instance("lark", "beta", "user:ou_1");
        assert_ne!(
            stable_team_id_for_session_key(&alpha),
            stable_team_id_for_session_key(&beta)
        );
    }

    #[test]
    fn test_parse_specialist_session_scope_supports_colons_inside_team_id() {
        let (team_id, agent) =
            parse_specialist_session_scope("team-abc:with:colons:codex").expect("valid scope");
        assert_eq!(team_id, "team-abc:with:colons");
        assert_eq!(agent, "codex");
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
    fn test_record_leader_update_writes_jsonl() {
        let (session, _tmp) = make_session();
        let lead_key = SessionKey::new("lark", "user:test");
        let event_id = session
            .record_leader_update(
                Some(&lead_key),
                None,
                "codex-alpha",
                LeaderUpdateKind::PostUpdate,
                "正在处理",
                Some("T001"),
            )
            .unwrap();

        let content = std::fs::read_to_string(session.dir.join("leader-updates.jsonl")).unwrap();
        let row: LeaderUpdateRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(row.event_id, event_id);
        assert_eq!(row.team_id, "team-001");
        assert_eq!(row.lead_session_channel.as_deref(), Some("lark"));
        assert_eq!(row.lead_session_scope.as_deref(), Some("user:test"));
        assert_eq!(row.source_agent, "codex-alpha");
        assert_eq!(row.text, "正在处理");
        assert_eq!(row.task_id.as_deref(), Some("T001"));
    }

    #[test]
    fn test_record_channel_send_writes_jsonl() {
        let (session, _tmp) = make_session();
        let lead_key = SessionKey::new("lark", "user:test");
        let event_id = session
            .record_channel_send(
                "lark",
                None,
                None,
                "user:test",
                Some(&lead_key),
                None,
                Some("msg-1"),
                None,
                ChannelSendSourceKind::LeadText,
                "codex-alpha",
                Some("T001"),
                Some("dedupe-1"),
                "任务已认领",
                ChannelSendStatus::Sent,
                None,
            )
            .unwrap();

        let content = std::fs::read_to_string(session.dir.join("channel-sends.jsonl")).unwrap();
        let row: ChannelSendRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(row.event_id, event_id);
        assert_eq!(row.channel, "lark");
        assert_eq!(row.target_scope, "user:test");
        assert_eq!(row.source_agent, "codex-alpha");
        assert_eq!(row.task_id.as_deref(), Some("T001"));
        assert_eq!(row.dedupe_key.as_deref(), Some("dedupe-1"));
        assert_eq!(row.text, "任务已认领");
        assert_eq!(row.status, ChannelSendStatus::Sent);
    }

    #[test]
    fn test_latest_delivery_ledgers_return_last_record() {
        let (session, _tmp) = make_session();
        let lead_key = SessionKey::with_instance("lark", "alpha", "group:test");
        let lead_source = crate::turn_context::TurnDeliverySource::from_session_key(&lead_key)
            .with_reply_context(Some("msg-2".into()), Some("thread-9".into()));

        session
            .record_leader_update(
                Some(&lead_key),
                Some(&lead_source),
                "codex-alpha",
                LeaderUpdateKind::PostUpdate,
                "first",
                None,
            )
            .unwrap();
        session
            .record_leader_update(
                Some(&lead_key),
                Some(&lead_source),
                "codex-beta",
                LeaderUpdateKind::SystemForward,
                "second",
                Some("T002"),
            )
            .unwrap();

        session
            .record_channel_send(
                "lark",
                Some("beta"),
                Some("gamma"),
                "group:test",
                Some(&lead_key),
                Some(&lead_source),
                Some("msg-2"),
                Some("thread-9"),
                ChannelSendSourceKind::Milestone,
                "codex-beta",
                Some("T002"),
                None,
                "done",
                ChannelSendStatus::Sent,
                None,
            )
            .unwrap();

        let leader = session.load_latest_leader_update().unwrap().unwrap();
        assert_eq!(leader.source_agent, "codex-beta");
        assert_eq!(
            leader.lead_session_channel_instance.as_deref(),
            Some("alpha")
        );
        assert_eq!(leader.lead_reply_to.as_deref(), Some("msg-2"));

        let send = session.load_latest_channel_send().unwrap().unwrap();
        assert_eq!(send.sender_channel_instance.as_deref(), Some("beta"));
        assert_eq!(send.target_channel_instance.as_deref(), Some("gamma"));
        assert_eq!(send.reply_to.as_deref(), Some("msg-2"));
        assert_eq!(send.thread_ts.as_deref(), Some("thread-9"));
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
    fn test_pending_completion_round_trip() {
        let (session, _tmp) = make_session();
        let envelope = TeamRoutingEnvelope {
            run_id: "run-123".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("ws", "group:team")),
            fallback_session_keys: vec![],
            team_id: "team-001".into(),
            delivery_status: RoutingDeliveryStatus::PersistedPending,
            event: TeamRoutingEvent::failed("T999", "boom"),
            delivery_source: None,
        };

        session.append_pending_completion(&envelope).unwrap();
        let loaded = session.load_pending_completions().unwrap();
        assert_eq!(loaded, vec![envelope]);

        session.clear_pending_completions().unwrap();
        assert!(session.load_pending_completions().unwrap().is_empty());
    }

    #[test]
    fn test_remove_pending_completion_by_run_id() {
        let (session, _tmp) = make_session();
        let first = TeamRoutingEnvelope {
            run_id: "run-1".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("ws", "group:team")),
            fallback_session_keys: vec![],
            team_id: "team-001".into(),
            delivery_status: RoutingDeliveryStatus::PersistedPending,
            event: TeamRoutingEvent::failed("T001", "boom"),
            delivery_source: None,
        };
        let second = TeamRoutingEnvelope {
            run_id: "run-2".into(),
            parent_run_id: None,
            requester_session_key: Some(SessionKey::new("ws", "group:team")),
            fallback_session_keys: vec![],
            team_id: "team-001".into(),
            delivery_status: RoutingDeliveryStatus::PersistedPending,
            event: TeamRoutingEvent::failed("T002", "boom"),
            delivery_source: None,
        };

        session.append_pending_completion(&first).unwrap();
        session.append_pending_completion(&second).unwrap();

        assert!(session
            .remove_pending_completion_by_run_id("run-1")
            .unwrap());
        let loaded = session.load_pending_completions().unwrap();
        assert_eq!(loaded, vec![second]);
        assert!(!session
            .remove_pending_completion_by_run_id("missing-run")
            .unwrap());
    }

    #[test]
    fn test_archive_completed_cycle_moves_active_artifacts() {
        let (session, _tmp) = make_session();
        let registry = TaskRegistry::new_in_memory().unwrap();
        registry
            .create_task(CreateTask {
                id: "T001".into(),
                title: "Task".into(),
                assignee_hint: Some("codex".into()),
                deps: vec![],
                timeout_secs: 60,
                spec: None,
                success_criteria: None,
            })
            .unwrap();
        registry.try_claim("T001", "codex").unwrap();
        registry
            .submit_task_result("T001", "codex", "done")
            .unwrap();
        registry.accept_task("T001", "leader").unwrap();
        session.sync_tasks_md(&registry).unwrap();
        session.write_task_result("T001", "result").unwrap();

        let tasks = registry.all_tasks().unwrap();
        let archive_rel = session.archive_completed_cycle(&tasks).unwrap().unwrap();
        let archive_dir = _tmp.path().join(archive_rel);

        assert!(archive_dir.join("TASKS.md").is_file());
        assert!(archive_dir
            .join("tasks")
            .join("T001")
            .join("result.md")
            .is_file());
        assert!(!_tmp.path().join("TASKS.md").exists());
        assert!(!_tmp.path().join("tasks").exists());
    }

    #[test]
    fn test_clear_delivery_dedupe_ledgers_removes_files() {
        let (session, _tmp) = make_session();
        session
            .mark_delivery_dedupe("group:team", "all_tasks_done")
            .unwrap();
        session
            .record_delivery_dedupe_hit("group:team", "all_tasks_done")
            .unwrap();

        session.clear_delivery_dedupe_ledgers().unwrap();

        assert!(!_tmp.path().join("delivered-milestones.jsonl").exists());
        assert!(!_tmp.path().join("delivery-dedupe-hits.jsonl").exists());
    }

    #[test]
    fn test_delivery_dedupe_persists_across_reloads() {
        let (session, tmp) = make_session();
        assert!(session
            .mark_delivery_dedupe("group:team", "all_tasks_done")
            .unwrap());
        assert!(!session
            .mark_delivery_dedupe("group:team", "all_tasks_done")
            .unwrap());
        assert!(session
            .mark_delivery_dedupe("group:other", "all_tasks_done")
            .unwrap());

        let reloaded = TeamSession::from_dir("team-001", tmp.path().to_path_buf());
        assert!(!reloaded
            .mark_delivery_dedupe("group:team", "all_tasks_done")
            .unwrap());
        assert!(reloaded
            .mark_delivery_dedupe("group:team", "task_done:T001")
            .unwrap());
    }

    #[test]
    fn test_delivery_dedupe_hit_metrics_round_trip() {
        let (session, _tmp) = make_session();
        session
            .record_delivery_dedupe_hit("group:team", "all_tasks_done")
            .unwrap();
        session
            .record_delivery_dedupe_hit("group:team", "all_tasks_done")
            .unwrap();
        session
            .mark_delivery_dedupe("group:team", "all_tasks_done")
            .unwrap();

        assert_eq!(session.delivery_dedupe_hit_count().unwrap(), 2);
        assert_eq!(session.delivery_dedupe_ledger_size().unwrap(), 1);
    }

    #[test]
    fn test_delivery_dedupe_ledgers_retain_recent_window_only() {
        let (session, tmp) = make_session();

        for idx in 0..(DELIVERY_DEDUPE_LEDGER_MAX_LINES + 5) {
            session
                .record_delivery_dedupe_hit("group:team", &format!("hit-{idx}"))
                .unwrap();
            let _ = session
                .mark_delivery_dedupe("group:team", &format!("ledger-{idx}"))
                .unwrap();
        }

        assert_eq!(
            session.delivery_dedupe_hit_count().unwrap(),
            DELIVERY_DEDUPE_LEDGER_MAX_LINES
        );
        assert_eq!(
            session.delivery_dedupe_ledger_size().unwrap(),
            DELIVERY_DEDUPE_LEDGER_MAX_LINES
        );

        let hit_lines =
            std::fs::read_to_string(tmp.path().join("delivery-dedupe-hits.jsonl")).unwrap();
        assert!(!hit_lines.contains("hit-0"));
        assert!(hit_lines.contains(&format!("hit-{}", DELIVERY_DEDUPE_LEDGER_MAX_LINES + 4)));

        let ledger_lines =
            std::fs::read_to_string(tmp.path().join("delivered-milestones.jsonl")).unwrap();
        assert!(!ledger_lines.contains("ledger-0"));
        assert!(ledger_lines.contains(&format!("ledger-{}", DELIVERY_DEDUPE_LEDGER_MAX_LINES + 4)));
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
            spec_path: "tasks/T010/spec.md".into(),
            plan_path: "tasks/T010/plan.md".into(),
            progress_path: "tasks/T010/progress.md".into(),
            result_path: "tasks/T010/result.md".into(),
        };

        session.write_task_meta("T010", &meta).unwrap();
        session
            .write_task_spec("T010", "# Spec\nImplement auth")
            .unwrap();
        session
            .write_task_plan("T010", "# Task Plan\n- [ ] Draft auth flow")
            .unwrap();
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
        assert!(task_dir.join("plan.md").is_file());
        assert!(task_dir.join("progress.md").is_file());
        assert!(task_dir.join("result.md").is_file());

        let meta_text = std::fs::read_to_string(task_dir.join("meta.json")).unwrap();
        assert!(meta_text.contains("\"id\": \"T010\""));
        assert!(meta_text.contains("\"status\": \"pending\""));
        let spec_text = std::fs::read_to_string(task_dir.join("spec.md")).unwrap();
        assert!(spec_text.contains("Implement auth"));
        let plan_text = std::fs::read_to_string(task_dir.join("plan.md")).unwrap();
        assert!(plan_text.contains("Draft auth flow"));
        let progress_text = std::fs::read_to_string(task_dir.join("progress.md")).unwrap();
        assert!(progress_text.contains("schema drafted"));
        let result_text = std::fs::read_to_string(task_dir.join("result.md")).unwrap();
        assert!(result_text.contains("Ready for review"));
    }

    #[test]
    fn test_ensure_task_plan_preserves_existing_edits() {
        let (session, tmp) = make_session();
        session
            .write_task_plan("T012", "# Task Plan\n- [x] Existing step")
            .unwrap();
        session
            .ensure_task_plan("T012", "# Task Plan\n- [ ] Replacement step")
            .unwrap();

        let plan =
            std::fs::read_to_string(tmp.path().join("tasks").join("T012").join("plan.md")).unwrap();
        assert!(plan.contains("Existing step"));
        assert!(!plan.contains("Replacement step"));
    }

    #[test]
    fn test_append_task_progress_appends_instead_of_overwriting() {
        let (session, _tmp) = make_session();
        session
            .append_task_progress("T011", "checkpoint one")
            .unwrap();
        session
            .append_task_progress("T011", "checkpoint two")
            .unwrap();

        let progress =
            std::fs::read_to_string(_tmp.path().join("tasks").join("T011").join("progress.md"))
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
