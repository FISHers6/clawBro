use qai_agent::{
    team::{
        heartbeat::DispatchFn,
        orchestrator::TeamOrchestrator,
        registry::TaskRegistry,
        session::{stable_team_id_for_session_key, TeamSession},
    },
    AgentEntry, AgentRoster, ConductorRuntimeDispatch, SessionRegistry,
};
use qai_protocol::{render_scope_storage_key, InboundMsg, MsgContent, MsgSource, SessionKey};
use qai_runtime::{
    BackendFamily, BackendRegistry, BackendSpec, CapabilityProfile, LaunchSpec,
    NativeTeamCapability, RoleEligibility, RuntimeEvent, ScriptedAdapter, ScriptedTurn,
    TeamCallback, TeamToolCall, ToolBridgeKind,
};
use qai_session::{SessionManager, SessionStorage};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

struct TeamHarness {
    _root: TempDir,
    registry: Arc<SessionRegistry>,
    orchestrator: Arc<TeamOrchestrator>,
    lead_key: SessionKey,
    specialist_key: SessionKey,
    _event_rx: broadcast::Receiver<qai_protocol::AgentEvent>,
}

fn family_profile(family: BackendFamily, lead: bool, specialist: bool) -> CapabilityProfile {
    CapabilityProfile {
        streaming: true,
        workspace_native_contract: !matches!(family, BackendFamily::Acp),
        tool_bridge: match family {
            BackendFamily::Acp => ToolBridgeKind::Mcp,
            BackendFamily::OpenClawGateway | BackendFamily::QuickAiNative => {
                ToolBridgeKind::BackendNative
            }
        },
        native_team: match family {
            BackendFamily::QuickAiNative => NativeTeamCapability::Unsupported,
            BackendFamily::Acp | BackendFamily::OpenClawGateway => {
                NativeTeamCapability::SupportedButDisabled
            }
        },
        role_eligibility: RoleEligibility {
            solo: true,
            relay: true,
            specialist,
            lead,
        },
    }
}

fn write_scoped_memory(dir: &Path, session_key: &SessionKey, content: &str) {
    let memory_dir = dir.join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    let scope_key = render_scope_storage_key(session_key);
    std::fs::write(memory_dir.join(format!("{scope_key}.md")), content).unwrap();
}

async fn build_team_harness(
    lead_backend: BackendSpec,
    lead_adapter_key: &str,
    lead_adapter: Arc<ScriptedAdapter>,
    specialist_backend: BackendSpec,
    specialist_adapter_key: &str,
    specialist_adapter: Arc<ScriptedAdapter>,
) -> TeamHarness {
    let root = tempfile::tempdir().unwrap();
    let scope = format!("group:mixed-team:{}", uuid::Uuid::new_v4());
    let lead_key = SessionKey::new("ws", &scope);
    let team_id = stable_team_id_for_session_key(&lead_key);
    let specialist_key = SessionKey::new("specialist", format!("{team_id}:worker"));

    let leader_persona = root.path().join("leader-persona");
    let worker_persona = root.path().join("worker-persona");
    let leader_workspace = root.path().join("leader-workspace");
    let worker_workspace = root.path().join("worker-workspace");
    std::fs::create_dir_all(&leader_persona).unwrap();
    std::fs::create_dir_all(&worker_persona).unwrap();
    std::fs::create_dir_all(&leader_workspace).unwrap();
    std::fs::create_dir_all(&worker_workspace).unwrap();
    std::fs::write(leader_persona.join("SOUL.md"), "leader-soul").unwrap();
    std::fs::write(leader_persona.join("IDENTITY.md"), "leader-identity").unwrap();
    std::fs::write(leader_persona.join("MEMORY.md"), "leader-long-term-memory").unwrap();
    std::fs::write(worker_persona.join("SOUL.md"), "worker-soul").unwrap();
    std::fs::write(worker_persona.join("IDENTITY.md"), "worker-identity").unwrap();
    std::fs::write(worker_persona.join("MEMORY.md"), "worker-long-term-memory").unwrap();
    write_scoped_memory(&leader_persona, &lead_key, "leader-private-memory");
    write_scoped_memory(&worker_persona, &specialist_key, "worker-private-memory");
    std::fs::write(
        leader_workspace.join("AGENTS.md"),
        "leader workspace agents",
    )
    .unwrap();
    std::fs::write(leader_workspace.join("USER.md"), "leader workspace user").unwrap();
    std::fs::write(
        leader_workspace.join("HEARTBEAT.md"),
        "leader workspace heartbeat",
    )
    .unwrap();
    std::fs::write(
        worker_workspace.join("AGENTS.md"),
        "worker workspace agents",
    )
    .unwrap();
    std::fs::write(worker_workspace.join("USER.md"), "worker workspace user").unwrap();
    std::fs::write(
        worker_workspace.join("HEARTBEAT.md"),
        "worker workspace heartbeat",
    )
    .unwrap();

    let storage = SessionStorage::new(root.path().join("sessions"));
    let session_manager = Arc::new(SessionManager::new(storage));

    let runtime_registry = Arc::new(BackendRegistry::new());
    runtime_registry
        .register_adapter(lead_adapter_key, lead_adapter)
        .await;
    if lead_adapter_key != specialist_adapter_key {
        runtime_registry
            .register_adapter(specialist_adapter_key, specialist_adapter)
            .await;
    } else {
        runtime_registry
            .register_adapter(specialist_adapter_key, specialist_adapter)
            .await;
    }
    runtime_registry.register_backend(lead_backend).await;
    runtime_registry.register_backend(specialist_backend).await;

    let roster = AgentRoster::new(vec![
        AgentEntry {
            name: "leader".into(),
            mentions: vec!["@leader".into()],
            backend_id: "leader-main".into(),
            persona_dir: Some(leader_persona),
            workspace_dir: Some(leader_workspace),
            extra_skills_dirs: vec![],
        },
        AgentEntry {
            name: "worker".into(),
            mentions: vec!["@worker".into()],
            backend_id: "worker-main".into(),
            persona_dir: Some(worker_persona),
            workspace_dir: Some(worker_workspace),
            extra_skills_dirs: vec![],
        },
    ]);

    let (registry, event_rx) = SessionRegistry::with_runtime_dispatch(
        None,
        session_manager,
        String::new(),
        Some(roster),
        None,
        None,
        None,
        vec![],
        Arc::new(ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry))),
    );
    registry.set_team_tool_url("http://127.0.0.1:3000/runtime/team-tools?token=test".into());

    let session = Arc::new(TeamSession::from_dir(
        &team_id,
        root.path().join("team-session"),
    ));
    std::fs::create_dir_all(&session.dir).unwrap();
    session.write_context_md("shared-team-context").unwrap();
    session.write_team_md("team-manifest").unwrap();
    session.write_heartbeat_md("heartbeat-checklist").unwrap();
    session.write_agents_md("team agents").unwrap();

    let task_registry = Arc::new(TaskRegistry::new_in_memory().unwrap());
    let registry_for_dispatch = Arc::clone(&registry);
    let task_registry_for_dispatch = Arc::clone(&task_registry);
    let session_for_dispatch = Arc::clone(&session);
    let dispatch_fn: DispatchFn = Arc::new(move |agent: String, task| {
        let registry = Arc::clone(&registry_for_dispatch);
        let task_registry = Arc::clone(&task_registry_for_dispatch);
        let session = Arc::clone(&session_for_dispatch);
        Box::pin(async move {
            let specialist_key = session.specialist_session_key(&agent);
            let reminder = session.build_task_reminder(&task, &task_registry);
            registry.set_task_reminder(specialist_key.clone(), reminder);
            registry
                .handle(InboundMsg {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_key: specialist_key,
                    content: MsgContent::text(task.spec.as_deref().unwrap_or(&task.title)),
                    sender: "orchestrator".to_string(),
                    channel: "specialist".to_string(),
                    timestamp: chrono::Utc::now(),
                    thread_ts: None,
                    target_agent: Some(format!("@{agent}")),
                    source: MsgSource::Heartbeat,
                })
                .await
                .map(|_| ())
        })
    });

    let orchestrator = TeamOrchestrator::new(
        task_registry,
        Arc::clone(&session),
        dispatch_fn,
        Duration::from_millis(20),
    );
    orchestrator.set_lead_session_key(lead_key.clone());
    orchestrator.set_scope(lead_key.clone());
    orchestrator.set_lead_agent_name("leader".into());
    orchestrator.set_available_specialists(vec!["worker".into()]);
    let _ = orchestrator.mcp_server_port.set(32123);

    let (team_notify_tx, mut team_notify_rx) =
        mpsc::channel::<qai_agent::team::completion_routing::TeamNotifyRequest>(32);
    orchestrator.set_team_notify_tx(team_notify_tx);
    let registry_for_notify = Arc::clone(&registry);
    tokio::spawn(async move {
        while let Some(request) = team_notify_rx.recv().await {
            let requester = request
                .envelope
                .requester_session_key
                .clone()
                .expect("team test expected requester session key");
            let inbound = InboundMsg {
                id: uuid::Uuid::new_v4().to_string(),
                session_key: requester.clone(),
                content: MsgContent::text(request.envelope.event.render_for_parent()),
                sender: "gateway".to_string(),
                channel: requester.channel.clone(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: None,
                source: MsgSource::TeamNotify,
            };
            let _ = registry_for_notify.handle(inbound).await;
        }
    });

    registry.register_team_orchestrator(team_id, Arc::clone(&orchestrator));

    TeamHarness {
        _root: root,
        registry,
        orchestrator,
        lead_key,
        specialist_key,
        _event_rx: event_rx,
    }
}

async fn send_lead_turn(harness: &TeamHarness, text: &str) {
    harness
        .registry
        .handle(InboundMsg {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: harness.lead_key.clone(),
            content: MsgContent::text(text),
            sender: "user".to_string(),
            channel: "ws".to_string(),
            timestamp: chrono::Utc::now(),
            thread_ts: None,
            target_agent: None,
            source: MsgSource::Human,
        })
        .await
        .unwrap();
}

async fn wait_until(timeout: Duration, predicate: impl Fn() -> bool, label: &str) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {label}");
}

fn backend_spec(backend_id: &str, family: BackendFamily, adapter_key: &str) -> BackendSpec {
    BackendSpec {
        backend_id: backend_id.to_string(),
        family,
        adapter_key: adapter_key.to_string(),
        launch: LaunchSpec::BundledCommand,
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend: None,
        acp_auth_method: None,
        codex_projection: None,
        approval_mode: Default::default(),
    }
}

fn has_all_files(capture: &qai_runtime::CapturedTurn, names: &[&str]) -> bool {
    names.iter().all(|name| {
        capture
            .session
            .context
            .workspace_native_files
            .iter()
            .any(|entry| entry == name)
    })
}

fn has_scoped_memory_file(capture: &qai_runtime::CapturedTurn, channel: &str) -> bool {
    let prefix = format!("memory/c={channel}");
    capture
        .session
        .context
        .workspace_native_files
        .iter()
        .any(|entry| entry.starts_with(&prefix) && entry.ends_with(".md"))
}

fn read_task_artifact(harness: &TeamHarness, task_id: &str, name: &str) -> String {
    std::fs::read_to_string(
        harness
            .orchestrator
            .session
            .dir
            .join("tasks")
            .join(task_id)
            .join(name),
    )
    .unwrap()
}

#[tokio::test]
async fn mixed_backend_smoke_acp_lead_native_specialist_submit_and_accept() {
    let lead_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::Acp, true, false),
        |_spec, session| {
            let input = session.context.user_input.as_deref().unwrap_or_default();
            if input.contains("kickoff mixed flow") {
                return Ok(ScriptedTurn {
                    full_text: "acp-lead:planned:T001".into(),
                    events: vec![
                        RuntimeEvent::ToolCallback(TeamCallback::TaskCreated {
                            task_id: "T001".into(),
                            title: "ship feature".into(),
                            assignee: "worker".into(),
                        }),
                        RuntimeEvent::ToolCallback(TeamCallback::TaskAssigned {
                            task_id: "T001".into(),
                            assignee: "worker".into(),
                        }),
                        RuntimeEvent::ToolCallback(TeamCallback::ExecutionStarted),
                    ],
                });
            }
            if input.contains("已提交待验收") {
                return Ok(ScriptedTurn {
                    full_text: "acp-lead:accepted:T001".into(),
                    events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskAccepted {
                        task_id: "T001".into(),
                        by: "leader".into(),
                    })],
                });
            }
            Ok(ScriptedTurn {
                full_text: format!("acp-lead:noop:{input}"),
                events: vec![],
            })
        },
    ));
    let specialist_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::QuickAiNative, false, true),
        |_spec, _session| {
            Ok(ScriptedTurn {
                full_text: "native-worker:submitted:T001".into(),
                events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskSubmitted {
                    task_id: "T001".into(),
                    summary: "worker submitted result".into(),
                    result_markdown: None,
                    agent: "worker".into(),
                })],
            })
        },
    ));

    let harness = build_team_harness(
        backend_spec("leader-main", BackendFamily::Acp, "acp"),
        "acp",
        Arc::clone(&lead_adapter),
        backend_spec("worker-main", BackendFamily::QuickAiNative, "native"),
        "native",
        Arc::clone(&specialist_adapter),
    )
    .await;

    send_lead_turn(&harness, "kickoff mixed flow").await;
    wait_until(
        Duration::from_secs(3),
        || {
            harness
                .orchestrator
                .registry
                .get_task("T001")
                .unwrap()
                .map(|task| task.status_raw.starts_with("accepted:"))
                .unwrap_or(false)
        },
        "task acceptance",
    )
    .await;

    let leader_captures = lead_adapter.captures();
    let specialist_captures = specialist_adapter.captures();
    assert!(leader_captures
        .iter()
        .any(|capture| capture.rendered_prompt.contains("leader-private-memory")));
    assert!(specialist_captures
        .iter()
        .any(|capture| capture.rendered_prompt.contains("shared-team-context")));
    assert!(specialist_captures
        .iter()
        .all(|capture| !capture.rendered_prompt.contains("worker-private-memory")));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.session.team_tool_url.is_some()));
    assert!(leader_captures.iter().any(|capture| {
        capture.session.role == qai_runtime::RuntimeRole::Leader
            && has_all_files(
                capture,
                &[
                    "SOUL.md",
                    "IDENTITY.md",
                    "MEMORY.md",
                    "AGENTS.md",
                    "USER.md",
                    "HEARTBEAT.md",
                    "TEAM.md",
                    "CONTEXT.md",
                ],
            )
            && has_scoped_memory_file(capture, "ws")
    }));
    assert!(specialist_captures.iter().all(|capture| {
        has_all_files(
            capture,
            &[
                "SOUL.md",
                "IDENTITY.md",
                "AGENTS.md",
                "USER.md",
                "HEARTBEAT.md",
                "TEAM.md",
                "CONTEXT.md",
            ],
        ) && has_scoped_memory_file(capture, "specialist")
            && !capture
                .session
                .context
                .workspace_native_files
                .iter()
                .any(|entry| entry == "MEMORY.md")
    }));
    assert_eq!(
        harness.orchestrator.team_state(),
        qai_agent::team::orchestrator::TeamState::Done
    );
    let meta = read_task_artifact(&harness, "T001", "meta.json");
    let spec = read_task_artifact(&harness, "T001", "spec.md");
    let progress = read_task_artifact(&harness, "T001", "progress.md");
    let result = read_task_artifact(&harness, "T001", "result.md");
    assert!(meta.contains("accepted:leader:"));
    assert!(spec.contains("ship feature"));
    assert!(result.contains("worker submitted result"));
    assert!(result.contains("Accepted by: leader"));
    assert!(progress.contains("leader accepted submission"));

    let err = harness
        .registry
        .invoke_team_tool(
            &harness.specialist_key,
            TeamToolCall::CreateTask {
                id: Some("BAD".into()),
                title: "should fail".into(),
                assignee: Some("worker".into()),
                spec: None,
                deps: vec![],
                success_criteria: None,
            },
        )
        .await
        .unwrap_err();
    let err_text = err.to_string();
    assert!(err_text.contains("CreateTask"));
    assert!(err_text.contains("Specialist"));
}

#[tokio::test]
async fn mixed_backend_smoke_native_lead_acp_specialist_submit_and_accept() {
    let lead_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::QuickAiNative, true, false),
        |_spec, session| {
            let input = session.context.user_input.as_deref().unwrap_or_default();
            if input.contains("kickoff reverse flow") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:planned:T001".into(),
                    events: vec![
                        RuntimeEvent::ToolCallback(TeamCallback::TaskCreated {
                            task_id: "T001".into(),
                            title: "ship reverse feature".into(),
                            assignee: "worker".into(),
                        }),
                        RuntimeEvent::ToolCallback(TeamCallback::ExecutionStarted),
                    ],
                });
            }
            if input.contains("已提交待验收") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:accepted:T001".into(),
                    events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskAccepted {
                        task_id: "T001".into(),
                        by: "leader".into(),
                    })],
                });
            }
            Ok(ScriptedTurn::default())
        },
    ));
    let specialist_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::Acp, false, true),
        |_spec, session| {
            assert_eq!(session.role, qai_runtime::RuntimeRole::Specialist);
            Ok(ScriptedTurn {
                full_text: "acp-worker:submitted:T001".into(),
                events: vec![RuntimeEvent::ToolCallback(TeamCallback::TaskSubmitted {
                    task_id: "T001".into(),
                    summary: "acp worker delivered".into(),
                    result_markdown: None,
                    agent: "worker".into(),
                })],
            })
        },
    ));

    let harness = build_team_harness(
        backend_spec("leader-main", BackendFamily::QuickAiNative, "native"),
        "native",
        Arc::clone(&lead_adapter),
        backend_spec("worker-main", BackendFamily::Acp, "acp"),
        "acp",
        Arc::clone(&specialist_adapter),
    )
    .await;

    send_lead_turn(&harness, "kickoff reverse flow").await;
    wait_until(
        Duration::from_secs(3),
        || {
            harness
                .orchestrator
                .registry
                .get_task("T001")
                .unwrap()
                .map(|task| task.status_raw.starts_with("accepted:"))
                .unwrap_or(false)
        },
        "reverse mixed acceptance",
    )
    .await;

    let leader_captures = lead_adapter.captures();
    let specialist_captures = specialist_adapter.captures();
    assert!(leader_captures
        .iter()
        .any(|capture| capture.session.role == qai_runtime::RuntimeRole::Leader));
    assert!(leader_captures.iter().any(|capture| {
        has_all_files(
            capture,
            &[
                "SOUL.md",
                "IDENTITY.md",
                "MEMORY.md",
                "AGENTS.md",
                "TEAM.md",
                "CONTEXT.md",
            ],
        ) && has_scoped_memory_file(capture, "ws")
    }));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.session.role == qai_runtime::RuntimeRole::Specialist));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.session.tool_surface.team_tools));
    assert!(specialist_captures.iter().all(|capture| {
        has_all_files(
            capture,
            &[
                "SOUL.md",
                "IDENTITY.md",
                "AGENTS.md",
                "TEAM.md",
                "CONTEXT.md",
            ],
        ) && has_scoped_memory_file(capture, "specialist")
            && !capture
                .session
                .context
                .workspace_native_files
                .iter()
                .any(|entry| entry == "MEMORY.md")
    }));
    let meta = read_task_artifact(&harness, "T001", "meta.json");
    let result = read_task_artifact(&harness, "T001", "result.md");
    assert!(meta.contains("accepted:leader:"));
    assert!(result.contains("acp worker delivered"));
    assert!(result.contains("Accepted by: leader"));
}

#[tokio::test]
async fn mixed_backend_smoke_openclaw_specialist_uses_canonical_team_surface_and_escalates() {
    let lead_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::QuickAiNative, true, false),
        |_spec, session| {
            let input = session.context.user_input.as_deref().unwrap_or_default();
            if input.contains("kickoff escalation flow") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:planned:T001".into(),
                    events: vec![
                        RuntimeEvent::ToolCallback(TeamCallback::TaskCreated {
                            task_id: "T001".into(),
                            title: "handle escalations".into(),
                            assignee: "worker".into(),
                        }),
                        RuntimeEvent::ToolCallback(TeamCallback::ExecutionStarted),
                    ],
                });
            }
            if input.contains("已更新检查点") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:checkpoint:T001".into(),
                    events: vec![],
                });
            }
            if input.contains("请求协助") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:help:T001".into(),
                    events: vec![],
                });
            }
            if input.contains("已阻塞") {
                return Ok(ScriptedTurn {
                    full_text: "native-lead:blocked:T001".into(),
                    events: vec![],
                });
            }
            Ok(ScriptedTurn::default())
        },
    ));
    let specialist_adapter = Arc::new(ScriptedAdapter::new(
        family_profile(BackendFamily::OpenClawGateway, false, true),
        |_spec, _session| {
            Ok(ScriptedTurn {
                full_text: "openclaw-worker:block:T001".into(),
                events: vec![
                    RuntimeEvent::ToolCallback(TeamCallback::TaskCheckpoint {
                        task_id: "T001".into(),
                        note: "checkpoint progress".into(),
                        agent: "worker".into(),
                    }),
                    RuntimeEvent::ToolCallback(TeamCallback::TaskHelpRequested {
                        task_id: "T001".into(),
                        message: "need a hint".into(),
                        agent: "worker".into(),
                    }),
                    RuntimeEvent::ToolCallback(TeamCallback::TaskBlocked {
                        task_id: "T001".into(),
                        reason: "blocked on external dependency".into(),
                        agent: "worker".into(),
                    }),
                ],
            })
        },
    ));

    let harness = build_team_harness(
        backend_spec("leader-main", BackendFamily::QuickAiNative, "native"),
        "native",
        Arc::clone(&lead_adapter),
        backend_spec("worker-main", BackendFamily::OpenClawGateway, "openclaw"),
        "openclaw",
        Arc::clone(&specialist_adapter),
    )
    .await;

    send_lead_turn(&harness, "kickoff escalation flow").await;
    wait_until(
        Duration::from_secs(3),
        || {
            harness
                .orchestrator
                .registry
                .get_task("T001")
                .unwrap()
                .map(|task| task.status_raw == "pending")
                .unwrap_or(false)
        },
        "blocked task reset",
    )
    .await;
    wait_until(
        Duration::from_secs(3),
        || {
            let leader_inputs: Vec<String> = lead_adapter
                .captures()
                .into_iter()
                .filter_map(|capture| capture.session.context.user_input)
                .collect();
            leader_inputs
                .iter()
                .any(|input| input.contains("已更新检查点"))
                && leader_inputs.iter().any(|input| input.contains("请求协助"))
                && leader_inputs.iter().any(|input| input.contains("已阻塞"))
        },
        "lead escalation notifications",
    )
    .await;

    let leader_inputs: Vec<String> = lead_adapter
        .captures()
        .into_iter()
        .filter_map(|capture| capture.session.context.user_input)
        .collect();
    let specialist_captures = specialist_adapter.captures();

    assert!(leader_inputs
        .iter()
        .any(|input| input.contains("已更新检查点")));
    assert!(leader_inputs.iter().any(|input| input.contains("请求协助")));
    assert!(leader_inputs.iter().any(|input| input.contains("已阻塞")));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.session.team_tool_url.is_some()));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.session.tool_surface.team_tools));
    assert!(specialist_captures
        .iter()
        .all(|capture| capture.rendered_prompt.contains("任务ID: T001")));
    assert!(specialist_captures.iter().all(|capture| {
        has_all_files(
            capture,
            &[
                "SOUL.md",
                "IDENTITY.md",
                "AGENTS.md",
                "TEAM.md",
                "CONTEXT.md",
            ],
        ) && has_scoped_memory_file(capture, "specialist")
            && !capture
                .session
                .context
                .workspace_native_files
                .iter()
                .any(|entry| entry == "MEMORY.md")
    }));
    let meta = read_task_artifact(&harness, "T001", "meta.json");
    let progress = read_task_artifact(&harness, "T001", "progress.md");
    assert!(meta.contains("\"status\": \"pending\""));
    assert!(progress.contains("checkpoint progress"));
    assert!(progress.contains("need a hint"));
    assert!(progress.contains("blocked on external dependency"));
}
