use crate::runtime::{adapter::BackendAdapter, backend::CapabilityProfile};
use dashmap::DashMap;
use std::time::{Duration, Instant};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSpec {
    pub backend_id: String,
    pub family: crate::runtime::backend::BackendFamily,
    pub adapter_key: String,
    pub launch: crate::runtime::adapter::LaunchSpec,
    pub approval_mode: crate::runtime::backend::ApprovalMode,
    pub external_mcp_servers: Vec<crate::runtime::contract::ExternalMcpServerSpec>,
    pub provider_profile: Option<crate::runtime::provider_profiles::ConfiguredProviderProfile>,
    /// Optional ACP backend identity. Only populated when `family == Acp`.
    /// `None` means generic ACP CLI backend.
    pub acp_backend: Option<crate::runtime::acp::AcpBackend>,
    /// Optional ACP auth method to negotiate after initialize().
    /// Currently only bridge-backed backends such as Codex consume this.
    pub acp_auth_method: Option<crate::runtime::acp::AcpAuthMethod>,
    /// Optional Codex-specific provider projection mode within the ACP family.
    pub codex_projection: Option<crate::runtime::acp::CodexProjectionMode>,
}

pub struct BackendRegistry {
    backends: RwLock<HashMap<String, BackendSpec>>,
    adapters: RwLock<HashMap<String, Arc<dyn BackendAdapter>>>,
    capability_cache: DashMap<String, CapabilityCacheEntry>,
    probe_ttl: Duration,
}

#[derive(Debug, Clone)]
struct CapabilityCacheEntry {
    profile: CapabilityProfile,
    observed_at: Instant,
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self {
            backends: RwLock::new(HashMap::new()),
            adapters: RwLock::new(HashMap::new()),
            capability_cache: DashMap::new(),
            probe_ttl: Duration::from_secs(30),
        }
    }
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_probe_ttl(probe_ttl: Duration) -> Self {
        Self {
            probe_ttl,
            ..Self::default()
        }
    }

    pub async fn register_adapter(
        &self,
        adapter_key: impl Into<String>,
        adapter: Arc<dyn BackendAdapter>,
    ) {
        self.adapters
            .write()
            .await
            .insert(adapter_key.into(), adapter);
    }

    pub async fn register_backend(&self, spec: BackendSpec) {
        self.capability_cache.remove(spec.backend_id.as_str());
        self.backends
            .write()
            .await
            .insert(spec.backend_id.clone(), spec);
    }

    pub fn invalidate_backend(&self, backend_id: &str) {
        self.capability_cache.remove(backend_id);
    }

    pub async fn backend_spec(&self, backend_id: &str) -> Option<BackendSpec> {
        self.backends.read().await.get(backend_id).cloned()
    }

    pub async fn all_backend_specs(&self) -> Vec<BackendSpec> {
        let mut specs: Vec<_> = self.backends.read().await.values().cloned().collect();
        specs.sort_by(|a, b| a.backend_id.cmp(&b.backend_id));
        specs
    }

    pub async fn has_adapter(&self, adapter_key: &str) -> bool {
        self.adapters.read().await.contains_key(adapter_key)
    }

    pub fn cached_capability_profile(&self, backend_id: &str) -> Option<CapabilityProfile> {
        self.capability_cache
            .get(backend_id)
            .map(|entry| entry.profile.clone())
    }

    pub async fn adapter_for(&self, spec: &BackendSpec) -> anyhow::Result<Arc<dyn BackendAdapter>> {
        self.adapters
            .read()
            .await
            .get(&spec.adapter_key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no adapter registered for '{}'", spec.adapter_key))
    }

    pub async fn probe_backend(&self, backend_id: &str) -> anyhow::Result<CapabilityProfile> {
        if let Some(cached) = self.capability_cache.get(backend_id) {
            if cached.observed_at.elapsed() < self.probe_ttl {
                return Ok(cached.profile.clone());
            }
        }

        let spec = self
            .backend_spec(backend_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("backend '{}' not registered", backend_id))?;
        let adapter = self.adapter_for(&spec).await?;
        let profile = adapter.probe(&spec).await?;
        self.capability_cache.insert(
            backend_id.to_string(),
            CapabilityCacheEntry {
                profile: profile.clone(),
                observed_at: Instant::now(),
            },
        );
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        adapter::LaunchSpec,
        backend::{BackendFamily, CapabilityProfile},
        contract::{RuntimeSessionSpec, TurnResult},
        event_sink::RuntimeEventSink,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FakeAdapter {
        probe_count: Arc<AtomicUsize>,
        profile: CapabilityProfile,
    }

    #[async_trait::async_trait(?Send)]
    impl BackendAdapter for FakeAdapter {
        async fn probe(&self, _spec: &BackendSpec) -> anyhow::Result<CapabilityProfile> {
            self.probe_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.profile.clone())
        }

        async fn run_turn(
            &self,
            _spec: &BackendSpec,
            _session: RuntimeSessionSpec,
            _sink: RuntimeEventSink,
        ) -> anyhow::Result<TurnResult> {
            Ok(TurnResult {
                full_text: String::new(),
                events: Vec::new(),
                emitted_backend_session_id: None,
                backend_resume_fingerprint: None,
                used_backend_id: None,
                resume_recovery: None,
            })
        }
    }

    #[tokio::test]
    async fn registry_can_probe_backend_with_fake_adapter() {
        let registry = BackendRegistry::new();
        let probe_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(FakeAdapter {
            probe_count: Arc::clone(&probe_count),
            profile: CapabilityProfile::solo_only(),
        });
        registry.register_adapter("fake", adapter).await;
        registry
            .register_backend(BackendSpec {
                backend_id: "codex".into(),
                family: BackendFamily::Acp,
                adapter_key: "fake".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let first = registry.probe_backend("codex").await.unwrap();
        let second = registry.probe_backend("codex").await.unwrap();

        assert_eq!(first, CapabilityProfile::solo_only());
        assert_eq!(second, CapabilityProfile::solo_only());
        assert_eq!(probe_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn registry_reprobes_when_cache_is_invalidated() {
        let registry = BackendRegistry::new();
        let probe_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(FakeAdapter {
            probe_count: Arc::clone(&probe_count),
            profile: CapabilityProfile::solo_only(),
        });
        registry.register_adapter("fake", adapter).await;
        registry
            .register_backend(BackendSpec {
                backend_id: "openclaw-main".into(),
                family: BackendFamily::OpenClawGateway,
                adapter_key: "fake".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let _ = registry.probe_backend("openclaw-main").await.unwrap();
        registry.invalidate_backend("openclaw-main");
        let _ = registry.probe_backend("openclaw-main").await.unwrap();

        assert_eq!(probe_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn registry_reprobes_when_probe_ttl_expires() {
        let registry = BackendRegistry::with_probe_ttl(Duration::ZERO);
        let probe_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(FakeAdapter {
            probe_count: Arc::clone(&probe_count),
            profile: CapabilityProfile::solo_only(),
        });
        registry.register_adapter("fake", adapter).await;
        registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::ClawBroNative,
                adapter_key: "fake".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let _ = registry.probe_backend("native-main").await.unwrap();
        let _ = registry.probe_backend("native-main").await.unwrap();

        assert_eq!(probe_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn registry_returns_backend_specs_by_id() {
        let registry = BackendRegistry::new();
        registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::ClawBroNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::ExternalCommand {
                    command: "clawbro-rust-agent".into(),
                    args: vec!["--stdio".into()],
                    env: vec![("RUST_LOG".into(), "debug".into())],
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let spec = registry.backend_spec("native-main").await.unwrap();
        assert_eq!(spec.backend_id, "native-main");
        assert_eq!(spec.adapter_key, "native");
        match spec.launch {
            LaunchSpec::ExternalCommand { command, args, env } => {
                assert_eq!(command, "clawbro-rust-agent");
                assert_eq!(args, vec!["--stdio"]);
                assert_eq!(env, vec![("RUST_LOG".to_string(), "debug".to_string())]);
            }
            other => panic!("unexpected launch spec: {other:?}"),
        }
    }

    #[tokio::test]
    async fn acp_backend_spec_preserves_explicit_identity() {
        use crate::runtime::acp::AcpBackend;
        let registry = BackendRegistry::new();
        registry
            .register_backend(BackendSpec {
                backend_id: "claude-main".into(),
                family: BackendFamily::Acp,
                adapter_key: "acp".into(),
                launch: LaunchSpec::ExternalCommand {
                    command: "npx".into(),
                    args: vec!["@zed-industries/claude-agent-acp".into()],
                    env: vec![],
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: Some(AcpBackend::Claude),
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let spec = registry.backend_spec("claude-main").await.unwrap();
        assert_eq!(spec.acp_backend, Some(AcpBackend::Claude));
    }

    #[tokio::test]
    async fn acp_backend_spec_preserves_none_when_omitted() {
        let registry = BackendRegistry::new();
        registry
            .register_backend(BackendSpec {
                backend_id: "generic-acp".into(),
                family: BackendFamily::Acp,
                adapter_key: "acp".into(),
                launch: LaunchSpec::ExternalCommand {
                    command: "some-acp-tool".into(),
                    args: vec!["--acp".into()],
                    env: vec![],
                },
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let spec = registry.backend_spec("generic-acp").await.unwrap();
        assert_eq!(spec.acp_backend, None);
    }

    #[tokio::test]
    async fn non_acp_backend_spec_always_has_none() {
        let registry = BackendRegistry::new();
        registry
            .register_backend(BackendSpec {
                backend_id: "native-main".into(),
                family: BackendFamily::ClawBroNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::BundledCommand,
                approval_mode: Default::default(),
                external_mcp_servers: vec![],
                provider_profile: None,
                acp_backend: None,
                acp_auth_method: None,
                codex_projection: None,
            })
            .await;

        let spec = registry.backend_spec("native-main").await.unwrap();
        assert_eq!(spec.acp_backend, None);
    }
}
