use crate::agent_core::roster::AgentEntry;
use crate::cli::config_keys::{
    AgentKey, BackendKey, BindingKey, DeliverySenderBindingKey, DeliveryTargetOverrideKey,
    ProviderKey, TeamScopeKey,
};
use crate::cli::config_model::ConfigGraph;
use crate::config::{
    BackendCatalogEntry, BindingConfig, DeliverySenderBindingConfig, DeliveryTargetOverrideConfig,
    ProgressPresentationMode, ProviderProfileConfig, TeamScopeConfig,
};

#[derive(Debug, Clone)]
pub enum ConfigPatch {
    UpsertProvider(ProviderProfileConfig),
    RemoveProvider(ProviderKey),
    UpsertBackend(BackendCatalogEntry),
    RemoveBackend(BackendKey),
    UpsertAgent(AgentEntry),
    RemoveAgent(AgentKey),
    SetChannelEnabled {
        channel: String,
        enabled: bool,
    },
    SetWeChatPresentation(ProgressPresentationMode),
    UpsertBinding {
        key: BindingKey,
        value: BindingConfig,
    },
    RemoveBinding(BindingKey),
    UpsertDeliverySenderBinding {
        key: DeliverySenderBindingKey,
        value: DeliverySenderBindingConfig,
    },
    RemoveDeliverySenderBinding(DeliverySenderBindingKey),
    UpsertDeliveryTargetOverride {
        key: DeliveryTargetOverrideKey,
        value: DeliveryTargetOverrideConfig,
    },
    RemoveDeliveryTargetOverride(DeliveryTargetOverrideKey),
    UpsertTeamScope {
        key: TeamScopeKey,
        value: TeamScopeConfig,
    },
    RemoveTeamScope(TeamScopeKey),
}

impl ConfigPatch {
    pub fn apply(&self, graph: &mut ConfigGraph) {
        match self {
            Self::UpsertProvider(provider) => {
                graph
                    .providers
                    .insert(provider.id.clone(), provider.clone());
            }
            Self::RemoveProvider(key) => {
                graph.providers.remove(key.as_str());
            }
            Self::UpsertBackend(backend) => {
                graph.backends.insert(backend.id.clone(), backend.clone());
            }
            Self::RemoveBackend(key) => {
                graph.backends.remove(key.as_str());
            }
            Self::UpsertAgent(agent) => {
                graph.agents.insert(agent.name.clone(), agent.clone());
            }
            Self::RemoveAgent(key) => {
                graph.agents.remove(key.as_str());
            }
            Self::SetChannelEnabled { channel, enabled } => match channel.as_str() {
                "wechat" => {
                    if let Some(wechat) = graph.channels.wechat.as_mut() {
                        wechat.enabled = *enabled;
                    } else if *enabled {
                        graph.channels.wechat = Some(crate::config::WeChatSection {
                            enabled: true,
                            presentation: crate::config::ProgressPresentationMode::FinalOnly,
                        });
                    }
                }
                "lark" => {
                    if let Some(lark) = graph.channels.lark.as_mut() {
                        lark.enabled = *enabled;
                    } else if *enabled {
                        graph.channels.lark = Some(crate::config::LarkSection {
                            enabled: true,
                            presentation: crate::config::ProgressPresentationMode::FinalOnly,
                            trigger_policy: None,
                            default_instance: None,
                            instances: Vec::new(),
                        });
                    }
                }
                "dingtalk" => {
                    if let Some(dingtalk) = graph.channels.dingtalk.as_mut() {
                        dingtalk.enabled = *enabled;
                    } else if *enabled {
                        graph.channels.dingtalk = Some(crate::config::DingTalkSection {
                            enabled: true,
                            presentation: crate::config::ProgressPresentationMode::FinalOnly,
                        });
                    }
                }
                "dingtalk_webhook" => {
                    if let Some(dingtalk_webhook) = graph.channels.dingtalk_webhook.as_mut() {
                        dingtalk_webhook.enabled = *enabled;
                    } else if *enabled {
                        graph.channels.dingtalk_webhook =
                            Some(crate::config::DingTalkWebhookSection {
                                enabled: true,
                                secret_key: String::new(),
                                webhook_path: "/channels/dingtalk/webhook".to_string(),
                                access_token: None,
                                presentation: crate::config::ProgressPresentationMode::FinalOnly,
                            });
                    }
                }
                _ => {}
            },
            Self::SetWeChatPresentation(presentation) => {
                if let Some(wechat) = graph.channels.wechat.as_mut() {
                    wechat.presentation = *presentation;
                } else {
                    graph.channels.wechat = Some(crate::config::WeChatSection {
                        enabled: true,
                        presentation: *presentation,
                    });
                }
            }
            Self::UpsertBinding { key, value } => {
                graph.bindings.insert(key.to_string(), value.clone());
            }
            Self::RemoveBinding(key) => {
                graph.bindings.remove(key.as_str());
            }
            Self::UpsertDeliverySenderBinding { key, value } => {
                graph
                    .delivery_sender_bindings
                    .insert(key.to_string(), value.clone());
            }
            Self::RemoveDeliverySenderBinding(key) => {
                graph.delivery_sender_bindings.remove(key.as_str());
            }
            Self::UpsertDeliveryTargetOverride { key, value } => {
                graph
                    .delivery_target_overrides
                    .insert(key.to_string(), value.clone());
            }
            Self::RemoveDeliveryTargetOverride(key) => {
                graph.delivery_target_overrides.remove(key.as_str());
            }
            Self::UpsertTeamScope { key, value } => {
                graph.team_scopes.insert(key.clone(), value.clone());
            }
            Self::RemoveTeamScope(key) => {
                graph.team_scopes.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BackendApprovalConfig, BackendFamilyConfig, BackendLaunchConfig, DeliveryPurposeConfig,
        GroupModeConfig, GroupTeamConfig, InteractionMode, ProviderProfileProtocolConfig,
    };

    #[test]
    fn upsert_provider_and_backend_work() {
        let mut graph = ConfigGraph::default();
        ConfigPatch::UpsertProvider(ProviderProfileConfig {
            id: "deepseek-anthropic".to_string(),
            protocol: ProviderProfileProtocolConfig::AnthropicCompatible {
                base_url: "https://api.deepseek.com/anthropic".to_string(),
                auth_token_env: "DEEPSEEK_API_KEY".to_string(),
                default_model: "deepseek-chat".to_string(),
                small_fast_model: None,
            },
        })
        .apply(&mut graph);
        ConfigPatch::UpsertBackend(BackendCatalogEntry {
            id: "claude-main".to_string(),
            family: BackendFamilyConfig::Acp,
            adapter_key: None,
            acp_backend: Some(crate::runtime::AcpBackend::Claude),
            acp_auth_method: None,
            codex: None,
            provider_profile: Some("deepseek-anthropic".to_string()),
            approval: BackendApprovalConfig::default(),
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::BundledCommand,
        })
        .apply(&mut graph);

        assert!(graph.providers.contains_key("deepseek-anthropic"));
        assert!(graph.backends.contains_key("claude-main"));
    }

    #[test]
    fn set_wechat_enabled_creates_channel_section() {
        let mut graph = ConfigGraph::default();
        ConfigPatch::SetChannelEnabled {
            channel: "wechat".to_string(),
            enabled: true,
        }
        .apply(&mut graph);
        assert!(graph
            .channels
            .wechat
            .as_ref()
            .is_some_and(|cfg| cfg.enabled));
    }

    #[test]
    fn set_lark_enabled_creates_channel_section() {
        let mut graph = ConfigGraph::default();
        ConfigPatch::SetChannelEnabled {
            channel: "lark".to_string(),
            enabled: true,
        }
        .apply(&mut graph);
        assert!(graph.channels.lark.as_ref().is_some_and(|cfg| cfg.enabled));
    }

    #[test]
    fn set_wechat_presentation_creates_channel_section() {
        let mut graph = ConfigGraph::default();
        ConfigPatch::SetWeChatPresentation(ProgressPresentationMode::ProgressCompact)
            .apply(&mut graph);
        assert_eq!(
            graph.channels.wechat.as_ref().map(|cfg| cfg.presentation),
            Some(ProgressPresentationMode::ProgressCompact)
        );
    }

    #[test]
    fn upsert_team_scope_uses_composite_key() {
        let mut graph = ConfigGraph::default();
        let key = TeamScopeKey::new("wechat", "user:abc");
        ConfigPatch::UpsertTeamScope {
            key: key.clone(),
            value: TeamScopeConfig {
                scope: "user:abc".to_string(),
                name: Some("demo".to_string()),
                mode: GroupModeConfig {
                    interaction: InteractionMode::Team,
                    auto_promote: false,
                    front_bot: Some("claude".to_string()),
                    channel: Some("wechat".to_string()),
                },
                team: GroupTeamConfig {
                    roster: vec!["claw".to_string()],
                    ..Default::default()
                },
            },
        }
        .apply(&mut graph);

        assert!(graph.team_scopes.contains_key(&key));
    }

    #[test]
    fn upsert_binding_uses_stable_key() {
        let mut graph = ConfigGraph::default();
        let binding = BindingConfig::Channel {
            agent: "claw".to_string(),
            channel: "wechat".to_string(),
        };
        let key = BindingKey::from_binding(&binding);
        ConfigPatch::UpsertBinding {
            key: key.clone(),
            value: binding,
        }
        .apply(&mut graph);

        assert!(graph.bindings.contains_key(key.as_str()));
    }

    #[test]
    fn upsert_delivery_sender_binding_uses_stable_key() {
        let mut graph = ConfigGraph::default();
        let binding = DeliverySenderBindingConfig {
            purpose: DeliveryPurposeConfig::Milestone,
            agent: Some("claw".to_string()),
            channel: Some("wechat".to_string()),
            channel_instance: "default".to_string(),
        };
        let key = DeliverySenderBindingKey::from_binding(&binding);
        ConfigPatch::UpsertDeliverySenderBinding {
            key: key.clone(),
            value: binding,
        }
        .apply(&mut graph);
        assert!(graph.delivery_sender_bindings.contains_key(key.as_str()));
    }

    #[test]
    fn upsert_delivery_target_override_uses_stable_key() {
        let mut graph = ConfigGraph::default();
        let override_cfg = DeliveryTargetOverrideConfig {
            purpose: DeliveryPurposeConfig::LeadFinal,
            agent: Some("claw".to_string()),
            channel: Some("wechat".to_string()),
            channel_instance: Some("default".to_string()),
            scope: "user:abc".to_string(),
            reply_to: None,
            thread_ts: None,
        };
        let key = DeliveryTargetOverrideKey::from_binding(&override_cfg);
        ConfigPatch::UpsertDeliveryTargetOverride {
            key: key.clone(),
            value: override_cfg,
        }
        .apply(&mut graph);
        assert!(graph.delivery_target_overrides.contains_key(key.as_str()));
    }
}
