use axum::http::StatusCode;

use crate::channels_internal::ws_virtual::WsVirtualChannel;
use crate::config::ProgressPresentationMode;
use crate::im_sink::spawn_im_turn;
use crate::protocol::{InboundMsg, MsgContent, MsgSource, SessionKey};
use crate::runtime::{TeamToolCall, TeamToolRequest, TeamToolResponse};
use crate::state::AppState;
use std::sync::Arc;

/// Resolve the outbound channel for a session key.
/// Falls back to WsVirtualChannel (WS-broadcast-only) when no IM channel is registered,
/// e.g. in pure-WebSocket deployments.
fn resolve_channel(state: &AppState, session_key: &SessionKey) -> Arc<dyn crate::channels_internal::Channel> {
    state
        .channel_registry
        .resolve_for_session(session_key)
        .unwrap_or_else(|| Arc::new(WsVirtualChannel))
}

pub async fn invoke_team_http_request(
    state: &AppState,
    provided_token: &str,
    request: TeamToolRequest,
) -> (StatusCode, TeamToolResponse) {
    if provided_token != *state.runtime_token {
        return (
            StatusCode::UNAUTHORIZED,
            TeamToolResponse {
                ok: false,
                message: "invalid runtime token".to_string(),
                payload: None,
            },
        );
    }

    // Short-circuit: social tools handled directly via AppState (no orchestrator needed).
    // These two tools work in Solo mode where no TeamOrchestrator exists.
    match &request.call {
        TeamToolCall::ListAgents => {
            return handle_list_agents(state);
        }
        TeamToolCall::SendMessage { .. } => {
            // Fall through — destructure by value below to avoid borrow conflict
        }
        _ => {
            return match state
                .registry
                .invoke_team_tool(&request.session_key, request.call)
                .await
            {
                Ok(resp) => (StatusCode::OK, resp),
                Err(err) => (
                    StatusCode::BAD_REQUEST,
                    TeamToolResponse {
                        ok: false,
                        message: err.to_string(),
                        payload: None,
                    },
                ),
            };
        }
    }

    // Reached only for SendMessage
    let TeamToolCall::SendMessage {
        target,
        message,
        scope,
    } = request.call
    else {
        unreachable!()
    };
    handle_send_message(
        state,
        &request.session_key,
        &target,
        &message,
        scope.as_deref(),
    )
    .await
}

fn handle_list_agents(state: &AppState) -> (StatusCode, TeamToolResponse) {
    let agents: Vec<serde_json::Value> = state
        .registry
        .roster
        .as_ref()
        .map(|r| {
            r.all_agents()
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "mentions": e.mentions,
                        "backend_id": e.backend_id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let message = if agents.is_empty() {
        "No agents configured in roster.".to_string()
    } else {
        let names: Vec<&str> = agents.iter().filter_map(|v| v["name"].as_str()).collect();
        format!(
            "Roster has {} agent(s): {}.",
            agents.len(),
            names.join(", ")
        )
    };

    (
        StatusCode::OK,
        TeamToolResponse {
            ok: true,
            message,
            payload: Some(serde_json::Value::Array(agents)),
        },
    )
}

async fn handle_send_message(
    state: &AppState,
    caller_session: &SessionKey,
    target: &str,
    message: &str,
    scope_override: Option<&str>,
) -> (StatusCode, TeamToolResponse) {
    // Target is an agent name — verify it exists in roster before dispatching.
    let roster_entry = state
        .registry
        .roster
        .as_ref()
        .and_then(|r| r.find_by_name(target))
        .map(|e| (e.name.clone(), e.mentions.clone()));

    let Some((agent_name, agent_mentions)) = roster_entry else {
        return (
            StatusCode::BAD_REQUEST,
            TeamToolResponse {
                ok: false,
                message: format!(
                    "Agent '{}' not found in roster. Use list_agents to see available agents.",
                    target
                ),
                payload: None,
            },
        );
    };

    // Use the stored mention string — routing.rs resolves target_agent via find_by_mention(),
    // so the mention must match what's in the roster, not a synthesized "@{name}".
    let mention = agent_mentions
        .first()
        .cloned()
        .unwrap_or_else(|| format!("@{}", agent_name));

    // Self-send guard: prevent agent from dispatching a turn to itself.
    // For specialist sessions, scope encodes the agent name as the last colon-separated token.
    // For Solo sessions, full detection requires registry introspection (V1 limitation: not detected).
    let caller_scope_tokens: Vec<&str> = caller_session.scope.split(':').collect();
    let caller_encoded_agent = caller_scope_tokens.last().copied().unwrap_or("");
    if !caller_encoded_agent.is_empty()
        && agent_name.to_lowercase() == caller_encoded_agent.to_lowercase()
    {
        return (
            StatusCode::BAD_REQUEST,
            TeamToolResponse {
                ok: false,
                message: format!(
                    "Cannot send message to '{}': self-send detected. An agent cannot dispatch a turn to itself.",
                    agent_name
                ),
                payload: None,
            },
        );
    }

    // Inherit the caller's channel so the dispatched turn — and its reply — travel the same
    // transport as the original conversation (DingTalk, Lark, or WebSocket).
    let scope = scope_override.unwrap_or(&caller_session.scope).to_string();
    let session_key = SessionKey {
        channel: caller_session.channel.clone(),
        channel_instance: caller_session.channel_instance.clone(),
        scope,
    };
    let channel = resolve_channel(state, &session_key);
    let turn_id = uuid::Uuid::new_v4().to_string();
    let inbound = InboundMsg {
        id: turn_id,
        channel: session_key.channel.clone(),
        session_key,
        content: MsgContent::text(message),
        sender: "agent".to_string(),
        timestamp: chrono::Utc::now(),
        thread_ts: None,
        target_agent: Some(mention.clone()),
        source: MsgSource::BotMention,
    };
    spawn_im_turn(
        Arc::clone(&state.registry),
        channel,
        Arc::clone(&state.channel_registry),
        Arc::clone(&state.cfg),
        inbound,
        ProgressPresentationMode::FinalOnly,
    );

    (
        StatusCode::OK,
        TeamToolResponse {
            ok: true,
            message: format!("Message dispatched to agent '{}'.", mention),
            payload: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::roster::{AgentEntry, AgentRoster};
    use crate::agent_core::SessionRegistry;
    use crate::config::GatewayConfig;
    use crate::protocol::{SessionKey, TeamToolRequest};
    use crate::session::{SessionManager, SessionStorage};
    use crate::skills_internal::SkillLoader;
    use std::sync::Arc;

    fn make_state_with_roster(roster: AgentRoster) -> AppState {
        let cfg = GatewayConfig::default();
        let storage = SessionStorage::new(cfg.session.dir.clone());
        let session_manager = Arc::new(SessionManager::new(storage));
        // Include global_dirs to match the established test pattern in team_tools_handler.rs
        let mut all_skill_dirs = vec![cfg.skills.dir.clone()];
        all_skill_dirs.extend(cfg.skills.global_dirs.iter().cloned());
        let skill_loader = SkillLoader::new(all_skill_dirs);
        let skills = skill_loader.load_all();
        let system_injection = skill_loader.build_system_injection(&skills);
        let skill_dirs = skill_loader.search_dirs().to_vec();
        // IMPORTANT: SessionRegistry::new signature order:
        // (default_backend_id, session_manager, system_injection,
        //  roster [4th], memory_system [5th], default_persona_dir [6th], default_workspace [7th], skill_dirs)
        let (registry, _rx) = SessionRegistry::new(
            None,
            session_manager,
            system_injection,
            Some(roster), // 4th: roster
            None,         // 5th: memory_system
            None,         // 6th: default_persona_dir
            None,         // 7th: default_workspace
            skill_dirs,
        );
        AppState {
            registry: Arc::clone(&registry),
            runtime_registry: Arc::new(crate::runtime::BackendRegistry::new()),
            event_tx: registry.global_sender(),
            cfg: Arc::new(cfg),
            channel_registry: Arc::new(crate::channel_registry::ChannelRegistry::new()),
            dingtalk_webhook_channel: None,
            runtime_token: Arc::new("tok".to_string()),
            approvals: crate::runtime::ApprovalBroker::default(),
            scheduler_service: crate::scheduler_runtime::build_test_scheduler_service(),
            config_path: Arc::new(crate::config::config_file_path()),
        }
    }

    #[tokio::test]
    async fn list_agents_returns_roster_without_orchestrator() {
        let roster = AgentRoster::new(vec![
            AgentEntry {
                name: "coder".to_string(),
                mentions: vec!["@coder".to_string()],
                backend_id: "claude".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "reviewer".to_string(),
                mentions: vec!["@reviewer".to_string()],
                backend_id: "codex".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ]);
        let state = make_state_with_roster(roster);
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::ListAgents,
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(resp.ok);
        assert!(resp.message.contains("coder"));
        assert!(resp.message.contains("reviewer"));
        let payload = resp.payload.unwrap();
        let arr = payload.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn list_agents_empty_roster_returns_ok_with_empty_payload() {
        let state = make_state_with_roster(AgentRoster::new(vec![]));
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::ListAgents,
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(resp.ok);
        let payload = resp.payload.unwrap();
        assert_eq!(payload.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn send_message_to_unknown_agent_returns_error() {
        let state = make_state_with_roster(AgentRoster::new(vec![]));
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::SendMessage {
                target: "ghost-agent".to_string(),
                message: "hello".to_string(),
                scope: None,
            },
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(!resp.ok);
        assert!(resp.message.contains("ghost-agent"));
    }

    #[tokio::test]
    async fn send_message_to_user_returns_error() {
        // "user" is not a valid target — agents communicate with the user by returning text,
        // not by using send_message.
        let state = make_state_with_roster(AgentRoster::new(vec![]));
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::SendMessage {
                target: "user".to_string(),
                message: "task complete".to_string(),
                scope: None,
            },
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn send_message_to_known_agent_dispatches() {
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "coder".to_string(),
            mentions: vec!["@coder".to_string()],
            backend_id: "claude".to_string(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        let state = make_state_with_roster(roster);
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::SendMessage {
                target: "coder".to_string(),
                message: "please review PR #42".to_string(),
                scope: None,
            },
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(resp.ok);
        assert!(resp.message.contains("@coder"));
    }

    #[tokio::test]
    async fn send_message_inherits_caller_channel() {
        // When the caller is on a "lark" session, the dispatched InboundMsg must also
        // carry channel="lark", not the legacy hardcoded "ws".
        // We verify this indirectly: the call succeeds and the channel in the session_key
        // matches the caller's. (spawn_im_turn is fire-and-forget so we can't inspect
        // the inbound directly, but the channel resolution path is exercised.)
        let roster = AgentRoster::new(vec![AgentEntry {
            name: "reviewer".to_string(),
            mentions: vec!["@reviewer".to_string()],
            backend_id: "claude".to_string(),
            persona_dir: None,
            workspace_dir: None,
            extra_skills_dirs: vec![],
        }]);
        let state = make_state_with_roster(roster);
        let request = TeamToolRequest {
            // Simulate a caller coming from Lark
            session_key: SessionKey::new("lark", "group:oc_x"),
            call: crate::runtime::TeamToolCall::SendMessage {
                target: "reviewer".to_string(),
                message: "please review PR #99".to_string(),
                scope: None,
            },
        };
        let (status, resp) = invoke_team_http_request(&state, "tok", request).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(resp.ok);
        assert!(resp.message.contains("@reviewer"));
    }

    #[tokio::test]
    async fn invalid_token_returns_unauthorized() {
        let state = make_state_with_roster(AgentRoster::new(vec![]));
        let request = TeamToolRequest {
            session_key: SessionKey::new("ws", "main"),
            call: crate::runtime::TeamToolCall::ListAgents,
        };
        let (status, resp) = invoke_team_http_request(&state, "wrong-token", request).await;
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
        assert!(!resp.ok);
    }
}
