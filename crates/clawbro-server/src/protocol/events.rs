use crate::agent_core::team::completion_routing::{PendingRoutingRecord, TeamRoutingEnvelope};
use crate::agent_core::team::session::{ChannelSendRecord, LeaderUpdateRecord};
use crate::diagnostics::{BackendDiagnostic, ChannelDiagnostic};
use crate::runtime::{ApprovalDecision, PermissionRequest};
use crate::scheduler::{ScheduledJob, ScheduledRun};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Agent 执行过程中产生的事件（用于 WebSocket 流式推送）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    TextDelta {
        session_id: Uuid,
        delta: String,
    },
    ApprovalRequest {
        session_id: Uuid,
        session_key: crate::protocol::SessionKey,
        approval_id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at_ms: Option<u64>,
    },
    ToolCallStart {
        session_id: Uuid,
        tool_name: String,
        call_id: String,
    },
    ToolCallResult {
        session_id: Uuid,
        call_id: String,
        result: String,
    },
    ToolCallFailed {
        session_id: Uuid,
        tool_name: String,
        call_id: String,
        error: String,
    },
    Thinking {
        session_id: Uuid,
    },
    TurnComplete {
        session_id: Uuid,
        full_text: String,
        #[serde(default)]
        sender: Option<String>,
    },
    Error {
        session_id: Uuid,
        message: String,
    },
}

impl AgentEvent {
    pub fn session_id(&self) -> Uuid {
        match self {
            Self::TextDelta { session_id, .. } => *session_id,
            Self::ApprovalRequest { session_id, .. } => *session_id,
            Self::ToolCallStart { session_id, .. } => *session_id,
            Self::ToolCallResult { session_id, .. } => *session_id,
            Self::ToolCallFailed { session_id, .. } => *session_id,
            Self::Thinking { session_id } => *session_id,
            Self::TurnComplete { session_id, .. } => *session_id,
            Self::Error { session_id, .. } => *session_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsTopic {
    Approvals,
    Backends,
    Backend {
        backend_id: String,
    },
    Channels,
    Channel {
        channel: String,
    },
    Session {
        session_key: crate::protocol::SessionKey,
    },
    Scheduler,
    SchedulerJob {
        job_id: String,
    },
    Team {
        team_id: String,
    },
    Task {
        team_id: String,
        task_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryEvent {
    pub session_id: String,
    pub session_key: crate::protocol::SessionKey,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    ApprovalPending {
        request: PermissionRequest,
    },
    ApprovalResolved {
        approval_id: String,
        decision: ApprovalDecision,
        resolved: bool,
    },
    SessionUpdated {
        summary: SessionSummaryEvent,
    },
    BackendUpdated {
        backend: BackendDiagnostic,
    },
    ChannelUpdated {
        channel: ChannelDiagnostic,
    },
    SchedulerJobUpdated {
        job: ScheduledJob,
    },
    SchedulerJobDeleted {
        job_id: String,
    },
    SchedulerRunUpdated {
        run: ScheduledRun,
    },
    TeamLeaderUpdate {
        team_id: String,
        record: LeaderUpdateRecord,
    },
    TeamChannelSend {
        team_id: String,
        record: ChannelSendRecord,
    },
    TeamRoutingEvent {
        team_id: String,
        event: TeamRoutingEnvelope,
    },
    TeamPendingCompletion {
        team_id: String,
        record: PendingRoutingRecord,
    },
    TaskUpdated {
        team_id: String,
        task: crate::agent_core::team::registry::Task,
    },
}

impl DashboardEvent {
    pub fn matches_topic(&self, topic: &WsTopic) -> bool {
        match (self, topic) {
            (Self::ApprovalPending { .. }, WsTopic::Approvals)
            | (Self::ApprovalResolved { .. }, WsTopic::Approvals) => true,
            (Self::BackendUpdated { .. }, WsTopic::Backends) => true,
            (Self::BackendUpdated { backend }, WsTopic::Backend { backend_id }) => {
                backend.backend_id == *backend_id
            }
            (Self::ChannelUpdated { .. }, WsTopic::Channels) => true,
            (Self::ChannelUpdated { channel }, WsTopic::Channel { channel: target }) => {
                channel.channel == *target
            }
            (Self::SessionUpdated { summary }, WsTopic::Session { session_key }) => {
                summary.session_key == *session_key
            }
            (Self::SchedulerJobUpdated { .. }, WsTopic::Scheduler)
            | (Self::SchedulerJobDeleted { .. }, WsTopic::Scheduler)
            | (Self::SchedulerRunUpdated { .. }, WsTopic::Scheduler) => true,
            (Self::SchedulerJobUpdated { job }, WsTopic::SchedulerJob { job_id }) => {
                job.id == *job_id
            }
            (Self::SchedulerJobDeleted { job_id: deleted }, WsTopic::SchedulerJob { job_id }) => {
                deleted == job_id
            }
            (Self::SchedulerRunUpdated { run }, WsTopic::SchedulerJob { job_id }) => {
                run.job_id == *job_id
            }
            (Self::TeamLeaderUpdate { team_id, .. }, WsTopic::Team { team_id: target })
            | (Self::TeamChannelSend { team_id, .. }, WsTopic::Team { team_id: target })
            | (Self::TeamRoutingEvent { team_id, .. }, WsTopic::Team { team_id: target })
            | (Self::TeamPendingCompletion { team_id, .. }, WsTopic::Team { team_id: target })
            | (Self::TaskUpdated { team_id, .. }, WsTopic::Team { team_id: target }) => {
                team_id == target
            }
            (
                Self::TeamLeaderUpdate { team_id, record },
                WsTopic::Task {
                    team_id: target_team,
                    task_id: target_task,
                },
            ) => team_id == target_team && record.task_id.as_deref() == Some(target_task.as_str()),
            (
                Self::TeamChannelSend { team_id, record },
                WsTopic::Task {
                    team_id: target_team,
                    task_id: target_task,
                },
            ) => team_id == target_team && record.task_id.as_deref() == Some(target_task.as_str()),
            (
                Self::TeamRoutingEvent { team_id, event },
                WsTopic::Task {
                    team_id: target_team,
                    task_id: target_task,
                },
            ) => team_id == target_team && event.event.task_id == *target_task,
            (
                Self::TeamPendingCompletion { team_id, record },
                WsTopic::Task {
                    team_id: target_team,
                    task_id: target_task,
                },
            ) => team_id == target_team && record.envelope.event.task_id == *target_task,
            (
                Self::TaskUpdated { team_id, task },
                WsTopic::Task {
                    team_id: target_team,
                    task_id: target_task,
                },
            ) => team_id == target_team && task.id == *target_task,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::team::completion_routing::{RoutingDeliveryStatus, TeamRoutingEvent};
    use crate::agent_core::team::session::{ChannelSendSourceKind, ChannelSendStatus};

    #[test]
    fn test_turn_complete_sender_default_none() {
        // Old-format JSON (no sender field) must deserialize with sender=None (backward compat)
        let json = r#"{"type":"TurnComplete","session_id":"00000000-0000-0000-0000-000000000001","full_text":"hello"}"#;
        let event: AgentEvent = serde_json::from_str(json).unwrap();
        if let AgentEvent::TurnComplete { sender, .. } = event {
            assert!(
                sender.is_none(),
                "legacy JSON should deserialize with sender=None"
            );
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn test_approval_request_round_trip() {
        let event = AgentEvent::ApprovalRequest {
            session_id: Uuid::nil(),
            session_key: crate::protocol::SessionKey::new("ws", "approval"),
            approval_id: "approval-1".into(),
            prompt: "Allow `git status`?".into(),
            command: Some("git status".into()),
            cwd: Some("/tmp".into()),
            host: Some("gateway".into()),
            agent_id: Some("main".into()),
            expires_at_ms: Some(123),
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        match decoded {
            AgentEvent::ApprovalRequest {
                approval_id,
                session_key,
                command,
                expires_at_ms,
                ..
            } => {
                assert_eq!(approval_id, "approval-1");
                assert_eq!(
                    session_key,
                    crate::protocol::SessionKey::new("ws", "approval")
                );
                assert_eq!(command.as_deref(), Some("git status"));
                assert_eq!(expires_at_ms, Some(123));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn dashboard_event_matches_team_and_task_topics() {
        let approval = DashboardEvent::ApprovalPending {
            request: PermissionRequest {
                id: "approval-1".into(),
                prompt: "allow?".into(),
                command: None,
                cwd: None,
                host: None,
                agent_id: None,
                expires_at_ms: None,
            },
        };
        assert!(approval.matches_topic(&WsTopic::Approvals));

        let backend = DashboardEvent::BackendUpdated {
            backend: BackendDiagnostic {
                backend_id: "claude-main".into(),
                family: crate::runtime::BackendFamily::Acp,
                adapter_key: "acp".into(),
                registered: true,
                adapter_registered: true,
                probed: true,
                healthy: true,
                error: None,
                capability_profile: None,
                acp_backend: Some(crate::runtime::AcpBackend::Claude),
                acp_support_category: Some(
                    crate::diagnostics::AcpSupportCategory::SupportedWithBridge,
                ),
                notes: vec![],
            },
        };
        assert!(backend.matches_topic(&WsTopic::Backends));
        assert!(backend.matches_topic(&WsTopic::Backend {
            backend_id: "claude-main".into()
        }));

        let channel = DashboardEvent::ChannelUpdated {
            channel: ChannelDiagnostic {
                channel: "lark".into(),
                configured: true,
                enabled: true,
                routing_present: true,
                credential_state: "unknown".into(),
                notes: vec![],
            },
        };
        assert!(channel.matches_topic(&WsTopic::Channels));
        assert!(channel.matches_topic(&WsTopic::Channel {
            channel: "lark".into()
        }));

        let session = DashboardEvent::SessionUpdated {
            summary: SessionSummaryEvent {
                session_id: "sid-1".into(),
                session_key: crate::protocol::SessionKey::new("lark", "group:oc_demo"),
                created_at: "2026-03-21T00:00:00Z".into(),
                updated_at: "2026-03-21T00:00:00Z".into(),
                message_count: 1,
                status: "idle".into(),
                backend_id: None,
            },
        };
        assert!(session.matches_topic(&WsTopic::Session {
            session_key: crate::protocol::SessionKey::new("lark", "group:oc_demo")
        }));

        let scheduler_job = DashboardEvent::SchedulerJobUpdated {
            job: ScheduledJob {
                id: "job-1".into(),
                name: "daily".into(),
                enabled: true,
                schedule: crate::scheduler::ScheduleSpec::Every { interval_ms: 1000 },
                timezone: "Asia/Shanghai".into(),
                target: crate::scheduler::ScheduledTarget::DeliveryMessage(
                    crate::scheduler::DeliveryMessageTarget {
                        session_key: "lark:group:oc_demo".into(),
                        message: "hi".into(),
                    },
                ),
                next_run_at: None,
                last_scheduled_at: None,
                last_run_at: None,
                last_success_at: None,
                run_now_requested_at: None,
                max_retries: 0,
                lease_token: None,
                lease_expires_at: None,
                running_since: None,
                source_kind: crate::scheduler::SourceKind::HumanCli,
                source_actor: "tester".into(),
                source_session_key: None,
                created_via: "cli".into(),
                requested_by_role: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        };
        assert!(scheduler_job.matches_topic(&WsTopic::Scheduler));
        assert!(scheduler_job.matches_topic(&WsTopic::SchedulerJob {
            job_id: "job-1".into()
        }));

        let leader = DashboardEvent::TeamLeaderUpdate {
            team_id: "team-1".into(),
            record: LeaderUpdateRecord {
                event_id: "evt-1".into(),
                ts: "2026-03-21T00:00:00Z".into(),
                team_id: "team-1".into(),
                lead_session_channel: None,
                lead_session_channel_instance: None,
                lead_session_scope: None,
                lead_reply_to: None,
                lead_thread_ts: None,
                lead_turn_id: None,
                source_agent: "claude".into(),
                kind: crate::agent_core::team::session::LeaderUpdateKind::PostUpdate,
                text: "update".into(),
                task_id: Some("T001".into()),
                channel_send_event_id: None,
                session_message_id: None,
            },
        };
        assert!(leader.matches_topic(&WsTopic::Team {
            team_id: "team-1".into()
        }));
        assert!(leader.matches_topic(&WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T001".into()
        }));
        assert!(!leader.matches_topic(&WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T002".into()
        }));

        let send = DashboardEvent::TeamChannelSend {
            team_id: "team-1".into(),
            record: ChannelSendRecord {
                event_id: "send-1".into(),
                ts: "2026-03-21T00:00:00Z".into(),
                channel: "lark".into(),
                sender_channel_instance: None,
                target_channel_instance: None,
                target_scope: "group:oc_demo".into(),
                team_id: "team-1".into(),
                lead_session_channel: None,
                lead_session_channel_instance: None,
                lead_session_scope: None,
                lead_reply_to: None,
                lead_thread_ts: None,
                reply_to: None,
                thread_ts: None,
                source_kind: ChannelSendSourceKind::LeadText,
                source_agent: "claude".into(),
                task_id: Some("T001".into()),
                dedupe_key: None,
                text: "hello".into(),
                status: ChannelSendStatus::Sent,
                provider_message_id: None,
                error: None,
            },
        };
        assert!(send.matches_topic(&WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T001".into()
        }));

        let routing = DashboardEvent::TeamRoutingEvent {
            team_id: "team-1".into(),
            event: TeamRoutingEnvelope {
                run_id: "run-1".into(),
                parent_run_id: None,
                requester_session_key: Some(crate::protocol::SessionKey::new(
                    "lark",
                    "group:oc_demo",
                )),
                fallback_session_keys: vec![],
                delivery_source: None,
                team_id: "team-1".into(),
                delivery_status: RoutingDeliveryStatus::PersistedPending,
                event: TeamRoutingEvent::submitted("T001", "claw", "done"),
            },
        };
        assert!(routing.matches_topic(&WsTopic::Task {
            team_id: "team-1".into(),
            task_id: "T001".into()
        }));
    }
}
