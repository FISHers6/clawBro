use crate::{
    contract::{
        RuntimeContext, RuntimeEvent, RuntimeRole, RuntimeSessionSpec, ToolSurfaceSpec, TurnIntent,
        TurnMode, TurnResult,
    },
    event_sink::RuntimeEventSink,
    registry::BackendRegistry,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct RuntimeConductor {
    registry: Arc<BackendRegistry>,
}

impl RuntimeConductor {
    pub fn new(registry: Arc<BackendRegistry>) -> Self {
        Self { registry }
    }

    pub async fn execute(&self, intent: TurnIntent) -> anyhow::Result<TurnResult> {
        let backend_id = resolve_backend_id(&intent)?;
        let session = RuntimeSessionSpec {
            backend_id: backend_id.clone(),
            participant_name: None,
            session_key: intent.session_key.clone(),
            role: role_from_intent(&intent),
            workspace_dir: None,
            prompt_text: intent.user_text.clone(),
            tool_surface: tool_surface_from_intent(&intent),
            approval_mode: Default::default(),
            tool_bridge_url: None,
            external_mcp_servers: vec![],
            team_tool_url: None,
            provider_profile: None,
            context: RuntimeContext::default(),
            backend_session_id: None,
        };
        self.execute_prepared(intent, session).await
    }

    pub async fn execute_prepared(
        &self,
        intent: TurnIntent,
        session: RuntimeSessionSpec,
    ) -> anyhow::Result<TurnResult> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = RuntimeEventSink::new(tx);
        let mut result = self
            .execute_prepared_streaming(intent, session, sink)
            .await?;

        let mut drained = Vec::new();
        while let Ok(event) = rx.try_recv() {
            drained.push(event);
        }

        if result.events.is_empty() {
            result.events = drained;
        } else if !drained.is_empty() {
            let mut events = drained;
            events.extend(result.events);
            result.events = events;
        }

        if !result.events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::TurnComplete { .. } | RuntimeEvent::TurnFailed { .. }
            )
        }) && !result.full_text.is_empty()
        {
            result.events.push(RuntimeEvent::TurnComplete {
                full_text: result.full_text.clone(),
            });
        }

        Ok(result)
    }

    pub async fn execute_prepared_streaming(
        &self,
        intent: TurnIntent,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult> {
        let backend_id = session.backend_id.clone();
        let spec = self
            .registry
            .backend_spec(&backend_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("backend '{}' not registered", backend_id))?;
        let adapter = self.registry.adapter_for(&spec).await?;
        let profile = self.registry.probe_backend(&backend_id).await?;
        ensure_role_eligibility(&backend_id, &intent, &session, &profile)?;
        adapter.run_turn(&spec, session, sink).await
    }
}

fn ensure_role_eligibility(
    backend_id: &str,
    intent: &TurnIntent,
    session: &RuntimeSessionSpec,
    profile: &crate::backend::CapabilityProfile,
) -> anyhow::Result<()> {
    let allowed = match intent.mode {
        TurnMode::Solo => profile.role_eligibility.solo,
        TurnMode::Relay => profile.role_eligibility.relay,
        TurnMode::Team => match session.role {
            RuntimeRole::Leader => profile.role_eligibility.lead,
            RuntimeRole::Specialist => profile.role_eligibility.specialist,
            RuntimeRole::Solo => profile.role_eligibility.solo,
        },
    };

    if allowed {
        return Ok(());
    }

    let role = match intent.mode {
        TurnMode::Solo => "solo",
        TurnMode::Relay => "relay",
        TurnMode::Team => match session.role {
            RuntimeRole::Leader => "team-leader",
            RuntimeRole::Specialist => "team-specialist",
            RuntimeRole::Solo => "solo",
        },
    };

    anyhow::bail!(
        "backend '{}' is not eligible for runtime role '{}'",
        backend_id,
        role
    )
}

fn resolve_backend_id(intent: &TurnIntent) -> anyhow::Result<String> {
    intent
        .target_backend
        .clone()
        .or_else(|| intent.leader_candidate.clone())
        .ok_or_else(|| anyhow::anyhow!("turn intent did not resolve a backend target"))
}

fn role_from_intent(intent: &TurnIntent) -> RuntimeRole {
    match intent.mode {
        TurnMode::Solo => RuntimeRole::Solo,
        TurnMode::Relay => RuntimeRole::Specialist,
        TurnMode::Team => {
            if intent.target_backend == intent.leader_candidate {
                RuntimeRole::Leader
            } else {
                RuntimeRole::Specialist
            }
        }
    }
}

fn tool_surface_from_intent(intent: &TurnIntent) -> ToolSurfaceSpec {
    ToolSurfaceSpec {
        team_tools: matches!(intent.mode, TurnMode::Team),
        local_skills: true,
        external_mcp: false,
        backend_native_tools: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapter::{BackendAdapter, LaunchSpec},
        backend::{
            BackendFamily, CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
        },
        contract::{RuntimeEvent, TurnIntent},
        registry::{BackendRegistry, BackendSpec},
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CapturedSessions {
        roles: Mutex<Vec<RuntimeRole>>,
    }

    struct FakeAdapter {
        captured: Arc<CapturedSessions>,
        profile: CapabilityProfile,
    }

    #[async_trait::async_trait(?Send)]
    impl BackendAdapter for FakeAdapter {
        async fn probe(&self, _spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
            Ok(self.profile.clone())
        }

        async fn run_turn(
            &self,
            _spec: &BackendSpec,
            session: RuntimeSessionSpec,
            sink: RuntimeEventSink,
        ) -> anyhow::Result<TurnResult> {
            self.captured.roles.lock().unwrap().push(session.role);
            sink.emit(RuntimeEvent::TextDelta {
                text: "hello ".into(),
            })?;
            sink.emit(RuntimeEvent::TurnComplete {
                full_text: "hello world".into(),
            })?;
            Ok(TurnResult {
                full_text: "hello world".into(),
                events: Vec::new(),
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: None,
                resume_recovery: None,
            })
        }
    }

    #[tokio::test]
    async fn conductor_translates_intent_to_runtime_event_stream() {
        let registry = Arc::new(BackendRegistry::new());
        let captured = Arc::new(CapturedSessions::default());
        registry
            .register_adapter(
                "fake",
                Arc::new(FakeAdapter {
                    captured: Arc::clone(&captured),
                    profile: CapabilityProfile {
                        streaming: true,
                        workspace_native_contract: false,
                        tool_bridge: ToolBridgeKind::None,
                        native_team: NativeTeamCapability::Unsupported,
                        role_eligibility: RoleEligibility {
                            solo: true,
                            relay: true,
                            specialist: true,
                            lead: true,
                        },
                    },
                }),
            )
            .await;
        registry
            .register_backend(BackendSpec {
                backend_id: "codex".into(),
                family: BackendFamily::Acp,
                adapter_key: "fake".into(),
                launch: LaunchSpec::Embedded,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;
        let conductor = RuntimeConductor::new(Arc::clone(&registry));

        let result = conductor
            .execute(TurnIntent {
                session_key: qai_protocol::SessionKey::new("lark", "group:test"),
                mode: TurnMode::Team,
                leader_candidate: Some("codex".into()),
                target_backend: Some("codex".into()),
                user_text: "ship it".into(),
            })
            .await
            .unwrap();

        assert_eq!(result.full_text, "hello world");
        assert_eq!(
            captured.roles.lock().unwrap().as_slice(),
            &[RuntimeRole::Leader]
        );
        assert!(matches!(
            result.events.as_slice(),
            [
                RuntimeEvent::TextDelta { .. },
                RuntimeEvent::TurnComplete { .. }
            ]
        ));
    }

    #[tokio::test]
    async fn conductor_uses_specialist_role_for_non_leader_team_target() {
        let registry = Arc::new(BackendRegistry::new());
        let captured = Arc::new(CapturedSessions::default());
        registry
            .register_adapter(
                "fake",
                Arc::new(FakeAdapter {
                    captured: Arc::clone(&captured),
                    profile: CapabilityProfile {
                        streaming: true,
                        workspace_native_contract: false,
                        tool_bridge: ToolBridgeKind::None,
                        native_team: NativeTeamCapability::Unsupported,
                        role_eligibility: RoleEligibility {
                            solo: true,
                            relay: true,
                            specialist: true,
                            lead: true,
                        },
                    },
                }),
            )
            .await;
        registry
            .register_backend(BackendSpec {
                backend_id: "openclaw-team".into(),
                family: BackendFamily::OpenClawGateway,
                adapter_key: "fake".into(),
                launch: LaunchSpec::GatewayWs {
                    endpoint: "ws://127.0.0.1:18789".into(),
                    token: None,
                    password: None,
                    role: None,
                    scopes: vec![],
                    agent_id: None,
                    team_helper_command: None,
                    team_helper_args: vec![],
                    lead_helper_mode: false,
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;
        let conductor = RuntimeConductor::new(Arc::clone(&registry));

        conductor
            .execute(TurnIntent {
                session_key: qai_protocol::SessionKey::new("relay", "group:test:openclaw"),
                mode: TurnMode::Team,
                leader_candidate: Some("claude".into()),
                target_backend: Some("openclaw-team".into()),
                user_text: "subtask".into(),
            })
            .await
            .unwrap();

        assert_eq!(
            captured.roles.lock().unwrap().as_slice(),
            &[RuntimeRole::Specialist]
        );
    }

    #[tokio::test]
    async fn conductor_rejects_team_lead_when_backend_profile_disallows_it() {
        let registry = Arc::new(BackendRegistry::new());
        let captured = Arc::new(CapturedSessions::default());
        registry
            .register_adapter(
                "fake",
                Arc::new(FakeAdapter {
                    captured: Arc::clone(&captured),
                    profile: CapabilityProfile {
                        streaming: true,
                        workspace_native_contract: false,
                        tool_bridge: ToolBridgeKind::None,
                        native_team: NativeTeamCapability::Unsupported,
                        role_eligibility: RoleEligibility {
                            solo: true,
                            relay: true,
                            specialist: false,
                            lead: false,
                        },
                    },
                }),
            )
            .await;
        registry
            .register_backend(BackendSpec {
                backend_id: "openclaw-main".into(),
                family: BackendFamily::OpenClawGateway,
                adapter_key: "fake".into(),
                launch: LaunchSpec::GatewayWs {
                    endpoint: "ws://127.0.0.1:18789".into(),
                    token: None,
                    password: None,
                    role: None,
                    scopes: vec![],
                    agent_id: None,
                    team_helper_command: None,
                    team_helper_args: vec![],
                    lead_helper_mode: false,
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;
        let conductor = RuntimeConductor::new(Arc::clone(&registry));

        let err = conductor
            .execute(TurnIntent {
                session_key: qai_protocol::SessionKey::new("lark", "group:test"),
                mode: TurnMode::Team,
                leader_candidate: Some("openclaw-main".into()),
                target_backend: Some("openclaw-main".into()),
                user_text: "plan this".into(),
            })
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("backend 'openclaw-main' is not eligible for runtime role 'team-leader'"));
        assert!(captured.roles.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn conductor_allows_relay_when_backend_is_relay_only() {
        let registry = Arc::new(BackendRegistry::new());
        let captured = Arc::new(CapturedSessions::default());
        registry
            .register_adapter(
                "fake",
                Arc::new(FakeAdapter {
                    captured: Arc::clone(&captured),
                    profile: CapabilityProfile {
                        streaming: true,
                        workspace_native_contract: false,
                        tool_bridge: ToolBridgeKind::None,
                        native_team: NativeTeamCapability::Unsupported,
                        role_eligibility: RoleEligibility {
                            solo: true,
                            relay: true,
                            specialist: false,
                            lead: false,
                        },
                    },
                }),
            )
            .await;
        registry
            .register_backend(BackendSpec {
                backend_id: "relay-backend".into(),
                family: BackendFamily::OpenClawGateway,
                adapter_key: "fake".into(),
                launch: LaunchSpec::GatewayWs {
                    endpoint: "ws://127.0.0.1:18789".into(),
                    token: None,
                    password: None,
                    role: None,
                    scopes: vec![],
                    agent_id: None,
                    team_helper_command: None,
                    team_helper_args: vec![],
                    lead_helper_mode: false,
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;
        let conductor = RuntimeConductor::new(Arc::clone(&registry));

        let result = conductor
            .execute(TurnIntent {
                session_key: qai_protocol::SessionKey::new("relay", "group:test:relay"),
                mode: TurnMode::Relay,
                leader_candidate: None,
                target_backend: Some("relay-backend".into()),
                user_text: "subtask".into(),
            })
            .await
            .unwrap();

        assert_eq!(result.full_text, "hello world");
        assert_eq!(
            captured.roles.lock().unwrap().as_slice(),
            &[RuntimeRole::Specialist]
        );
    }
}
