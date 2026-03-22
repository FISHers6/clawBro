use crate::config::GatewayConfig;
use crate::gateway::api::types::ApiErrorBody;
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct RawConfigResponse {
    pub content: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PutRawConfigBody {
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteConfigResponse {
    pub ok: bool,
    pub path: String,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidateConfigResponse {
    pub ok: bool,
    pub error: Option<String>,
}

pub async fn get_raw_config(State(state): State<AppState>) -> Json<RawConfigResponse> {
    let path = state.config_path.as_ref();
    let content = std::fs::read_to_string(path).unwrap_or_default();
    Json(RawConfigResponse {
        content,
        path: path.display().to_string(),
    })
}

pub async fn put_raw_config(
    State(state): State<AppState>,
    Json(body): Json<PutRawConfigBody>,
) -> Result<Json<WriteConfigResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let path = state.config_path.as_ref();

    let cfg = GatewayConfig::from_toml_str(&body.content).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: format!("TOML parse error: {e}"),
            }),
        )
    })?;

    cfg.validate_runtime_topology().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: format!("Validation error: {e}"),
            }),
        )
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: format!("Failed to create parent directory: {e}"),
                }),
            )
        })?;
    }

    std::fs::write(path, &body.content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: format!("Failed to write config file: {e}"),
            }),
        )
    })?;

    Ok(Json(WriteConfigResponse {
        ok: true,
        path: path.display().to_string(),
        restart_required: true,
    }))
}

pub async fn validate_config(Json(body): Json<PutRawConfigBody>) -> Json<ValidateConfigResponse> {
    match GatewayConfig::from_toml_str(&body.content) {
        Err(e) => Json(ValidateConfigResponse {
            ok: false,
            error: Some(format!("TOML parse error: {e}")),
        }),
        Ok(cfg) => match cfg.validate_runtime_topology() {
            Err(e) => Json(ValidateConfigResponse {
                ok: false,
                error: Some(format!("Validation error: {e}")),
            }),
            Ok(()) => Json(ValidateConfigResponse {
                ok: true,
                error: None,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validate_returns_ok_for_minimal_config() {
        // A minimal valid config with at least one [[backend]] entry
        let toml = r#"
[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"
"#;
        let body = PutRawConfigBody {
            content: toml.to_string(),
        };
        let Json(resp) = validate_config(Json(body)).await;
        assert!(resp.ok, "expected ok=true, got error: {:?}", resp.error);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn validate_returns_error_for_invalid_toml() {
        let body = PutRawConfigBody {
            content: "this is not valid toml ][[[".to_string(),
        };
        let Json(resp) = validate_config(Json(body)).await;
        assert!(!resp.ok, "expected ok=false for invalid TOML");
        assert!(resp.error.is_some());
    }
}
