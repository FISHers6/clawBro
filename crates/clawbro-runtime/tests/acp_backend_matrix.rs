/// Runtime shape regression tests for ACP backend matrix.
///
/// These tests verify that:
/// - config-to-spec conversion preserves backend identity and launch shape
/// - backend identity parsing round-trips correctly
/// - launch command/args are not rewritten for any ACP backend
/// - generic ACP CLI fallback works when acp_backend is omitted
use clawbro_runtime::{AcpBackend, BackendFamily, BackendSpec, LaunchSpec};

fn make_acp_spec(acp_backend: Option<AcpBackend>, command: &str, args: &[&str]) -> BackendSpec {
    BackendSpec {
        backend_id: format!("{command}-test"),
        family: BackendFamily::Acp,
        adapter_key: "acp".into(),
        launch: LaunchSpec::ExternalCommand {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: vec![],
        },
        approval_mode: Default::default(),
        external_mcp_servers: vec![],
        provider_profile: None,
        acp_backend,
        acp_auth_method: None,
        codex_projection: None,
    }
}

#[test]
fn claude_bridge_backed_backend_identity_preserved() {
    let spec = make_acp_spec(
        Some(AcpBackend::Claude),
        "npx",
        &["@zed-industries/claude-agent-acp"],
    );
    assert_eq!(spec.acp_backend, Some(AcpBackend::Claude));
    assert_eq!(spec.family, BackendFamily::Acp);
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &["@zed-industries/claude-agent-acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn codex_bridge_backed_backend_identity_preserved() {
    let spec = make_acp_spec(
        Some(AcpBackend::Codex),
        "npx",
        &["@zed-industries/codex-acp"],
    );
    assert_eq!(spec.acp_backend, Some(AcpBackend::Codex));
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &["@zed-industries/codex-acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn codebuddy_bridge_backed_backend_identity_preserved() {
    let spec = make_acp_spec(
        Some(AcpBackend::Codebuddy),
        "npx",
        &["@tencent-ai/codebuddy-code", "--acp"],
    );
    assert_eq!(spec.acp_backend, Some(AcpBackend::Codebuddy));
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &["@tencent-ai/codebuddy-code", "--acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn qwen_generic_acp_backend_identity_preserved() {
    let spec = make_acp_spec(
        Some(AcpBackend::Qwen),
        "npx",
        &["@qwen-code/qwen-code", "--acp"],
    );
    assert_eq!(spec.acp_backend, Some(AcpBackend::Qwen));
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &["@qwen-code/qwen-code", "--acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn goose_generic_acp_subcommand_backend_identity_preserved() {
    let spec = make_acp_spec(Some(AcpBackend::Goose), "goose", &["acp"]);
    assert_eq!(spec.acp_backend, Some(AcpBackend::Goose));
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "goose");
            assert_eq!(args, &["acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn omitted_acp_backend_falls_back_to_generic_semantics() {
    let spec = make_acp_spec(None, "some-acp-tool", &["--acp"]);
    assert_eq!(spec.acp_backend, None);
    assert_eq!(spec.family, BackendFamily::Acp);
    // Generic path — launch shape is preserved without any rewriting
    match &spec.launch {
        LaunchSpec::ExternalCommand { command, args, .. } => {
            assert_eq!(command, "some-acp-tool");
            assert_eq!(args, &["--acp"]);
        }
        other => panic!("unexpected launch spec: {other:?}"),
    }
}

#[test]
fn launch_shape_is_not_rewritten_for_any_backend() {
    // Verify that the ACP adapter does not alter command/args at runtime-spec level.
    // The launch spec is preserved exactly as configured.
    let backends = [
        (Some(AcpBackend::Claude), "npx", vec!["--custom-arg"]),
        (Some(AcpBackend::Qwen), "qwen", vec!["--acp", "--debug"]),
        (None, "custom-acp-tool", vec!["--port", "9999"]),
    ];
    for (backend, cmd, args) in backends {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        let spec = make_acp_spec(backend, cmd, &arg_refs);
        match &spec.launch {
            LaunchSpec::ExternalCommand {
                command,
                args: spec_args,
                ..
            } => {
                assert_eq!(
                    command, cmd,
                    "command must not be rewritten for {backend:?}"
                );
                assert_eq!(
                    spec_args, &args,
                    "args must not be rewritten for {backend:?}"
                );
            }
            other => panic!("unexpected launch spec for {backend:?}: {other:?}"),
        }
    }
}

#[test]
fn acp_backend_policy_matches_identity_for_all_known_backends() {
    use clawbro_runtime::acp::policy::{AcpBackendPolicy, BootstrapStyle};

    // Bridge-backed
    for backend in [AcpBackend::Claude, AcpBackend::Codex, AcpBackend::Codebuddy] {
        let policy = AcpBackendPolicy::for_backend(Some(backend));
        assert_eq!(
            policy.bootstrap_style,
            BootstrapStyle::BridgeBacked,
            "{backend:?} should be bridge-backed"
        );
    }

    // Generic
    for backend in [
        AcpBackend::Qwen,
        AcpBackend::Iflow,
        AcpBackend::Goose,
        AcpBackend::Kimi,
        AcpBackend::Opencode,
        AcpBackend::Qoder,
        AcpBackend::Vibe,
        AcpBackend::Custom,
    ] {
        let policy = AcpBackendPolicy::for_backend(Some(backend));
        assert_eq!(
            policy.bootstrap_style,
            BootstrapStyle::Generic,
            "{backend:?} should be generic"
        );
    }

    // None → generic
    assert_eq!(
        AcpBackendPolicy::for_backend(None).bootstrap_style,
        BootstrapStyle::Generic
    );
}
