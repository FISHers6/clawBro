use crate::cli::config_model::ConfigGraph;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    pub lines: Vec<String>,
}

impl ConfigDiff {
    pub fn between(before: &ConfigGraph, after: &ConfigGraph) -> Self {
        let mut lines = Vec::new();

        if before.provider_ids() != after.provider_ids() {
            lines.push(format!(
                "providers: [{}] -> [{}]",
                join_set(before.provider_ids()),
                join_set(after.provider_ids())
            ));
        }
        if before.backend_ids() != after.backend_ids() {
            lines.push(format!(
                "backends: [{}] -> [{}]",
                join_set(before.backend_ids()),
                join_set(after.backend_ids())
            ));
        }
        if before.agent_names() != after.agent_names() {
            lines.push(format!(
                "agents: [{}] -> [{}]",
                join_set(before.agent_names()),
                join_set(after.agent_names())
            ));
        }

        let before_wechat = before.channels.wechat.as_ref().map(|cfg| cfg.enabled);
        let after_wechat = after.channels.wechat.as_ref().map(|cfg| cfg.enabled);
        if before_wechat != after_wechat {
            lines.push(format!(
                "channel wechat enabled: {} -> {}",
                format_bool(before_wechat),
                format_bool(after_wechat)
            ));
        }

        if before.team_scopes.len() != after.team_scopes.len() {
            lines.push(format!(
                "team_scopes: {} -> {}",
                before.team_scopes.len(),
                after.team_scopes.len()
            ));
        }

        Self { lines }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

fn join_set(values: impl IntoIterator<Item = String>) -> String {
    values.into_iter().collect::<Vec<_>>().join(", ")
}

fn format_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unset",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config_patch::ConfigPatch;

    #[test]
    fn diff_reports_wechat_enablement_change() {
        let before = ConfigGraph::default();
        let mut after = ConfigGraph::default();
        ConfigPatch::SetChannelEnabled {
            channel: "wechat".to_string(),
            enabled: true,
        }
        .apply(&mut after);
        let diff = ConfigDiff::between(&before, &after);
        assert!(diff
            .lines
            .iter()
            .any(|line| line.contains("channel wechat enabled")));
    }
}
