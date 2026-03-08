use crate::traits::AgentCtx;
use anyhow::Result;
use async_trait::async_trait;
use qai_protocol::AgentEvent;
use qai_runtime::{
    acp::AcpBackendAdapter, ApprovalBroker, BackendRegistry, OpenClawBackendAdapter,
    QuickAiNativeBackendAdapter, RuntimeConductor, RuntimeContext, RuntimeEvent, RuntimeRole,
    RuntimeSessionSpec, ToolSurfaceSpec, TurnIntent, TurnResult,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

#[derive(Clone)]
pub struct RuntimeDispatchRequest {
    pub intent: TurnIntent,
    pub ctx: AgentCtx,
    pub fallback_backend_id: Option<String>,
    pub event_tx: broadcast::Sender<AgentEvent>,
}

#[async_trait]
pub trait RuntimeDispatch: Send + Sync {
    async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult>;
}

pub fn default_runtime_dispatch() -> Arc<dyn RuntimeDispatch> {
    let approvals = ApprovalBroker::default();
    let registry = Arc::new(BackendRegistry::new());
    let rt_registry = Arc::clone(&registry);
    futures::executor::block_on(async move {
        rt_registry
            .register_adapter("acp", Arc::new(AcpBackendAdapter::new(approvals.clone())))
            .await;
        rt_registry
            .register_adapter(
                "openclaw",
                Arc::new(OpenClawBackendAdapter::new(approvals.clone())),
            )
            .await;
        rt_registry
            .register_adapter("native", Arc::new(QuickAiNativeBackendAdapter))
            .await;
    });
    Arc::new(ConductorRuntimeDispatch::new(registry))
}

pub struct ConductorRuntimeDispatch {
    worker_pool: RuntimeWorkerPool,
}

impl ConductorRuntimeDispatch {
    pub fn new(registry: Arc<BackendRegistry>) -> Self {
        Self {
            worker_pool: RuntimeWorkerPool::new(registry, default_worker_count()),
        }
    }
}

#[async_trait]
impl RuntimeDispatch for ConductorRuntimeDispatch {
    async fn dispatch(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
        let session_id = request.ctx.session_id;
        let _ = request.event_tx.send(AgentEvent::Thinking { session_id });
        let outer_event_tx = request.event_tx.clone();
        let result = self.worker_pool.execute(request).await;

        match result {
            Ok(turn) => Ok(turn),
            Err(err) => {
                let _ = outer_event_tx.send(AgentEvent::Error {
                    session_id,
                    message: err.to_string(),
                });
                let _ = outer_event_tx.send(AgentEvent::TurnComplete {
                    session_id,
                    full_text: String::new(),
                    sender: None,
                });
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "runtime conductor forced TurnComplete after runtime error"
                );
                Err(err)
            }
        }
    }
}

struct RuntimeDispatchJob {
    request: RuntimeDispatchRequest,
    result_tx: oneshot::Sender<Result<TurnResult>>,
}

struct RuntimeWorkerPool {
    senders: Vec<mpsc::UnboundedSender<RuntimeDispatchJob>>,
    next: AtomicUsize,
}

impl RuntimeWorkerPool {
    fn new(registry: Arc<BackendRegistry>, worker_count: usize) -> Self {
        let worker_count = worker_count.max(1);
        let mut senders = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (tx, rx) = mpsc::unbounded_channel();
            spawn_runtime_worker(worker_index, Arc::clone(&registry), rx);
            senders.push(tx);
        }
        Self {
            senders,
            next: AtomicUsize::new(0),
        }
    }

    async fn execute(&self, request: RuntimeDispatchRequest) -> Result<TurnResult> {
        let (result_tx, result_rx) = oneshot::channel();
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.senders.len();
        self.senders[idx]
            .send(RuntimeDispatchJob { request, result_tx })
            .map_err(|_| anyhow::anyhow!("runtime worker pool is unavailable"))?;
        result_rx
            .await
            .map_err(|_| anyhow::anyhow!("runtime worker dropped turn result"))?
    }
}

fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(2, 8))
        .unwrap_or(4)
}

fn spawn_runtime_worker(
    worker_index: usize,
    registry: Arc<BackendRegistry>,
    mut rx: mpsc::UnboundedReceiver<RuntimeDispatchJob>,
) {
    std::thread::Builder::new()
        .name(format!("qai-runtime-worker-{worker_index}"))
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build runtime worker current_thread runtime");
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                while let Some(job) = rx.recv().await {
                    let result = run_dispatch_job(Arc::clone(&registry), job.request).await;
                    let _ = job.result_tx.send(result);
                }
            });
        })
        .expect("Failed to spawn runtime worker");
}

async fn run_dispatch_job(
    registry: Arc<BackendRegistry>,
    request: RuntimeDispatchRequest,
) -> Result<TurnResult> {
    let session_id = request.ctx.session_id;
    let backend_id = request
        .intent
        .target_backend
        .clone()
        .or_else(|| request.intent.leader_candidate.clone())
        .or(request.fallback_backend_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no backend selected for turn"))?;
    registry
        .backend_spec(&backend_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("backend `{backend_id}` is not registered"))?;

    let conductor = RuntimeConductor::new(Arc::clone(&registry));
    let intent = request.intent;
    let intent_session_key = intent.session_key.clone();
    let ctx = request.ctx;
    let ctx_session_key = ctx.session_key.clone();
    let thread_event_tx = request.event_tx.clone();
    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel();
    let runtime_sink = qai_runtime::RuntimeEventSink::new(runtime_tx);
    let runtime_complete_seen = Arc::new(AtomicBool::new(false));
    let forward_complete_seen = Arc::clone(&runtime_complete_seen);
    let event_tx = thread_event_tx.clone();
    let completion_event_tx = thread_event_tx.clone();
    let forward_session_key = ctx_session_key.clone();
    let (forward_done_tx, forward_done_rx) = oneshot::channel();
    tokio::task::spawn_local(async move {
        while let Some(event) = runtime_rx.recv().await {
            if matches!(event, RuntimeEvent::TurnComplete { .. }) {
                forward_complete_seen.store(true, Ordering::SeqCst);
            }
            forward_runtime_event(&event_tx, session_id, &forward_session_key, &event);
        }
        let _ = forward_done_tx.send(());
    });
    let turn = conductor
        .execute_prepared_streaming(
            intent,
            RuntimeSessionSpec {
                backend_id,
                participant_name: ctx.participant_name.clone(),
                session_key: intent_session_key,
                role: runtime_role_from_agent_role(ctx.agent_role),
                workspace_dir: ctx.workspace_dir.clone(),
                prompt_text: String::new(),
                tool_surface: ToolSurfaceSpec {
                    team_tools: ctx.mcp_server_url.is_some() || ctx.team_tool_url.is_some(),
                    local_skills: false,
                    external_mcp: false,
                    backend_native_tools: true,
                },
                tool_bridge_url: ctx.mcp_server_url.clone(),
                team_tool_url: ctx.team_tool_url.clone(),
                context: runtime_context_from_ctx(&ctx),
            },
            runtime_sink,
        )
        .await?;
    let _ = forward_done_rx.await;
    if !runtime_complete_seen.load(Ordering::SeqCst) && !turn.full_text.is_empty() {
        let complete = RuntimeEvent::TurnComplete {
            full_text: turn.full_text.clone(),
        };
        forward_runtime_event(
            &completion_event_tx,
            session_id,
            &ctx_session_key,
            &complete,
        );
    }
    Ok(turn)
}

fn runtime_role_from_agent_role(role: crate::traits::AgentRole) -> RuntimeRole {
    match role {
        crate::traits::AgentRole::Solo => RuntimeRole::Solo,
        crate::traits::AgentRole::Lead => RuntimeRole::Leader,
        crate::traits::AgentRole::Specialist => RuntimeRole::Specialist,
    }
}

fn runtime_context_from_ctx(ctx: &AgentCtx) -> RuntimeContext {
    RuntimeContext {
        system_prompt: (!ctx.system_injection.trim().is_empty())
            .then(|| ctx.system_injection.clone()),
        workspace_native_files: collect_workspace_native_files(ctx),
        memory_summary: ctx.shared_memory.clone(),
        agent_memory: ctx.agent_memory.clone(),
        team_manifest: ctx.team_manifest.clone(),
        task_reminder: ctx.task_reminder.clone(),
        history_lines: ctx
            .history
            .iter()
            .map(|msg| format!("[{}]: {}", msg.role, msg.content))
            .collect(),
        user_input: Some(ctx.user_text.clone()),
    }
}

fn collect_workspace_native_files(ctx: &AgentCtx) -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "AGENTS.md",
        "CLAUDE.md",
        "TEAM.md",
        "CONTEXT.md",
        "TASKS.md",
    ];

    let mut files = BTreeSet::new();
    for dir in [ctx.workspace_dir.as_ref(), ctx.team_dir.as_ref()]
        .into_iter()
        .flatten()
    {
        for name in CANDIDATES {
            if dir.join(name).is_file() {
                files.insert((*name).to_string());
            }
        }
    }
    files.into_iter().collect()
}

fn forward_runtime_event(
    event_tx: &broadcast::Sender<AgentEvent>,
    session_id: uuid::Uuid,
    session_key: &qai_protocol::SessionKey,
    event: &RuntimeEvent,
) {
    match event {
        RuntimeEvent::TextDelta { text } => {
            let _ = event_tx.send(AgentEvent::TextDelta {
                session_id,
                delta: text.clone(),
            });
        }
        RuntimeEvent::ApprovalRequest(request) => {
            let _ = event_tx.send(AgentEvent::ApprovalRequest {
                session_id,
                session_key: session_key.clone(),
                approval_id: request.id.clone(),
                prompt: request.prompt.clone(),
                command: request.command.clone(),
                cwd: request.cwd.clone(),
                host: request.host.clone(),
                agent_id: request.agent_id.clone(),
                expires_at_ms: request.expires_at_ms,
            });
        }
        RuntimeEvent::TurnComplete { full_text } => {
            let _ = event_tx.send(AgentEvent::TurnComplete {
                session_id,
                full_text: full_text.clone(),
                sender: None,
            });
        }
        RuntimeEvent::TurnFailed { error } => {
            let _ = event_tx.send(AgentEvent::Error {
                session_id,
                message: error.clone(),
            });
            tracing::warn!(session_id = %session_id, %error, "runtime turn failed");
        }
        RuntimeEvent::ToolCallback(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_runtime::{
        backend::{
            BackendFamily, CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
        },
        registry::BackendSpec,
        BackendAdapter, LaunchSpec, RuntimeEventSink,
    };
    use std::sync::Arc;

    struct FakeBackendAdapter;

    #[async_trait::async_trait(?Send)]
    impl BackendAdapter for FakeBackendAdapter {
        async fn probe(&self, _spec: &BackendSpec) -> Result<CapabilityProfile> {
            Ok(CapabilityProfile {
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
            })
        }

        async fn run_turn(
            &self,
            _spec: &BackendSpec,
            _session: RuntimeSessionSpec,
            sink: RuntimeEventSink,
        ) -> Result<TurnResult> {
            sink.emit(RuntimeEvent::ApprovalRequest(
                qai_runtime::PermissionRequest {
                    id: "approval-1".into(),
                    prompt: "Allow `git status`?".into(),
                    command: Some("git status".into()),
                    cwd: Some("/tmp".into()),
                    host: Some("gateway".into()),
                    agent_id: Some("main".into()),
                    expires_at_ms: Some(123),
                },
            ))?;
            sink.emit(RuntimeEvent::TextDelta {
                text: "hello ".into(),
            })?;
            sink.emit(RuntimeEvent::TurnComplete {
                full_text: "hello world".into(),
            })?;
            Ok(TurnResult {
                full_text: "hello world".into(),
                events: vec![],
            })
        }
    }

    #[tokio::test]
    async fn conductor_runtime_dispatch_executes_registered_adapter_and_forwards_events() {
        let runtime_registry = Arc::new(BackendRegistry::new());
        runtime_registry
            .register_adapter("acp", Arc::new(FakeBackendAdapter))
            .await;
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "codex".into(),
                family: BackendFamily::Acp,
                adapter_key: "acp".into(),
                launch: LaunchSpec::Command {
                    command: "codex-acp".into(),
                    args: vec![],
                    env: vec![],
                },
            })
            .await;
        let dispatcher = ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry));
        let (tx, mut rx) = broadcast::channel(8);

        let result = dispatcher
            .dispatch(RuntimeDispatchRequest {
                intent: TurnIntent {
                    session_key: qai_protocol::SessionKey::new("ws", "user"),
                    mode: qai_runtime::contract::TurnMode::Solo,
                    leader_candidate: None,
                    target_backend: Some("codex".into()),
                    user_text: "hello".into(),
                },
                ctx: AgentCtx::default(),
                fallback_backend_id: None,
                event_tx: tx,
            })
            .await
            .unwrap();

        assert_eq!(result.full_text, "hello world");
        let mut saw_thinking = false;
        let mut saw_approval = false;
        let mut saw_delta = false;
        let mut saw_complete = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(event)) => match event {
                    AgentEvent::Thinking { .. } => saw_thinking = true,
                    AgentEvent::ApprovalRequest {
                        approval_id,
                        command,
                        ..
                    } if approval_id == "approval-1"
                        && command.as_deref() == Some("git status") =>
                    {
                        saw_approval = true
                    }
                    AgentEvent::TextDelta { delta, .. } if delta == "hello " => saw_delta = true,
                    AgentEvent::TurnComplete { full_text, .. } if full_text == "hello world" => {
                        saw_complete = true
                    }
                    _ => {}
                },
                _ => {}
            }
            if saw_thinking && saw_approval && saw_delta && saw_complete {
                break;
            }
        }
        assert!(saw_thinking);
        assert!(saw_approval);
        assert!(saw_delta);
        assert!(saw_complete);
        let spec = runtime_registry.backend_spec("codex").await.unwrap();
        assert_eq!(spec.family, BackendFamily::Acp);
        assert_eq!(spec.adapter_key, "acp");
        assert!(matches!(spec.launch, LaunchSpec::Command { .. }));
    }
}
