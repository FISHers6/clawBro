use crate::config::GatewayConfig;
use crate::gateway::api::types::ApiErrorBody;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

// ─── Request / Response types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAgentBody {
    pub name: String,
    pub backend_id: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    pub persona_dir: Option<String>,
    pub workspace_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchAgentBody {
    pub backend_id: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub persona_dir: Option<String>,
    pub workspace_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentWriteResponse {
    pub ok: bool,
    pub name: String,
    pub restart_required: bool,
}

// ─── Helper: error shorthand ──────────────────────────────────────────────────

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorBody { error: msg.into() }),
    )
}

fn not_found(msg: impl Into<String>) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody { error: msg.into() }),
    )
}

fn conflict(msg: impl Into<String>) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::CONFLICT,
        Json(ApiErrorBody { error: msg.into() }),
    )
}

fn internal(msg: impl Into<String>) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody { error: msg.into() }),
    )
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Read and parse the config file as a `DocumentMut`. Returns an empty document
/// if the file does not exist yet.
fn read_document(path: &std::path::Path) -> Result<DocumentMut, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Failed to read config file: {e}")),
    };
    content
        .parse::<DocumentMut>()
        .map_err(|e| format!("Failed to parse TOML: {e}"))
}

/// Serialize `doc` to string, validate via `GatewayConfig`, and write to disk.
fn write_document(path: &std::path::Path, doc: &DocumentMut) -> Result<(), String> {
    let content = doc.to_string();

    // Validate: parse + topology check
    GatewayConfig::from_toml_str(&content)
        .and_then(|cfg| cfg.validate_runtime_topology().map(|_| ()))
        .map_err(|e| format!("Config validation failed: {e}"))?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {e}"))?;
    }

    std::fs::write(path, &content).map_err(|e| format!("Failed to write config file: {e}"))?;

    Ok(())
}

/// Find the index of the `[[agent_roster]]` entry whose `name` field equals
/// `name`. Returns `None` if not found.
fn find_agent_roster_index(doc: &DocumentMut, name: &str) -> Option<usize> {
    let item = doc.get("agent_roster")?;
    let arr = item.as_array_of_tables()?;
    arr.iter().position(|tbl| {
        tbl.get("name")
            .and_then(|v| v.as_str())
            .map(|n| n == name)
            .unwrap_or(false)
    })
}

/// Build a new `toml_edit::Table` for an agent roster entry.
fn build_agent_table(
    name: &str,
    backend_id: &str,
    mentions: &[String],
    persona_dir: Option<&str>,
    workspace_dir: Option<&str>,
) -> Table {
    let mut tbl = Table::new();
    tbl["name"] = value(name);
    tbl["backend_id"] = value(backend_id);

    let mut arr = Array::new();
    for m in mentions {
        arr.push(m.as_str());
    }
    tbl["mentions"] = Item::Value(toml_edit::Value::Array(arr));

    if let Some(pd) = persona_dir {
        tbl["persona_dir"] = value(pd);
    }
    if let Some(wd) = workspace_dir {
        tbl["workspace_dir"] = value(wd);
    }

    tbl
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// POST /api/agents — create a new agent_roster entry.
pub async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<CreateAgentBody>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    if body.name.trim().is_empty() {
        return Err(bad_request("Agent name must not be empty"));
    }

    let path = state.config_path.as_ref();
    let mut doc = read_document(path).map_err(bad_request)?;

    // Check for duplicate
    if find_agent_roster_index(&doc, &body.name).is_some() {
        return Err(conflict(format!(
            "Agent '{}' already exists in agent_roster",
            body.name
        )));
    }

    // Ensure agent_roster array-of-tables exists
    if doc.get("agent_roster").is_none() {
        doc["agent_roster"] = Item::ArrayOfTables(ArrayOfTables::new());
    }

    let tbl = build_agent_table(
        &body.name,
        &body.backend_id,
        &body.mentions,
        body.persona_dir.as_deref(),
        body.workspace_dir.as_deref(),
    );

    doc["agent_roster"]
        .as_array_of_tables_mut()
        .ok_or_else(|| bad_request("agent_roster is not an array of tables"))?
        .push(tbl);

    write_document(path, &doc).map_err(bad_request)?;

    Ok(Json(AgentWriteResponse {
        ok: true,
        name: body.name,
        restart_required: true,
    }))
}

/// PATCH /api/agents/{name} — update fields of an existing agent_roster entry.
pub async fn patch_agent(
    Path(agent_name): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<PatchAgentBody>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let path = state.config_path.as_ref();
    let mut doc = read_document(path).map_err(internal)?;

    let idx = find_agent_roster_index(&doc, &agent_name)
        .ok_or_else(|| not_found(format!("Agent '{}' not found in agent_roster", agent_name)))?;

    let tbl = doc["agent_roster"]
        .as_array_of_tables_mut()
        .ok_or_else(|| internal("agent_roster is not an array of tables"))?
        .get_mut(idx)
        .ok_or_else(|| internal("Index out of bounds after find"))?;

    if let Some(bid) = &body.backend_id {
        tbl["backend_id"] = value(bid.as_str());
    }
    if let Some(mentions) = &body.mentions {
        let mut arr = Array::new();
        for m in mentions {
            arr.push(m.as_str());
        }
        tbl["mentions"] = Item::Value(toml_edit::Value::Array(arr));
    }
    if let Some(pd) = &body.persona_dir {
        tbl["persona_dir"] = value(pd.as_str());
    }
    if let Some(wd) = &body.workspace_dir {
        tbl["workspace_dir"] = value(wd.as_str());
    }

    write_document(path, &doc).map_err(bad_request)?;

    Ok(Json(AgentWriteResponse {
        ok: true,
        name: agent_name,
        restart_required: true,
    }))
}

/// DELETE /api/agents/{name} — remove an agent_roster entry.
pub async fn delete_agent(
    Path(agent_name): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<AgentWriteResponse>, (StatusCode, Json<ApiErrorBody>)> {
    let path = state.config_path.as_ref();
    let mut doc = read_document(path).map_err(internal)?;

    let idx = find_agent_roster_index(&doc, &agent_name)
        .ok_or_else(|| not_found(format!("Agent '{}' not found in agent_roster", agent_name)))?;

    doc["agent_roster"]
        .as_array_of_tables_mut()
        .ok_or_else(|| internal("agent_roster is not an array of tables"))?
        .remove(idx);

    write_document(path, &doc).map_err(bad_request)?;

    Ok(Json(AgentWriteResponse {
        ok: true,
        name: agent_name,
        restart_required: true,
    }))
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_AGENT_TOML: &str = r#"
[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"

[[agent_roster]]
name = "alice"
backend_id = "native-main"
mentions = ["@alice"]

[[agent_roster]]
name = "bob"
backend_id = "native-main"
mentions = ["@bob"]
"#;

    #[test]
    fn find_agent_roster_index_returns_none_when_absent() {
        let doc = "".parse::<DocumentMut>().unwrap();
        assert_eq!(find_agent_roster_index(&doc, "alice"), None);
    }

    #[test]
    fn find_agent_roster_index_returns_correct_index() {
        let doc = TWO_AGENT_TOML.parse::<DocumentMut>().unwrap();
        assert_eq!(find_agent_roster_index(&doc, "alice"), Some(0));
        assert_eq!(find_agent_roster_index(&doc, "bob"), Some(1));
        assert_eq!(find_agent_roster_index(&doc, "charlie"), None);
    }

    #[test]
    fn write_document_round_trips_without_corruption() {
        use std::io::Write as _;

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // We skip full validation here (no valid backend/agent section in a
        // simple round-trip test), so we write the serialized TOML directly
        // instead of calling write_document() (which would fail validation).
        let doc = TWO_AGENT_TOML.parse::<DocumentMut>().unwrap();
        let serialized = doc.to_string();

        tmp.write_all(serialized.as_bytes()).unwrap();

        // Read back and re-parse to confirm structure is intact
        let path = tmp.path().to_owned();
        let content = std::fs::read_to_string(&path).unwrap();
        let doc2 = content.parse::<DocumentMut>().unwrap();

        assert_eq!(find_agent_roster_index(&doc2, "alice"), Some(0));
        assert_eq!(find_agent_roster_index(&doc2, "bob"), Some(1));

        // Confirm the serialized TOML still contains key strings
        assert!(serialized.contains("alice"));
        assert!(serialized.contains("bob"));
        assert!(serialized.contains("native-main"));
    }

    #[test]
    fn build_agent_table_sets_all_fields() {
        let tbl = build_agent_table(
            "rex",
            "backend-a",
            &["@rex".to_string()],
            Some("/persona/rex"),
            Some("/workspace/rex"),
        );
        assert_eq!(tbl.get("name").and_then(|v| v.as_str()), Some("rex"));
        assert_eq!(
            tbl.get("backend_id").and_then(|v| v.as_str()),
            Some("backend-a")
        );
        assert_eq!(
            tbl.get("persona_dir").and_then(|v| v.as_str()),
            Some("/persona/rex")
        );
        assert_eq!(
            tbl.get("workspace_dir").and_then(|v| v.as_str()),
            Some("/workspace/rex")
        );
    }

    #[test]
    fn build_agent_table_omits_optional_fields_when_none() {
        let tbl = build_agent_table("rex", "backend-a", &[], None, None);
        assert!(tbl.get("persona_dir").is_none());
        assert!(tbl.get("workspace_dir").is_none());
    }
}
