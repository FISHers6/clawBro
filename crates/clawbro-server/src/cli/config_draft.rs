use crate::cli::config_diff::ConfigDiff;
use crate::cli::config_model::ConfigGraph;
use crate::cli::config_patch::ConfigPatch;

#[derive(Debug, Clone)]
pub struct ConfigDraft {
    baseline: ConfigGraph,
    working: ConfigGraph,
    patches: Vec<ConfigPatch>,
}

impl ConfigDraft {
    pub fn new(baseline: ConfigGraph) -> Self {
        Self {
            working: baseline.clone(),
            baseline,
            patches: Vec::new(),
        }
    }

    pub fn baseline(&self) -> &ConfigGraph {
        &self.baseline
    }

    pub fn working(&self) -> &ConfigGraph {
        &self.working
    }

    pub fn patches(&self) -> &[ConfigPatch] {
        &self.patches
    }

    pub fn apply_patch(&mut self, patch: ConfigPatch) {
        patch.apply(&mut self.working);
        self.patches.push(patch);
    }

    pub fn is_dirty(&self) -> bool {
        !self.patches.is_empty()
    }

    pub fn reset(&mut self) {
        self.working = self.baseline.clone();
        self.patches.clear();
    }

    pub fn diff(&self) -> ConfigDiff {
        ConfigDiff::between(&self.baseline, &self.working)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config_patch::ConfigPatch;

    #[test]
    fn draft_tracks_dirty_state_and_reset() {
        let mut draft = ConfigDraft::new(ConfigGraph::default());
        assert!(!draft.is_dirty());
        draft.apply_patch(ConfigPatch::SetChannelEnabled {
            channel: "wechat".to_string(),
            enabled: true,
        });
        assert!(draft.is_dirty());
        assert!(draft
            .working()
            .channels
            .wechat
            .as_ref()
            .is_some_and(|cfg| cfg.enabled));
        draft.reset();
        assert!(!draft.is_dirty());
        assert!(draft.working().channels.wechat.is_none());
    }
}
