use crate::agent_core::roster::AgentEntry;
use crate::cli::config_builders::{build_team_scope, parse_public_updates};
use crate::cli::config_draft::ConfigDraft;
use crate::cli::config_keys::{
    AgentKey, BackendKey, BindingKey, DeliverySenderBindingKey, DeliveryTargetOverrideKey,
    ProviderKey, TeamScopeKey,
};
use crate::cli::config_patch::ConfigPatch;
use crate::cli::config_store::{load_graph, persist_graph};
use crate::cli::config_validate::{validate_graph, ValidationSeverity};
use crate::config::{
    AcpBackendConfig, BackendApprovalConfig, BackendCatalogEntry, BackendFamilyConfig,
    BackendLaunchConfig, BindingConfig, BindingPeerKindConfig, DeliveryPurposeConfig,
    DeliverySenderBindingConfig, DeliveryTargetOverrideConfig, ProgressPresentationMode,
    ProviderProfileConfig, ProviderProfileProtocolConfig,
};
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use std::collections::BTreeMap;

pub async fn run() -> Result<()> {
    let theme = ColorfulTheme::default();
    let mut draft = ConfigDraft::new(load_graph()?);

    loop {
        println!();
        println!("{}", style("ClawBro Config Wizard").bold().cyan());
        println!("{}", render_summary(&draft));

        let items = [
            "Summary",
            "Channels",
            "Providers",
            "Backends",
            "Agents",
            "Routing",
            "Delivery",
            "Team Scopes",
            "Validate",
            "Preview Diff",
            "Apply",
            "Reset Draft",
            "Exit",
        ];
        let idx = Select::with_theme(&theme)
            .with_prompt(if draft.is_dirty() {
                "Select an area to configure (draft has changes)"
            } else {
                "Select an area to configure"
            })
            .items(&items)
            .default(0)
            .interact()?;

        match idx {
            0 => println!("{}", render_summary(&draft)),
            1 => channels_menu(&mut draft, &theme).await?,
            2 => providers_menu(&mut draft, &theme)?,
            3 => backends_menu(&mut draft, &theme)?,
            4 => agents_menu(&mut draft, &theme)?,
            5 => routing_menu(&mut draft, &theme)?,
            6 => delivery_menu(&mut draft, &theme)?,
            7 => team_scopes_menu(&mut draft, &theme)?,
            8 => validate_draft(&draft),
            9 => preview_diff(&draft),
            10 => {
                apply_draft(&draft)?;
                draft = ConfigDraft::new(load_graph()?);
            }
            11 => {
                if Confirm::with_theme(&theme)
                    .with_prompt("Discard draft changes and reset to saved config?")
                    .default(false)
                    .interact()?
                {
                    draft.reset();
                    println!("{}", style("Draft reset").yellow());
                }
            }
            _ => break,
        }
    }

    Ok(())
}

async fn channels_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let items = ["WeChat", "Lark", "DingTalk", "DingTalk Webhook", "Back"];
        let idx = Select::with_theme(theme)
            .with_prompt("Channels")
            .items(&items)
            .default(0)
            .interact()?;
        match idx {
            0 => wechat_menu(draft, theme).await?,
            1 => toggle_named_channel(draft, theme, "lark")?,
            2 => toggle_named_channel(draft, theme, "dingtalk")?,
            3 => toggle_named_channel(draft, theme, "dingtalk_webhook")?,
            _ => break,
        }
    }
    Ok(())
}

async fn wechat_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let enabled = draft
            .working()
            .channels
            .wechat
            .as_ref()
            .is_some_and(|cfg| cfg.enabled);
        let presentation = draft
            .working()
            .channels
            .wechat
            .as_ref()
            .map(|cfg| cfg.presentation)
            .unwrap_or(ProgressPresentationMode::FinalOnly);
        println!(
            "{} enabled={} presentation={:?}",
            style("WeChat").cyan(),
            enabled,
            presentation
        );

        let idx = Select::with_theme(theme)
            .with_prompt("WeChat channel")
            .items(&[
                "Toggle enabled",
                "Set presentation",
                "Run QR login now",
                "Back",
            ])
            .default(0)
            .interact()?;
        match idx {
            0 => draft.apply_patch(ConfigPatch::SetChannelEnabled {
                channel: "wechat".to_string(),
                enabled: !enabled,
            }),
            1 => {
                let pidx = Select::with_theme(theme)
                    .with_prompt("WeChat presentation")
                    .items(&["Final Only", "Progress Compact"])
                    .default(match presentation {
                        ProgressPresentationMode::FinalOnly => 0,
                        ProgressPresentationMode::ProgressCompact => 1,
                    })
                    .interact()?;
                draft.apply_patch(ConfigPatch::SetWeChatPresentation(match pidx {
                    1 => ProgressPresentationMode::ProgressCompact,
                    _ => ProgressPresentationMode::FinalOnly,
                }));
            }
            2 => {
                let path = crate::channels_internal::wechat_login().await?;
                println!(
                    "{} WeChat credentials saved to {}",
                    style("✓").green(),
                    path.display()
                );
            }
            _ => break,
        }
    }
    Ok(())
}

fn toggle_named_channel(
    draft: &mut ConfigDraft,
    theme: &ColorfulTheme,
    channel: &str,
) -> Result<()> {
    let enabled = match channel {
        "lark" => draft
            .working()
            .channels
            .lark
            .as_ref()
            .is_some_and(|cfg| cfg.enabled),
        "dingtalk" => draft
            .working()
            .channels
            .dingtalk
            .as_ref()
            .is_some_and(|cfg| cfg.enabled),
        "dingtalk_webhook" => draft
            .working()
            .channels
            .dingtalk_webhook
            .as_ref()
            .is_some_and(|cfg| cfg.enabled),
        _ => false,
    };
    if Confirm::with_theme(theme)
        .with_prompt(format!(
            "{} `{}`?",
            if enabled { "Disable" } else { "Enable" },
            channel
        ))
        .default(true)
        .interact()?
    {
        draft.apply_patch(ConfigPatch::SetChannelEnabled {
            channel: channel.to_string(),
            enabled: !enabled,
        });
    }
    Ok(())
}

fn providers_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Providers")
            .items(&[
                "List",
                "Add official_session",
                "Add anthropic_compatible",
                "Add openai_compatible",
                "Remove",
                "Back",
            ])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().providers.is_empty() {
                    println!("{}", style("No providers configured").yellow());
                } else {
                    for provider in draft.working().providers.values() {
                        println!("{} {}", style("•").cyan(), provider.id);
                    }
                }
            }
            1 => {
                let id = required_input(theme, "Provider id")?;
                draft.apply_patch(ConfigPatch::UpsertProvider(ProviderProfileConfig {
                    id,
                    protocol: ProviderProfileProtocolConfig::OfficialSession,
                }));
            }
            2 => {
                let id = required_input(theme, "Provider id")?;
                let base_url = required_input(theme, "Base URL")?;
                let auth_env = required_input(theme, "Auth env var")?;
                let default_model = required_input(theme, "Default model")?;
                let small_fast_model = optional_input(theme, "Small fast model (optional)")?;
                draft.apply_patch(ConfigPatch::UpsertProvider(ProviderProfileConfig {
                    id,
                    protocol: ProviderProfileProtocolConfig::AnthropicCompatible {
                        base_url,
                        auth_token_env: auth_env,
                        default_model,
                        small_fast_model,
                    },
                }));
            }
            3 => {
                let id = required_input(theme, "Provider id")?;
                let base_url = required_input(theme, "Base URL")?;
                let auth_env = required_input(theme, "Auth env var")?;
                let default_model = required_input(theme, "Default model")?;
                draft.apply_patch(ConfigPatch::UpsertProvider(ProviderProfileConfig {
                    id,
                    protocol: ProviderProfileProtocolConfig::OpenaiCompatible {
                        base_url,
                        auth_token_env: auth_env,
                        default_model,
                    },
                }));
            }
            4 => {
                let id = required_input(theme, "Provider id to remove")?;
                draft.apply_patch(ConfigPatch::RemoveProvider(ProviderKey::new(id)));
            }
            _ => break,
        }
    }
    Ok(())
}

fn backends_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Backends")
            .items(&["List", "Add", "Remove", "Back"])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().backends.is_empty() {
                    println!("{}", style("No backends configured").yellow());
                } else {
                    for backend in draft.working().backends.values() {
                        println!("{} {}", style("•").cyan(), backend.id);
                    }
                }
            }
            1 => {
                let id = required_input(theme, "Backend id")?;
                let family = match Select::with_theme(theme)
                    .with_prompt("Backend family")
                    .items(&["acp", "native", "openclaw"])
                    .default(0)
                    .interact()?
                {
                    1 => BackendFamilyConfig::ClawBroNative,
                    2 => BackendFamilyConfig::OpenClawGateway,
                    _ => BackendFamilyConfig::Acp,
                };
                let acp_backend = if matches!(family, BackendFamilyConfig::Acp) {
                    match Select::with_theme(theme)
                        .with_prompt("ACP backend")
                        .items(&["none", "claude", "codex", "custom"])
                        .default(0)
                        .interact()?
                    {
                        1 => Some(AcpBackendConfig::Claude),
                        2 => Some(AcpBackendConfig::Codex),
                        3 => Some(AcpBackendConfig::Custom),
                        _ => None,
                    }
                } else {
                    None
                };
                let provider_profile = optional_input(theme, "Provider profile id (optional)")?;
                let launch = if Select::with_theme(theme)
                    .with_prompt("Launch type")
                    .items(&["bundled", "external"])
                    .default(0)
                    .interact()?
                    == 1
                {
                    let command = required_input(theme, "Command")?;
                    let args = optional_input(theme, "Arguments (space-separated, optional)")?
                        .map(|value| value.split_whitespace().map(ToString::to_string).collect())
                        .unwrap_or_default();
                    let env = optional_input(theme, "Env KEY=VALUE,comma-separated (optional)")?
                        .map(parse_env_csv)
                        .transpose()?
                        .unwrap_or_default();
                    BackendLaunchConfig::ExternalCommand { command, args, env }
                } else {
                    BackendLaunchConfig::BundledCommand
                };
                draft.apply_patch(ConfigPatch::UpsertBackend(BackendCatalogEntry {
                    id,
                    family,
                    adapter_key: None,
                    acp_backend,
                    acp_auth_method: None,
                    codex: None,
                    provider_profile,
                    approval: BackendApprovalConfig::default(),
                    external_mcp_servers: vec![],
                    launch,
                }));
            }
            2 => {
                let id = required_input(theme, "Backend id to remove")?;
                draft.apply_patch(ConfigPatch::RemoveBackend(BackendKey::new(id)));
            }
            _ => break,
        }
    }
    Ok(())
}

fn agents_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Agents")
            .items(&["List", "Add", "Remove", "Back"])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().agents.is_empty() {
                    println!("{}", style("No agents configured").yellow());
                } else {
                    for agent in draft.working().agents.values() {
                        println!("{} {}", style("•").cyan(), agent.name);
                    }
                }
            }
            1 => {
                let name = required_input(theme, "Agent name")?;
                let backend_id = required_input(theme, "Backend id")?;
                let mentions_csv = optional_input(theme, "Mentions comma-separated (optional)")?;
                let mentions = mentions_csv
                    .map(|value| parse_csv(&value))
                    .filter(|values| !values.is_empty())
                    .unwrap_or_else(|| vec![format!("@{}", name)]);
                draft.apply_patch(ConfigPatch::UpsertAgent(AgentEntry {
                    name,
                    mentions,
                    backend_id,
                    persona_dir: None,
                    workspace_dir: None,
                    extra_skills_dirs: vec![],
                }));
            }
            2 => {
                let name = required_input(theme, "Agent name to remove")?;
                draft.apply_patch(ConfigPatch::RemoveAgent(AgentKey::new(name)));
            }
            _ => break,
        }
    }
    Ok(())
}

fn routing_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Routing")
            .items(&["List", "Add", "Remove", "Back"])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().bindings.is_empty() {
                    println!("{}", style("No bindings configured").yellow());
                } else {
                    for (id, binding) in &draft.working().bindings {
                        println!("{} {} -> {}", style("•").cyan(), id, binding.agent_name());
                    }
                }
            }
            1 => add_binding_menu(draft, theme)?,
            2 => {
                let id = required_input(theme, "Binding id to remove")?;
                draft.apply_patch(ConfigPatch::RemoveBinding(BindingKey::new(id)));
            }
            _ => break,
        }
    }
    Ok(())
}

fn add_binding_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    let idx = Select::with_theme(theme)
        .with_prompt("Binding kind")
        .items(&[
            "Channel",
            "Scope",
            "Peer",
            "Channel Instance",
            "Default",
            "Team",
            "Thread",
        ])
        .default(0)
        .interact()?;

    let binding = match idx {
        1 => BindingConfig::Scope {
            agent: required_input(theme, "Agent")?,
            scope: required_input(theme, "Scope")?,
            channel: optional_input(theme, "Channel (optional)")?,
        },
        2 => BindingConfig::Peer {
            agent: required_input(theme, "Agent")?,
            peer_kind: match Select::with_theme(theme)
                .with_prompt("Peer kind")
                .items(&["user", "group"])
                .default(0)
                .interact()?
            {
                1 => BindingPeerKindConfig::Group,
                _ => BindingPeerKindConfig::User,
            },
            peer_id: required_input(theme, "Peer id")?,
            channel: optional_input(theme, "Channel (optional)")?,
        },
        3 => BindingConfig::ChannelInstance {
            agent: required_input(theme, "Agent")?,
            channel: required_input(theme, "Channel")?,
            channel_instance: required_input(theme, "Channel instance")?,
        },
        4 => BindingConfig::Default {
            agent: required_input(theme, "Agent")?,
        },
        5 => BindingConfig::Team {
            agent: required_input(theme, "Agent")?,
            team_id: required_input(theme, "Team id")?,
        },
        6 => BindingConfig::Thread {
            agent: required_input(theme, "Agent")?,
            scope: required_input(theme, "Scope")?,
            thread_id: required_input(theme, "Thread id")?,
            channel: optional_input(theme, "Channel (optional)")?,
        },
        _ => BindingConfig::Channel {
            agent: required_input(theme, "Agent")?,
            channel: required_input(theme, "Channel")?,
        },
    };
    let key = BindingKey::from_binding(&binding);
    draft.apply_patch(ConfigPatch::UpsertBinding {
        key,
        value: binding,
    });
    Ok(())
}

fn delivery_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Delivery")
            .items(&[
                "List sender bindings",
                "Add sender binding",
                "Remove sender binding",
                "List target overrides",
                "Add target override",
                "Remove target override",
                "Back",
            ])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().delivery_sender_bindings.is_empty() {
                    println!(
                        "{}",
                        style("No delivery sender bindings configured").yellow()
                    );
                } else {
                    for (id, binding) in &draft.working().delivery_sender_bindings {
                        println!(
                            "{} {} -> {}",
                            style("•").cyan(),
                            id,
                            binding.channel_instance
                        );
                    }
                }
            }
            1 => {
                let purpose = select_delivery_purpose(theme)?;
                let channel_instance = required_input(theme, "Channel instance")?;
                let binding = DeliverySenderBindingConfig {
                    purpose,
                    agent: optional_input(theme, "Agent (optional)")?,
                    channel: optional_input(theme, "Channel (optional)")?,
                    channel_instance,
                };
                let key = DeliverySenderBindingKey::from_binding(&binding);
                draft.apply_patch(ConfigPatch::UpsertDeliverySenderBinding {
                    key,
                    value: binding,
                });
            }
            2 => {
                let id = required_input(theme, "Sender binding id to remove")?;
                draft.apply_patch(ConfigPatch::RemoveDeliverySenderBinding(
                    DeliverySenderBindingKey::new(id),
                ));
            }
            3 => {
                if draft.working().delivery_target_overrides.is_empty() {
                    println!(
                        "{}",
                        style("No delivery target overrides configured").yellow()
                    );
                } else {
                    for (id, override_cfg) in &draft.working().delivery_target_overrides {
                        println!("{} {} -> {}", style("•").cyan(), id, override_cfg.scope);
                    }
                }
            }
            4 => {
                let purpose = select_delivery_purpose(theme)?;
                let override_cfg = DeliveryTargetOverrideConfig {
                    purpose,
                    agent: optional_input(theme, "Agent (optional)")?,
                    channel: optional_input(theme, "Channel (optional)")?,
                    channel_instance: optional_input(theme, "Channel instance (optional)")?,
                    scope: required_input(theme, "Scope")?,
                    reply_to: optional_input(theme, "Reply-to (optional)")?,
                    thread_ts: optional_input(theme, "Thread ts (optional)")?,
                };
                let key = DeliveryTargetOverrideKey::from_binding(&override_cfg);
                draft.apply_patch(ConfigPatch::UpsertDeliveryTargetOverride {
                    key,
                    value: override_cfg,
                });
            }
            5 => {
                let id = required_input(theme, "Target override id to remove")?;
                draft.apply_patch(ConfigPatch::RemoveDeliveryTargetOverride(
                    DeliveryTargetOverrideKey::new(id),
                ));
            }
            _ => break,
        }
    }
    Ok(())
}

fn team_scopes_menu(draft: &mut ConfigDraft, theme: &ColorfulTheme) -> Result<()> {
    loop {
        let idx = Select::with_theme(theme)
            .with_prompt("Team Scopes")
            .items(&["List", "Add", "Remove", "Back"])
            .default(0)
            .interact()?;
        match idx {
            0 => {
                if draft.working().team_scopes.is_empty() {
                    println!("{}", style("No team scopes configured").yellow());
                } else {
                    for key in draft.working().team_scopes.keys() {
                        println!("{} {}", style("•").cyan(), key);
                    }
                }
            }
            1 => {
                let channel = match Select::with_theme(theme)
                    .with_prompt("Channel")
                    .items(&["wechat", "lark", "dingtalk", "dingtalk_webhook"])
                    .default(0)
                    .interact()?
                {
                    1 => "lark",
                    2 => "dingtalk",
                    3 => "dingtalk_webhook",
                    _ => "wechat",
                }
                .to_string();
                let default_scope = if channel == "wechat" {
                    "user:o9cqxxxx@im.wechat"
                } else {
                    "user:scope"
                };
                let scope: String = Input::with_theme(theme)
                    .with_prompt("Scope")
                    .default(default_scope.to_string())
                    .interact_text()?;
                let name = optional_input(theme, "Display name (optional)")?;
                let front_bot = required_input(theme, "Front bot")?;
                let specialists = parse_csv(&required_input(theme, "Specialists comma-separated")?);
                let max_parallel = Input::<usize>::with_theme(theme)
                    .with_prompt("Max parallel")
                    .default(1)
                    .interact_text()?;
                let public_updates = match Select::with_theme(theme)
                    .with_prompt("Public updates")
                    .items(&["minimal", "normal", "verbose"])
                    .default(0)
                    .interact()?
                {
                    1 => parse_public_updates("normal")?,
                    2 => parse_public_updates("verbose")?,
                    _ => parse_public_updates("minimal")?,
                };
                let (key, value) = build_team_scope(
                    channel,
                    scope,
                    name,
                    front_bot,
                    specialists,
                    max_parallel,
                    public_updates,
                )?;
                draft.apply_patch(ConfigPatch::UpsertTeamScope { key, value });
            }
            2 => {
                let channel = required_input(theme, "Channel")?;
                let scope = required_input(theme, "Scope")?;
                draft.apply_patch(ConfigPatch::RemoveTeamScope(TeamScopeKey::new(
                    channel, scope,
                )));
            }
            _ => break,
        }
    }
    Ok(())
}

fn validate_draft(draft: &ConfigDraft) {
    let report = validate_graph(draft.working());
    if report.issues.is_empty() {
        println!("{}", style("✓ Draft is valid").green());
        return;
    }
    for issue in &report.issues {
        match issue.severity {
            ValidationSeverity::Error => {
                println!("{} [{}] {}", style("✗").red(), issue.code, issue.message)
            }
            ValidationSeverity::Warning => {
                println!("{} [{}] {}", style("!").yellow(), issue.code, issue.message)
            }
        }
    }
}

fn preview_diff(draft: &ConfigDraft) {
    let diff = draft.diff();
    if diff.is_empty() {
        println!("{}", style("No draft changes").yellow());
        return;
    }
    println!("{}", style("Draft diff").bold().cyan());
    for line in diff.lines {
        println!("  {}", line);
    }
}

fn apply_draft(draft: &ConfigDraft) -> Result<()> {
    let report = validate_graph(draft.working());
    if report.has_errors() {
        validate_draft(draft);
        anyhow::bail!("draft has validation errors; fix them before apply");
    }
    let (path, backup) = persist_graph(draft.working())?;
    println!("{} Saved {}", style("✓").green(), path.display());
    if let Some(backup) = backup {
        println!("  backup: {}", backup.display());
    }
    Ok(())
}

fn render_summary(draft: &ConfigDraft) -> String {
    let graph = draft.working();
    format!(
        "draft={} providers={} backends={} agents={} bindings={} delivery_sender={} delivery_target={} team_scopes={} channels[wechat={}, lark={}, dingtalk={}, dingtalk_webhook={}]",
        if draft.is_dirty() { "dirty" } else { "clean" },
        graph.providers.len(),
        graph.backends.len(),
        graph.agents.len(),
        graph.bindings.len(),
        graph.delivery_sender_bindings.len(),
        graph.delivery_target_overrides.len(),
        graph.team_scopes.len(),
        graph.channels.wechat.as_ref().is_some_and(|cfg| cfg.enabled),
        graph.channels.lark.as_ref().is_some_and(|cfg| cfg.enabled),
        graph.channels.dingtalk.as_ref().is_some_and(|cfg| cfg.enabled),
        graph
            .channels
            .dingtalk_webhook
            .as_ref()
            .is_some_and(|cfg| cfg.enabled),
    )
}

fn required_input(theme: &ColorfulTheme, prompt: &str) -> Result<String> {
    loop {
        let value: String = Input::with_theme(theme)
            .with_prompt(prompt)
            .interact_text()?;
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        println!("{}", style("Value cannot be empty").red());
    }
}

fn optional_input(theme: &ColorfulTheme, prompt: &str) -> Result<Option<String>> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_env_csv(raw: String) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for pair in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid env pair `{pair}`, expected KEY=VALUE"))?;
        env.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok(env)
}

fn select_delivery_purpose(theme: &ColorfulTheme) -> Result<DeliveryPurposeConfig> {
    let idx = Select::with_theme(theme)
        .with_prompt("Delivery purpose")
        .items(&[
            "lead_final",
            "lead_message",
            "milestone",
            "approval",
            "bot_mention",
            "cron",
        ])
        .default(0)
        .interact()?;
    Ok(match idx {
        1 => DeliveryPurposeConfig::LeadMessage,
        2 => DeliveryPurposeConfig::Milestone,
        3 => DeliveryPurposeConfig::Approval,
        4 => DeliveryPurposeConfig::BotMention,
        5 => DeliveryPurposeConfig::Cron,
        _ => DeliveryPurposeConfig::LeadFinal,
    })
}
