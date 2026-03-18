use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const TEAM_HELPER_CONTRACT: &str = "clawbro.team_helper";
pub const TEAM_HELPER_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedTeamHelperResult {
    pub action: String,
    pub task_id: Option<String>,
    pub ok: bool,
    pub payload: Value,
}

pub fn render_team_helper_success(action: &str, mut fields: Map<String, Value>) -> Value {
    fields.insert(
        "contract".into(),
        Value::String(TEAM_HELPER_CONTRACT.to_string()),
    );
    fields.insert("version".into(), Value::Number(TEAM_HELPER_VERSION.into()));
    fields.insert("ok".into(), Value::Bool(true));
    fields.insert("action".into(), Value::String(action.to_string()));
    Value::Object(fields)
}

pub fn render_team_helper_failure(action: &str, task_id: Option<&str>, error: &str) -> Value {
    let mut fields = Map::new();
    fields.insert(
        "contract".into(),
        Value::String(TEAM_HELPER_CONTRACT.to_string()),
    );
    fields.insert("version".into(), Value::Number(TEAM_HELPER_VERSION.into()));
    fields.insert("ok".into(), Value::Bool(false));
    fields.insert("action".into(), Value::String(action.to_string()));
    if let Some(task_id) = task_id {
        fields.insert("task_id".into(), Value::String(task_id.to_string()));
    }
    fields.insert("error".into(), Value::String(error.to_string()));
    Value::Object(fields)
}

impl ParsedTeamHelperResult {
    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("team helper result must be a JSON object"))?;
        let contract = object
            .get("contract")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("team helper result missing contract"))?;
        if contract != TEAM_HELPER_CONTRACT {
            return Err(anyhow!(
                "unsupported team helper contract: expected `{}`, got `{}`",
                TEAM_HELPER_CONTRACT,
                contract
            ));
        }

        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("team helper result missing version"))?;
        if version != TEAM_HELPER_VERSION as u64 {
            return Err(anyhow!(
                "unsupported team helper contract version: expected `{}`, got `{}`",
                TEAM_HELPER_VERSION,
                version
            ));
        }

        let action = object
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("team helper result missing action"))?;
        let ok = object
            .get("ok")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("team helper result missing ok flag"))?;

        Ok(Self {
            action: action.to_string(),
            task_id: object
                .get("task_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            ok,
            payload: value.clone(),
        })
    }
}

pub fn required_string_field(value: &Value, key: &str, label: &str) -> anyhow::Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .with_context(|| format!("{label} missing {key}"))
}

pub fn optional_string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_success_includes_contract_metadata() {
        let rendered = render_team_helper_success(
            "submit_task_result",
            Map::from_iter([
                ("task_id".into(), Value::String("T001".into())),
                ("summary".into(), Value::String("done".into())),
            ]),
        );

        assert_eq!(rendered["contract"], TEAM_HELPER_CONTRACT);
        assert_eq!(rendered["version"], TEAM_HELPER_VERSION);
        assert_eq!(rendered["ok"], true);
        assert_eq!(rendered["action"], "submit_task_result");
    }

    #[test]
    fn parse_rejects_wrong_version() {
        let bad = serde_json::json!({
            "contract": TEAM_HELPER_CONTRACT,
            "version": TEAM_HELPER_VERSION + 1,
            "ok": true,
            "action": "submit_task_result"
        });

        let err = ParsedTeamHelperResult::from_json(&bad).unwrap_err();
        assert!(err
            .to_string()
            .contains("unsupported team helper contract version"));
    }
}
