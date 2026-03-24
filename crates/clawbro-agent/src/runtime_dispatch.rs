use crate::traits::AgentCtx;
use anyhow::Result;
use async_trait::async_trait;
use clawbro_protocol::{render_scope_storage_key, AgentEvent};
use clawbro_runtime::contract::RuntimeToolCall;
use clawbro_runtime::{
    acp::AcpBackendAdapter, fingerprint_backend_spec, ApprovalBroker, BackendRegistry,
    ClawBroNativeBackendAdapter, OpenClawBackendAdapter, RuntimeConductor, RuntimeContext,
    RuntimeEvent, RuntimeHistoryMessage, RuntimePruningPolicy, RuntimeRole, RuntimeSessionSpec,
    RuntimeTranscriptSemantics, ToolSurfaceSpec, TranscriptCompactionMode, TranscriptPruningMode,
    TurnIntent, TurnResult,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{timeout, Duration};

const RUNTIME_FORWARDER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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
    async fn backend_resume_fingerprint(&self, backend_id: &str) -> Result<Option<String>>;
}

/// Convenience constructor that pre-registers all three adapters and returns a dispatch handle.
///
/// **Constraint:** Must be called from a **synchronous** context — i.e., before entering or
/// outside of any Tokio async runtime. Internally uses `futures::executor::block_on` to drive
/// the async adapter registration. Calling this from inside an async Tokio task will panic.
///
/// In production (`main.rs`) adapters are registered directly with `.await` on the runtime
/// and `ConductorRuntimeDispatch::new` is called explicitly. This function exists as a
/// test-support convenience only.
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
            .register_adapter("native", Arc::new(ClawBroNativeBackendAdapter))
            .await;
    });
    Arc::new(ConductorRuntimeDispatch::new(registry))
}

pub struct ConductorRuntimeDispatch {
    registry: Arc<BackendRegistry>,
    worker_pool: RuntimeWorkerPool,
}

impl ConductorRuntimeDispatch {
    pub fn new(registry: Arc<BackendRegistry>) -> Self {
        Self {
            worker_pool: RuntimeWorkerPool::new(Arc::clone(&registry), default_worker_count()),
            registry,
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

    async fn backend_resume_fingerprint(&self, backend_id: &str) -> Result<Option<String>> {
        let Some(spec) = self.registry.backend_spec(backend_id).await else {
            return Ok(None);
        };
        Ok(Some(fingerprint_backend_spec(&spec)?))
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
        .name(format!("clawbro-runtime-worker-{worker_index}"))
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
    tracing::debug!(session_id = %session_id, "Starting runtime dispatch job");
    let backend_id = request
        .intent
        .target_backend
        .clone()
        .or_else(|| request.intent.leader_candidate.clone())
        .or(request.fallback_backend_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no backend selected for turn"))?;
    let spec = registry
        .backend_spec(&backend_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("backend `{backend_id}` is not registered"))?;
    tracing::debug!(
        session_id = %session_id,
        backend_id = %backend_id,
        family = ?spec.family,
        approval_mode = ?spec.approval_mode,
        external_mcp_servers = spec.external_mcp_servers.len(),
        provider_profile = spec.provider_profile.is_some(),
        "Resolved runtime backend spec"
    );

    let conductor = RuntimeConductor::new(Arc::clone(&registry));
    let intent = request.intent;
    let intent_session_key = intent.session_key.clone();
    let ctx = request.ctx;
    let ctx_session_key = ctx.session_key.clone();
    let thread_event_tx = request.event_tx.clone();
    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel();
    let runtime_sink = clawbro_runtime::RuntimeEventSink::new(runtime_tx);
    let runtime_complete_seen = Arc::new(AtomicBool::new(false));
    let forward_complete_seen = Arc::clone(&runtime_complete_seen);
    let event_tx = thread_event_tx.clone();
    let completion_event_tx = thread_event_tx.clone();
    let forward_session_key = ctx_session_key.clone();
    let (forward_done_tx, forward_done_rx) = oneshot::channel();
    tokio::task::spawn_local(async move {
        tracing::debug!(session_id = %session_id, "Runtime event forwarder started");
        while let Some(event) = runtime_rx.recv().await {
            tracing::debug!(
                session_id = %session_id,
                event_kind = runtime_event_kind(&event),
                "Runtime event received for forwarding"
            );
            if matches!(event, RuntimeEvent::TurnComplete { .. }) {
                forward_complete_seen.store(true, Ordering::SeqCst);
            }
            forward_runtime_event(&event_tx, session_id, &forward_session_key, &event);
        }
        tracing::debug!(session_id = %session_id, "Runtime event forwarder drained");
        let _ = forward_done_tx.send(());
    });
    let provider_profile = spec
        .provider_profile
        .as_ref()
        .map(|profile| profile.resolve_from_env())
        .transpose()?;
    tracing::debug!(
        session_id = %session_id,
        backend_id = %backend_id,
        "Calling runtime conductor execute_prepared_streaming"
    );
    let turn = conductor
        .execute_prepared_streaming(
            intent,
            RuntimeSessionSpec {
                backend_id: backend_id.clone(),
                participant_name: ctx.participant_name.clone(),
                session_key: intent_session_key,
                role: runtime_role_from_agent_role(ctx.agent_role),
                workspace_dir: ctx.workspace_dir.clone(),
                prompt_text: String::new(),
                tool_surface: ToolSurfaceSpec {
                    team_tools: ctx.mcp_server_url.is_some() || ctx.team_tool_url.is_some(),
                    local_skills: false,
                    external_mcp: !spec.external_mcp_servers.is_empty(),
                    backend_native_tools: true,
                },
                approval_mode: spec.approval_mode,
                tool_bridge_url: ctx.mcp_server_url.clone(),
                external_mcp_servers: spec.external_mcp_servers.clone(),
                provider_profile,
                team_tool_url: ctx.team_tool_url.clone(),
                context: runtime_context_from_ctx(&ctx, spec.family),
                backend_session_id: ctx.backend_session_id.clone(),
            },
            runtime_sink,
        )
        .await?;
    tracing::debug!(
        session_id = %session_id,
        backend_id = %backend_id,
        full_text_len = turn.full_text.len(),
        emitted_backend_session_id = turn
            .emitted_backend_session_id
            .as_deref()
            .unwrap_or("<none>"),
        "Runtime conductor execute_prepared_streaming completed"
    );
    if turn.full_text.is_empty() {
        tracing::warn!(
            session_id = %session_id,
            backend_id = %backend_id,
            "Backend returned zero-length output; possible cold-start or subprocess initialization failure"
        );
    }
    tracing::debug!(
        session_id = %session_id,
        "Waiting for runtime event forwarder to finish"
    );
    match timeout(RUNTIME_FORWARDER_DRAIN_TIMEOUT, forward_done_rx).await {
        Ok(_) => {
            tracing::debug!(
                session_id = %session_id,
                runtime_complete_seen = runtime_complete_seen.load(Ordering::SeqCst),
                "Runtime event forwarder finished"
            );
        }
        Err(_) => {
            let saw_turn_complete = runtime_complete_seen.load(Ordering::SeqCst);
            if saw_turn_complete {
                tracing::debug!(
                    session_id = %session_id,
                    timeout_ms = RUNTIME_FORWARDER_DRAIN_TIMEOUT.as_millis(),
                    runtime_complete_seen = saw_turn_complete,
                    "Runtime event forwarder drain timed out after TurnComplete; continuing turn finalization"
                );
            } else {
                tracing::warn!(
                    session_id = %session_id,
                    timeout_ms = RUNTIME_FORWARDER_DRAIN_TIMEOUT.as_millis(),
                    runtime_complete_seen = saw_turn_complete,
                    "Runtime event forwarder drain timed out before TurnComplete; continuing turn finalization"
                );
            }
        }
    }
    if !runtime_complete_seen.load(Ordering::SeqCst) && !turn.full_text.is_empty() {
        let complete = RuntimeEvent::TurnComplete {
            full_text: turn.full_text.clone(),
        };
        tracing::debug!(
            session_id = %session_id,
            "Synthesizing TurnComplete because runtime stream ended without one"
        );
        forward_runtime_event(
            &completion_event_tx,
            session_id,
            &ctx_session_key,
            &complete,
        );
    }
    // Stamp the resolved backend_id so registry can call complete_turn() with the correct key.
    Ok(TurnResult {
        backend_resume_fingerprint: Some(fingerprint_backend_spec(&spec)?),
        used_backend_id: Some(backend_id),
        ..turn
    })
}

fn runtime_role_from_agent_role(role: crate::traits::AgentRole) -> RuntimeRole {
    match role {
        crate::traits::AgentRole::Solo => RuntimeRole::Solo,
        crate::traits::AgentRole::Lead => RuntimeRole::Leader,
        crate::traits::AgentRole::Specialist => RuntimeRole::Specialist,
    }
}

fn runtime_context_from_ctx(
    ctx: &AgentCtx,
    family: clawbro_runtime::backend::BackendFamily,
) -> RuntimeContext {
    let history_messages: Vec<RuntimeHistoryMessage> = ctx
        .history
        .iter()
        .map(|msg| RuntimeHistoryMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
            sender: msg.sender.clone(),
            tool_calls: msg
                .tool_calls
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|call| RuntimeToolCall {
                    tool_call_id: call.tool_call_id,
                    name: call.name,
                    input_json: call.input.to_string(),
                    output: call.output,
                })
                .collect(),
        })
        .collect();
    let transcript_semantics = transcript_semantics_for_family(family);

    RuntimeContext {
        system_prompt: (!ctx.system_injection.trim().is_empty())
            .then(|| ctx.system_injection.clone()),
        workspace_native_files: collect_workspace_native_files(ctx),
        memory_summary: ctx.shared_memory.clone(),
        agent_memory: ctx.agent_memory.clone(),
        team_manifest: ctx.team_manifest.clone(),
        task_reminder: ctx.task_reminder.clone(),
        history_lines: clawbro_runtime::render_history_lines(
            &history_messages,
            &transcript_semantics,
        ),
        history_messages,
        transcript_semantics,
        user_input: Some(ctx.user_text.clone()),
    }
}

fn transcript_semantics_for_family(
    family: clawbro_runtime::backend::BackendFamily,
) -> RuntimeTranscriptSemantics {
    let pruning = match family {
        clawbro_runtime::backend::BackendFamily::ClawBroNative => TranscriptPruningMode::Off,
        clawbro_runtime::backend::BackendFamily::Acp
        | clawbro_runtime::backend::BackendFamily::OpenClawGateway => {
            TranscriptPruningMode::RequestLocal
        }
    };
    RuntimeTranscriptSemantics {
        pruning,
        pruning_policy: default_pruning_policy_for_family(family),
        compaction: TranscriptCompactionMode::RawTranscriptOnly,
    }
}

fn default_pruning_policy_for_family(
    family: clawbro_runtime::backend::BackendFamily,
) -> RuntimePruningPolicy {
    match family {
        clawbro_runtime::backend::BackendFamily::ClawBroNative => RuntimePruningPolicy::default(),
        clawbro_runtime::backend::BackendFamily::Acp
        | clawbro_runtime::backend::BackendFamily::OpenClawGateway => RuntimePruningPolicy {
            keep_last_assistants: 3,
            min_prunable_tool_chars: 4_000,
            soft_trim_head_chars: 800,
            soft_trim_tail_chars: 800,
        },
    }
}

fn collect_workspace_native_files(ctx: &AgentCtx) -> Vec<String> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(persona_dir) = ctx.persona_dir.as_ref() {
        for name in ["SOUL.md", "IDENTITY.md"] {
            push_visible_file(&mut files, &mut seen, persona_dir, name);
        }
        push_visible_file(&mut files, &mut seen, persona_dir, "USER.md");
        if !matches!(ctx.agent_role, crate::traits::AgentRole::Specialist) {
            push_visible_file(&mut files, &mut seen, persona_dir, "MEMORY.md");
        }
        let scoped_memory_name = format!("memory/{}.md", scoped_memory_file_stem(&ctx.session_key));
        push_visible_relative_file(&mut files, &mut seen, persona_dir, &scoped_memory_name);
    }

    if !ctx.frontstage_human_turn {
        if let Some(workspace_root) = ctx.workspace_root.as_ref() {
            for name in ["AGENTS.md", "CLAUDE.md", "USER.md", "HEARTBEAT.md"] {
                push_visible_file(&mut files, &mut seen, workspace_root, name);
            }
        }

        if let Some(team_dir) = ctx.team_dir.as_ref() {
            for name in ["TEAM.md", "CONTEXT.md", "TASKS.md", "HEARTBEAT.md"] {
                push_visible_file(&mut files, &mut seen, team_dir, name);
            }
        }
    }

    files
}

fn push_visible_file(
    files: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    dir: &std::path::Path,
    name: &str,
) {
    if dir.join(name).is_file() && seen.insert(name.to_string()) {
        files.push(name.to_string());
    }
}

fn push_visible_relative_file(
    files: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    dir: &std::path::Path,
    relative_name: &str,
) {
    if dir.join(relative_name).is_file() && seen.insert(relative_name.to_string()) {
        files.push(relative_name.to_string());
    }
}

fn scoped_memory_file_stem(session_key: &clawbro_protocol::SessionKey) -> String {
    render_scope_storage_key(session_key)
}

fn forward_runtime_event(
    event_tx: &broadcast::Sender<AgentEvent>,
    session_id: uuid::Uuid,
    session_key: &clawbro_protocol::SessionKey,
    event: &RuntimeEvent,
) {
    tracing::debug!(
        session_id = %session_id,
        event_kind = runtime_event_kind(event),
        "Forwarding runtime event to agent event bus"
    );
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
        RuntimeEvent::ToolCallStarted {
            tool_name, call_id, ..
        } => {
            let _ = event_tx.send(AgentEvent::ToolCallStart {
                session_id,
                tool_name: tool_name.clone(),
                call_id: call_id.clone(),
            });
        }
        RuntimeEvent::ToolCallCompleted {
            call_id, result, ..
        } => {
            let _ = event_tx.send(AgentEvent::ToolCallResult {
                session_id,
                call_id: call_id.clone(),
                result: result.clone(),
            });
        }
        RuntimeEvent::ToolCallFailed {
            tool_name,
            call_id,
            error,
        } => {
            let _ = event_tx.send(AgentEvent::ToolCallFailed {
                session_id,
                tool_name: tool_name.clone(),
                call_id: call_id.clone(),
                error: error.clone(),
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
        RuntimeEvent::ToolCallback(callback) => {
            tracing::debug!(
                session_id = %session_id,
                callback = ?callback,
                "ToolCallback event received but not yet forwarded to agent event bus"
            );
        }
    }
}

fn runtime_event_kind(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::TextDelta { .. } => "text_delta",
        RuntimeEvent::ToolCallStarted { .. } => "tool_call_started",
        RuntimeEvent::ToolCallCompleted { .. } => "tool_call_completed",
        RuntimeEvent::ToolCallFailed { .. } => "tool_call_failed",
        RuntimeEvent::ApprovalRequest(_) => "approval_request",
        RuntimeEvent::ToolCallback(_) => "tool_callback",
        RuntimeEvent::TurnComplete { .. } => "turn_complete",
        RuntimeEvent::TurnFailed { .. } => "turn_failed",
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use clawbro_runtime::{
        backend::{
            BackendFamily, CapabilityProfile, NativeTeamCapability, RoleEligibility, ToolBridgeKind,
        },
        registry::BackendSpec,
        BackendAdapter, LaunchSpec, RuntimeEventSink,
    };
    use std::sync::Arc;
    use tempfile::tempdir;

    struct FakeBackendAdapter;

    struct LeakyEventAdapter;

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
                clawbro_runtime::PermissionRequest {
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
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: None,
                resume_recovery: None,
            })
        }
    }

    #[async_trait::async_trait(?Send)]
    impl BackendAdapter for LeakyEventAdapter {
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
            let leaked = sink.clone();
            tokio::task::spawn_local(async move {
                let _hold = leaked;
                std::future::pending::<()>().await;
            });
            sink.emit(RuntimeEvent::TextDelta {
                text: "hello ".into(),
            })?;
            sink.emit(RuntimeEvent::TurnComplete {
                full_text: "hello world".into(),
            })?;
            Ok(TurnResult {
                full_text: "hello world".into(),
                events: vec![],
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: None,
                resume_recovery: None,
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
                launch: LaunchSpec::ExternalCommand {
                    command: "codex-acp".into(),
                    args: vec![],
                    env: vec![],
                },
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
                approval_mode: Default::default(),
            })
            .await;
        let dispatcher = ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry));
        let (tx, mut rx) = broadcast::channel(8);

        let result = dispatcher
            .dispatch(RuntimeDispatchRequest {
                intent: TurnIntent {
                    session_key: clawbro_protocol::SessionKey::new("ws", "user"),
                    mode: clawbro_runtime::contract::TurnMode::Solo,
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
            if let Ok(Ok(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
            {
                match event {
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
                }
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
        assert!(matches!(spec.launch, LaunchSpec::ExternalCommand { .. }));
    }

    #[tokio::test]
    async fn dispatch_does_not_block_forever_when_runtime_sender_lingers() {
        let runtime_registry = Arc::new(BackendRegistry::new());
        runtime_registry
            .register_adapter("acp", Arc::new(LeakyEventAdapter))
            .await;
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "claude".into(),
                family: BackendFamily::Acp,
                adapter_key: "acp".into(),
                launch: LaunchSpec::BundledCommand,
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
                approval_mode: Default::default(),
            })
            .await;
        let dispatcher = ConductorRuntimeDispatch::new(Arc::clone(&runtime_registry));
        let (tx, mut rx) = broadcast::channel(8);

        let result = tokio::time::timeout(
            RUNTIME_FORWARDER_DRAIN_TIMEOUT + Duration::from_secs(1),
            dispatcher.dispatch(RuntimeDispatchRequest {
                intent: TurnIntent {
                    session_key: clawbro_protocol::SessionKey::new("ws", "user"),
                    mode: clawbro_runtime::contract::TurnMode::Solo,
                    leader_candidate: None,
                    target_backend: Some("claude".into()),
                    user_text: "hello".into(),
                },
                ctx: AgentCtx::default(),
                fallback_backend_id: None,
                event_tx: tx,
            }),
        )
        .await
        .expect("dispatch should not hang when runtime sender lingers")
        .unwrap();

        assert_eq!(result.full_text, "hello world");

        let mut saw_complete = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(AgentEvent::TurnComplete { full_text, .. })) =
                tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
            {
                if full_text == "hello world" {
                    saw_complete = true;
                    break;
                }
            }
        }
        assert!(
            saw_complete,
            "expected TurnComplete despite lingering sender"
        );
    }

    #[tokio::test]
    async fn runtime_context_includes_external_mcp_servers() {
        let runtime_registry = Arc::new(BackendRegistry::new());
        runtime_registry
            .register_adapter("acp", Arc::new(FakeBackendAdapter))
            .await;
        runtime_registry
            .register_backend(BackendSpec {
                backend_id: "codex".into(),
                family: BackendFamily::Acp,
                adapter_key: "acp".into(),
                launch: LaunchSpec::BundledCommand,
                external_mcp_servers: vec![clawbro_runtime::ExternalMcpServerSpec {
                    name: "filesystem".into(),
                    transport: clawbro_runtime::ExternalMcpTransport::Sse {
                        url: "http://127.0.0.1:3001/sse".into(),
                    },
                }],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
                approval_mode: Default::default(),
            })
            .await;

        let spec = runtime_registry.backend_spec("codex").await.unwrap();
        assert_eq!(spec.external_mcp_servers.len(), 1);

        let session = RuntimeSessionSpec {
            backend_id: spec.backend_id.clone(),
            participant_name: None,
            session_key: clawbro_protocol::SessionKey::new("ws", "user"),
            role: RuntimeRole::Solo,
            workspace_dir: None,
            prompt_text: String::new(),
            tool_surface: ToolSurfaceSpec {
                team_tools: false,
                local_skills: false,
                external_mcp: !spec.external_mcp_servers.is_empty(),
                backend_native_tools: true,
            },
            provider_profile: None,
            tool_bridge_url: None,
            external_mcp_servers: spec.external_mcp_servers.clone(),
            approval_mode: Default::default(),
            team_tool_url: None,
            context: RuntimeContext::default(),
            backend_session_id: None,
        };

        assert!(session.tool_surface.external_mcp);
        assert_eq!(session.external_mcp_servers.len(), 1);
    }

    #[test]
    fn collect_workspace_native_files_includes_heartbeat_when_present() {
        let tmp = tempdir().unwrap();
        let persona = tempdir().unwrap();
        std::fs::write(persona.path().join("SOUL.md"), "soul").unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "agents").unwrap();
        std::fs::write(tmp.path().join("HEARTBEAT.md"), "heartbeat").unwrap();

        let mut ctx = AgentCtx::default();
        ctx.session_key = clawbro_protocol::SessionKey::new("lark", "group:test");
        ctx.persona_dir = Some(persona.path().to_path_buf());
        ctx.workspace_root = Some(tmp.path().to_path_buf());
        ctx.workspace_dir = Some(tmp.path().to_path_buf());

        let files = collect_workspace_native_files(&ctx);
        assert!(files.contains(&"SOUL.md".to_string()));
        assert!(files.contains(&"AGENTS.md".to_string()));
        assert!(files.contains(&"HEARTBEAT.md".to_string()));
    }

    #[test]
    fn collect_workspace_native_files_dedupes_workspace_and_team_entries() {
        let persona = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let team = tempdir().unwrap();
        std::fs::write(persona.path().join("SOUL.md"), "workspace soul").unwrap();
        std::fs::write(persona.path().join("MEMORY.md"), "long term").unwrap();
        std::fs::create_dir_all(persona.path().join("memory")).unwrap();
        std::fs::write(
            persona.path().join("memory").join("c=lark#s=group:test.md"),
            "scoped",
        )
        .unwrap();
        std::fs::write(workspace.path().join("AGENTS.md"), "agents").unwrap();
        std::fs::write(workspace.path().join("USER.md"), "user").unwrap();
        std::fs::write(workspace.path().join("HEARTBEAT.md"), "workspace heartbeat").unwrap();
        std::fs::write(team.path().join("HEARTBEAT.md"), "team heartbeat").unwrap();
        std::fs::write(team.path().join("TEAM.md"), "team").unwrap();
        std::fs::write(team.path().join("TASKS.md"), "tasks").unwrap();

        let mut ctx = AgentCtx::default();
        ctx.session_key = clawbro_protocol::SessionKey::new("lark", "group:test");
        ctx.persona_dir = Some(persona.path().to_path_buf());
        ctx.workspace_root = Some(workspace.path().to_path_buf());
        ctx.workspace_dir = Some(workspace.path().to_path_buf());
        ctx.team_dir = Some(team.path().to_path_buf());

        let files = collect_workspace_native_files(&ctx);
        assert_eq!(
            files,
            vec![
                "SOUL.md".to_string(),
                "MEMORY.md".to_string(),
                "memory/c=lark#s=group:test.md".to_string(),
                "AGENTS.md".to_string(),
                "USER.md".to_string(),
                "HEARTBEAT.md".to_string(),
                "TEAM.md".to_string(),
                "TASKS.md".to_string(),
            ]
        );
    }

    #[test]
    fn collect_workspace_native_files_hides_long_term_memory_for_specialists() {
        let persona = tempdir().unwrap();
        let team = tempdir().unwrap();
        std::fs::write(persona.path().join("SOUL.md"), "soul").unwrap();
        std::fs::write(persona.path().join("IDENTITY.md"), "identity").unwrap();
        std::fs::write(persona.path().join("MEMORY.md"), "long term").unwrap();
        std::fs::create_dir_all(persona.path().join("memory")).unwrap();
        std::fs::write(
            persona
                .path()
                .join("memory")
                .join("c=specialist#s=team-1:coder.md"),
            "specialist scoped",
        )
        .unwrap();
        std::fs::write(team.path().join("TEAM.md"), "team").unwrap();
        std::fs::write(team.path().join("CONTEXT.md"), "context").unwrap();

        let mut ctx = AgentCtx::default();
        ctx.session_key = clawbro_protocol::SessionKey::new("specialist", "team-1:coder");
        ctx.agent_role = crate::traits::AgentRole::Specialist;
        ctx.persona_dir = Some(persona.path().to_path_buf());
        ctx.team_dir = Some(team.path().to_path_buf());

        let files = collect_workspace_native_files(&ctx);
        assert!(files.contains(&"SOUL.md".to_string()));
        assert!(files.contains(&"IDENTITY.md".to_string()));
        assert!(files.contains(&"memory/c=specialist#s=team-1:coder.md".to_string()));
        assert!(!files.contains(&"MEMORY.md".to_string()));
        assert_eq!(
            files
                .iter()
                .filter(|name| name.as_str() == "HEARTBEAT.md")
                .count(),
            0
        );
        assert!(files.contains(&"TEAM.md".to_string()));
        assert!(files.contains(&"CONTEXT.md".to_string()));
    }

    #[test]
    fn collect_workspace_native_files_hides_workspace_and_team_files_for_frontstage_human_turns() {
        let persona = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let team = tempdir().unwrap();
        std::fs::write(persona.path().join("SOUL.md"), "soul").unwrap();
        std::fs::write(persona.path().join("IDENTITY.md"), "identity").unwrap();
        std::fs::write(workspace.path().join("AGENTS.md"), "agents").unwrap();
        std::fs::write(workspace.path().join("CLAUDE.md"), "claude").unwrap();
        std::fs::write(team.path().join("TEAM.md"), "team").unwrap();
        std::fs::write(team.path().join("TASKS.md"), "tasks").unwrap();

        let mut ctx = AgentCtx::default();
        ctx.session_key = clawbro_protocol::SessionKey::new("lark", "group:test");
        ctx.frontstage_human_turn = true;
        ctx.persona_dir = Some(persona.path().to_path_buf());
        ctx.workspace_root = Some(workspace.path().to_path_buf());
        ctx.workspace_dir = Some(workspace.path().to_path_buf());
        ctx.team_dir = Some(team.path().to_path_buf());

        let files = collect_workspace_native_files(&ctx);
        assert!(files.contains(&"SOUL.md".to_string()));
        assert!(files.contains(&"IDENTITY.md".to_string()));
        assert!(!files.contains(&"AGENTS.md".to_string()));
        assert!(!files.contains(&"CLAUDE.md".to_string()));
        assert!(!files.contains(&"TEAM.md".to_string()));
        assert!(!files.contains(&"TASKS.md".to_string()));
    }

    #[test]
    fn runtime_context_projection_preserves_structured_history() {
        let mut ctx = AgentCtx::default();
        ctx.user_text = "current turn".into();
        ctx.history = vec![
            crate::traits::HistoryMsg {
                role: "user".into(),
                content: "hello".into(),
                sender: Some("alice".into()),
                tool_calls: None,
            },
            crate::traits::HistoryMsg {
                role: "assistant".into(),
                content: "hi there".into(),
                sender: Some("@codex".into()),
                tool_calls: Some(vec![clawbro_session::ToolCallRecord {
                    tool_call_id: Some("call-1".into()),
                    name: "read".into(),
                    input: serde_json::json!({"path":"README.md"}),
                    output: Some("ok".into()),
                }]),
            },
        ];

        let projected = runtime_context_from_ctx(&ctx, clawbro_runtime::BackendFamily::Acp);
        assert_eq!(projected.history_messages.len(), 2);
        assert_eq!(projected.history_messages[0].role, "user");
        assert_eq!(projected.history_messages[0].content, "hello");
        assert_eq!(
            projected.history_messages[0].sender.as_deref(),
            Some("alice")
        );
        assert_eq!(projected.history_messages[1].role, "assistant");
        assert_eq!(projected.history_messages[1].content, "hi there");
        assert_eq!(
            projected.history_messages[1].sender.as_deref(),
            Some("@codex")
        );
        assert_eq!(projected.history_messages[1].tool_calls.len(), 1);
        assert_eq!(projected.history_messages[1].tool_calls[0].name, "read");
        assert_eq!(
            projected.history_messages[1].tool_calls[0]
                .tool_call_id
                .as_deref(),
            Some("call-1")
        );
        assert_eq!(
            projected.transcript_semantics.pruning,
            TranscriptPruningMode::RequestLocal
        );
        assert_eq!(
            projected.history_lines,
            vec![
                "[user]: [alice]: hello".to_string(),
                "[assistant]: [@codex]: hi there".to_string(),
                "[tool_call:read#call-1]: {\"path\":\"README.md\"}".to_string(),
                "[tool_result:read#call-1]: ok".to_string()
            ]
        );
    }

    #[test]
    fn runtime_context_projection_only_prunes_compatibility_lines_not_structured_history() {
        let long_output = "y".repeat(5000);
        let mut ctx = AgentCtx::default();
        ctx.history = vec![
            crate::traits::HistoryMsg {
                role: "assistant".into(),
                content: "older".into(),
                sender: None,
                tool_calls: Some(vec![clawbro_session::ToolCallRecord {
                    tool_call_id: Some("call-99".into()),
                    name: "read".into(),
                    input: serde_json::json!({"path":"big.txt"}),
                    output: Some(long_output.clone()),
                }]),
            },
            crate::traits::HistoryMsg {
                role: "assistant".into(),
                content: "recent-1".into(),
                sender: None,
                tool_calls: None,
            },
            crate::traits::HistoryMsg {
                role: "assistant".into(),
                content: "recent-2".into(),
                sender: None,
                tool_calls: None,
            },
            crate::traits::HistoryMsg {
                role: "assistant".into(),
                content: "recent-3".into(),
                sender: None,
                tool_calls: None,
            },
        ];

        let projected = runtime_context_from_ctx(&ctx, clawbro_runtime::BackendFamily::Acp);
        assert_eq!(
            projected.history_messages[0].tool_calls[0]
                .output
                .as_deref(),
            Some(long_output.as_str())
        );
        let rendered = projected
            .history_lines
            .iter()
            .find(|line| line.starts_with("[tool_result:read#call-99]: "))
            .unwrap();
        assert!(rendered.contains("[tool result pruned; omitted"));
    }

    #[test]
    fn native_family_defaults_to_no_request_local_pruning() {
        let long_output = "n".repeat(5000);
        let mut ctx = AgentCtx::default();
        ctx.history = vec![crate::traits::HistoryMsg {
            role: "assistant".into(),
            content: "older".into(),
            sender: None,
            tool_calls: Some(vec![clawbro_session::ToolCallRecord {
                tool_call_id: Some("native-1".into()),
                name: "read".into(),
                input: serde_json::json!({"path":"big.txt"}),
                output: Some(long_output.clone()),
            }]),
        }];

        let projected =
            runtime_context_from_ctx(&ctx, clawbro_runtime::BackendFamily::ClawBroNative);
        assert_eq!(
            projected.transcript_semantics.pruning,
            TranscriptPruningMode::Off
        );
        assert_eq!(
            projected
                .transcript_semantics
                .pruning_policy
                .keep_last_assistants,
            3
        );
        let rendered = projected
            .history_lines
            .iter()
            .find(|line| line.starts_with("[tool_result:read#native-1]: "))
            .unwrap();
        assert!(!rendered.contains("[tool result pruned; omitted"));
        assert!(rendered.ends_with(&long_output));
    }

    #[test]
    fn compatibility_families_default_to_request_local_pruning_policy() {
        let acp = transcript_semantics_for_family(clawbro_runtime::BackendFamily::Acp);
        let openclaw =
            transcript_semantics_for_family(clawbro_runtime::BackendFamily::OpenClawGateway);

        assert_eq!(acp.pruning, TranscriptPruningMode::RequestLocal);
        assert_eq!(openclaw.pruning, TranscriptPruningMode::RequestLocal);
        assert_eq!(acp.pruning_policy.keep_last_assistants, 3);
        assert_eq!(acp.pruning_policy.min_prunable_tool_chars, 4_000);
        assert_eq!(openclaw.pruning_policy.soft_trim_head_chars, 800);
        assert_eq!(openclaw.pruning_policy.soft_trim_tail_chars, 800);
    }
}
