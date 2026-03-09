use crate::{adapter::BackendAdapter, backend::CapabilityProfile};
use dashmap::DashMap;
use std::time::{Duration, Instant};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSpec {
    pub backend_id: String,
    pub family: crate::backend::BackendFamily,
    pub adapter_key: String,
    pub launch: crate::adapter::LaunchSpec,
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
    use crate::{
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
                launch: LaunchSpec::Embedded,
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
                launch: LaunchSpec::Embedded,
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
                family: BackendFamily::QuickAiNative,
                adapter_key: "fake".into(),
                launch: LaunchSpec::Embedded,
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
                family: BackendFamily::QuickAiNative,
                adapter_key: "native".into(),
                launch: LaunchSpec::Command {
                    command: "quickai-rust-agent".into(),
                    args: vec!["--stdio".into()],
                    env: vec![("RUST_LOG".into(), "debug".into())],
                },
            })
            .await;

        let spec = registry.backend_spec("native-main").await.unwrap();
        assert_eq!(spec.backend_id, "native-main");
        assert_eq!(spec.adapter_key, "native");
        match spec.launch {
            LaunchSpec::Command { command, args, env } => {
                assert_eq!(command, "quickai-rust-agent");
                assert_eq!(args, vec!["--stdio"]);
                assert_eq!(env, vec![("RUST_LOG".to_string(), "debug".to_string())]);
            }
            other => panic!("unexpected launch spec: {other:?}"),
        }
    }
}
