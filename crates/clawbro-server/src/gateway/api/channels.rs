use crate::diagnostics::collect_channel_diagnostics;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use super::types::{ApiErrorBody, ApiListResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownChannelKind {
    Lark,
    DingTalk,
    DingTalkWebhook,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChannelApiView {
    pub channel: String,
    pub configured: bool,
    pub enabled: bool,
    pub routing_present: bool,
    pub credential_state: String,
    pub presentation: Option<crate::config::ProgressPresentationMode>,
    pub default_instance: Option<String>,
    pub trigger_policy: Option<crate::config::LarkTriggerPolicyConfig>,
    pub notes: Vec<String>,
}

pub async fn list_channels(State(state): State<AppState>) -> Json<ApiListResponse<ChannelApiView>> {
    Json(ApiListResponse {
        items: channel_views(&state),
    })
}

pub async fn get_channel(
    Path(channel_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ChannelApiView>, (StatusCode, Json<ApiErrorBody>)> {
    channel_views(&state)
        .into_iter()
        .find(|channel| channel.channel == channel_id)
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: format!("channel '{}' not found", channel_id),
                }),
            )
        })
}

fn channel_views(state: &AppState) -> Vec<ChannelApiView> {
    let mut views: Vec<_> = collect_channel_diagnostics(state)
        .into_iter()
        .map(|diagnostic| {
            let (presentation, default_instance, trigger_policy) =
                channel_runtime_details(state, classify_channel(&diagnostic.channel));

            ChannelApiView {
                channel: diagnostic.channel,
                configured: diagnostic.configured,
                enabled: diagnostic.enabled,
                routing_present: diagnostic.routing_present,
                credential_state: diagnostic.credential_state,
                presentation,
                default_instance,
                trigger_policy,
                notes: diagnostic.notes,
            }
        })
        .collect();
    views.sort_by(|a, b| a.channel.cmp(&b.channel));
    views
}

fn classify_channel(channel: &str) -> KnownChannelKind {
    match channel {
        "lark" => KnownChannelKind::Lark,
        "dingtalk" => KnownChannelKind::DingTalk,
        "dingtalk_webhook" => KnownChannelKind::DingTalkWebhook,
        _ => KnownChannelKind::Unknown,
    }
}

fn channel_runtime_details(
    state: &AppState,
    kind: KnownChannelKind,
) -> (
    Option<crate::config::ProgressPresentationMode>,
    Option<String>,
    Option<crate::config::LarkTriggerPolicyConfig>,
) {
    match kind {
        KnownChannelKind::Lark => state
            .cfg
            .channels
            .lark
            .as_ref()
            .map(|cfg| {
                (
                    Some(cfg.presentation),
                    cfg.default_instance.clone(),
                    cfg.trigger_policy,
                )
            })
            .unwrap_or((None, None, None)),
        KnownChannelKind::DingTalk => (
            state
                .cfg
                .channels
                .dingtalk
                .as_ref()
                .map(|cfg| cfg.presentation),
            None,
            None,
        ),
        KnownChannelKind::DingTalkWebhook => (
            state
                .cfg
                .channels
                .dingtalk_webhook
                .as_ref()
                .map(|cfg| cfg.presentation),
            None,
            None,
        ),
        KnownChannelKind::Unknown => (None, None, None),
    }
}
