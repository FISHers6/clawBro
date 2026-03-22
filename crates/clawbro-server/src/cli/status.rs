use crate::cli::config_store::load_graph;
use crate::cli::config_validate::validate_graph;
use crate::config::config_file_path;
use crate::config::BindingConfig;
use crate::scheduler_runtime::resolve_scheduler_db_path;
use anyhow::Result;
use console::style;

pub async fn run() -> Result<()> {
    let cfg_path = config_file_path();

    println!("{}", style("ClawBro — Status").bold().cyan());
    println!("{}", "─".repeat(40));

    if !cfg_path.exists() {
        println!(
            "{} config.toml not found — run: clawbro setup",
            style("⚠").yellow()
        );
        return Ok(());
    }

    let graph = load_graph()?;
    let cfg = graph.to_gateway_config();
    let report = validate_graph(&graph);

    let port = cfg.gateway.port;
    println!(
        "  Port         {}",
        if port == 0 {
            "random".into()
        } else {
            port.to_string()
        }
    );

    println!(
        "  Mode         {}",
        if graph.team_scopes.is_empty() {
            if graph.agents.is_empty() {
                "Solo".to_string()
            } else {
                format!("Multi-agent ({} agents)", graph.agents.len())
            }
        } else {
            format!(
                "Team ({} scopes, {} agents)",
                graph.team_scopes.len(),
                graph.agents.len()
            )
        }
    );

    println!(
        "  Backends     {}",
        if graph.backends.is_empty() {
            "(none configured)".into()
        } else {
            graph
                .backends
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "  Providers    {}",
        if graph.providers.is_empty() {
            "(none configured)".into()
        } else {
            graph
                .providers
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    let channels = [
        (
            "WeChat",
            graph
                .channels
                .wechat
                .as_ref()
                .is_some_and(|cfg| cfg.enabled),
        ),
        (
            "Lark",
            graph.channels.lark.as_ref().is_some_and(|cfg| cfg.enabled),
        ),
        (
            "DingTalk",
            graph
                .channels
                .dingtalk
                .as_ref()
                .is_some_and(|cfg| cfg.enabled),
        ),
        (
            "DingTalkWebhook",
            graph
                .channels
                .dingtalk_webhook
                .as_ref()
                .is_some_and(|cfg| cfg.enabled),
        ),
    ];
    let enabled_channels = channels
        .iter()
        .filter_map(|(name, enabled)| enabled.then_some(*name))
        .collect::<Vec<_>>();
    println!(
        "  Channels     {}",
        if enabled_channels.is_empty() {
            "WebSocket only".to_string()
        } else {
            enabled_channels.join(" + ")
        }
    );

    println!(
        "  WeChat       {}",
        match graph.channels.wechat.as_ref() {
            Some(section) => format!("enabled (presentation={:?})", section.presentation),
            None => "not configured".to_string(),
        }
    );

    println!(
        "  Routing      {}",
        if graph.bindings.is_empty() {
            "none".to_string()
        } else {
            format!(
                "{} bindings ({})",
                graph.bindings.len(),
                binding_summary(&graph)
            )
        }
    );

    println!(
        "  Teams        {}",
        if graph.team_scopes.is_empty() {
            "none".to_string()
        } else {
            team_scope_summary(&graph)
        }
    );

    println!(
        "  Delivery     sender={} target={}",
        graph.delivery_sender_bindings.len(),
        graph.delivery_target_overrides.len()
    );

    let has_api_env = cfg
        .provider_profiles
        .iter()
        .any(|profile| match &profile.protocol {
            crate::config::ProviderProfileProtocolConfig::OfficialSession => false,
            crate::config::ProviderProfileProtocolConfig::AnthropicCompatible {
                auth_token_env,
                ..
            }
            | crate::config::ProviderProfileProtocolConfig::OpenaiCompatible {
                auth_token_env,
                ..
            } => std::env::var(auth_token_env)
                .ok()
                .is_some_and(|value| !value.trim().is_empty()),
        });
    println!(
        "  API Env      {}",
        if has_api_env {
            style("set").green().to_string()
        } else {
            style("not set / source ~/.clawbro/.env")
                .yellow()
                .to_string()
        }
    );

    let port_file = dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join("gateway.port");
    println!(
        "  Gateway      {}",
        if port_file.exists() {
            style("running").green().to_string()
        } else {
            style("not running").dim().to_string()
        }
    );

    let scheduler_db = resolve_scheduler_db_path(&cfg);
    println!(
        "  Scheduler    {}",
        if cfg.scheduler.enabled {
            format!(
                "enabled (poll={}s, db={})",
                cfg.scheduler.poll_secs,
                scheduler_db.display()
            )
        } else {
            format!("disabled (db={})", scheduler_db.display())
        }
    );

    println!(
        "  Validation   {}",
        if report.has_errors() {
            style(format!(
                "{} error(s), {} warning(s)",
                report.error_count(),
                report.warning_count()
            ))
            .red()
            .to_string()
        } else {
            style(format!("ok ({} warning(s))", report.warning_count()))
                .green()
                .to_string()
        }
    );

    println!("\nConfig: {}", cfg_path.display());
    Ok(())
}

fn binding_summary(graph: &crate::cli::config_model::ConfigGraph) -> String {
    let mut samples = graph
        .bindings
        .values()
        .take(3)
        .map(|binding| match binding {
            BindingConfig::Channel { agent, channel } => format!("channel:{channel}->{agent}"),
            BindingConfig::Scope {
                agent,
                scope,
                channel,
            } => format!(
                "scope:{}:{}->{}",
                channel.as_deref().unwrap_or("*"),
                scope,
                agent
            ),
            BindingConfig::Default { agent } => format!("default->{agent}"),
            BindingConfig::ChannelInstance {
                agent,
                channel,
                channel_instance,
            } => format!("instance:{channel}/{channel_instance}->{agent}"),
            BindingConfig::Peer {
                agent,
                peer_id,
                channel,
                ..
            } => format!(
                "peer:{}:{}->{}",
                channel.as_deref().unwrap_or("*"),
                peer_id,
                agent
            ),
            BindingConfig::Team { agent, team_id } => format!("team:{team_id}->{agent}"),
            BindingConfig::Thread {
                agent,
                scope,
                thread_id,
                ..
            } => format!("thread:{scope}/{thread_id}->{agent}"),
        })
        .collect::<Vec<_>>();
    if graph.bindings.len() > samples.len() {
        samples.push(format!("+{}", graph.bindings.len() - samples.len()));
    }
    samples.join(", ")
}

fn team_scope_summary(graph: &crate::cli::config_model::ConfigGraph) -> String {
    let mut scopes = graph
        .team_scopes
        .values()
        .take(2)
        .map(|team_scope| {
            format!(
                "{}:{} (lead={}, specialists={})",
                team_scope.mode.channel.as_deref().unwrap_or("*"),
                team_scope.scope,
                team_scope.mode.front_bot.as_deref().unwrap_or("?"),
                team_scope.team.roster.len()
            )
        })
        .collect::<Vec<_>>();
    if graph.team_scopes.len() > scopes.len() {
        scopes.push(format!("+{}", graph.team_scopes.len() - scopes.len()));
    }
    scopes.join(", ")
}
