use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    Cron,
    At,
    Every,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Cron { expr: String },
    At { run_at: DateTime<Utc> },
    Every { interval_ms: i64 },
}

impl ScheduleSpec {
    pub fn kind(&self) -> ScheduleKind {
        match self {
            Self::Cron { .. } => ScheduleKind::Cron,
            Self::At { .. } => ScheduleKind::At,
            Self::Every { .. } => ScheduleKind::Every,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleInput {
    Cron { expr: String },
    At { run_at: DateTime<Utc> },
    Every { interval_ms: i64 },
    Delay { delay_ms: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionPrecondition {
    IdleGtSeconds { threshold_seconds: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurnTarget {
    pub session_key: String,
    pub prompt: String,
    pub agent: Option<String>,
    #[serde(default)]
    pub preconditions: Vec<ExecutionPrecondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryMessageTarget {
    pub session_key: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedTargetKind {
    Auto,
    AgentTurn,
    DeliveryMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTargetRequest {
    pub requested_kind: RequestedTargetKind,
    pub session_key: String,
    pub prompt: String,
    pub agent: Option<String>,
    #[serde(default)]
    pub preconditions: Vec<ExecutionPrecondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateTargetRequest {
    Session(SessionTargetRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduledTarget {
    AgentTurn(AgentTurnTarget),
    DeliveryMessage(DeliveryMessageTarget),
}

impl ScheduledTarget {
    pub fn agent_turn(&self) -> Option<&AgentTurnTarget> {
        match self {
            Self::AgentTurn(target) => Some(target),
            Self::DeliveryMessage(_) => None,
        }
    }

    pub fn delivery_message(&self) -> Option<&DeliveryMessageTarget> {
        match self {
            Self::AgentTurn(_) => None,
            Self::DeliveryMessage(target) => Some(target),
        }
    }

    pub fn session_key(&self) -> &str {
        match self {
            Self::AgentTurn(target) => &target.session_key,
            Self::DeliveryMessage(target) => &target.session_key,
        }
    }

    pub fn executor_agent(&self) -> Option<&str> {
        match self {
            Self::AgentTurn(target) => target.agent.as_deref(),
            Self::DeliveryMessage(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    HumanCli,
    HumanChat,
    AgentTool,
    SystemInternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerReason {
    Due,
    RunNow,
    MisfireRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: ScheduleSpec,
    pub timezone: String,
    pub target: ScheduledTarget,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_scheduled_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub run_now_requested_at: Option<DateTime<Utc>>,
    pub max_retries: u32,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub running_since: Option<DateTime<Utc>>,
    pub source_kind: SourceKind,
    pub source_actor: String,
    pub source_session_key: Option<String>,
    pub created_via: String,
    pub requested_by_role: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledRun {
    pub id: String,
    pub job_id: String,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub attempt: u32,
    pub error: Option<String>,
    pub result_summary: Option<String>,
    pub trigger_reason: TriggerReason,
    pub executor_session_key: Option<String>,
    pub executor_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateJobRequest {
    pub name: String,
    pub schedule: ScheduleInput,
    pub timezone: Option<String>,
    pub target: CreateTargetRequest,
    pub max_retries: u32,
    pub source_kind: SourceKind,
    pub source_actor: String,
    pub source_session_key: Option<String>,
    pub created_via: String,
    pub requested_by_role: Option<String>,
}
