use crate::{
    adapter::BackendAdapter,
    backend::CapabilityProfile,
    contract::{render_runtime_prompt, RuntimeEvent, RuntimeSessionSpec, TurnResult},
    event_sink::RuntimeEventSink,
    registry::BackendSpec,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct CapturedTurn {
    pub backend_id: String,
    pub session: RuntimeSessionSpec,
    pub rendered_prompt: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptedTurn {
    pub full_text: String,
    pub events: Vec<RuntimeEvent>,
}

type ScriptHandler =
    dyn Fn(&BackendSpec, &RuntimeSessionSpec) -> anyhow::Result<ScriptedTurn> + Send + Sync;

pub struct ScriptedAdapter {
    profile: CapabilityProfile,
    handler: Arc<ScriptHandler>,
    captures: Arc<Mutex<Vec<CapturedTurn>>>,
}

impl ScriptedAdapter {
    pub fn new(
        profile: CapabilityProfile,
        handler: impl Fn(&BackendSpec, &RuntimeSessionSpec) -> anyhow::Result<ScriptedTurn>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            profile,
            handler: Arc::new(handler),
            captures: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn captures(&self) -> Vec<CapturedTurn> {
        self.captures.lock().unwrap().clone()
    }
}

#[async_trait::async_trait(?Send)]
impl BackendAdapter for ScriptedAdapter {
    async fn probe(&self, _spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
        Ok(self.profile.clone())
    }

    async fn run_turn(
        &self,
        spec: &BackendSpec,
        session: RuntimeSessionSpec,
        sink: RuntimeEventSink,
    ) -> anyhow::Result<TurnResult> {
        self.captures.lock().unwrap().push(CapturedTurn {
            backend_id: spec.backend_id.clone(),
            rendered_prompt: render_runtime_prompt(&session),
            session: session.clone(),
        });

        let scripted = (self.handler)(spec, &session)?;
        for event in &scripted.events {
            sink.emit(event.clone())?;
        }

        Ok(TurnResult {
            full_text: scripted.full_text,
            events: scripted.events,
            emitted_backend_session_id: None,
            backend_resume_fingerprint: None,
            used_backend_id: None,
        })
    }
}
