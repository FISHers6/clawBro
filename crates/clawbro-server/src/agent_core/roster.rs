//! AgentRoster: user-configured @mention -> backend mapping.
//! @mention names are 100% user-defined (e.g. "@mybot", "@dev-agent").
//! Channels extract the mention from platform messages; roster resolves it to a backend.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One entry in the user-configured agent roster.
/// `mentions` is a user-defined list of @mention strings (e.g. ["@mybot", "@dev"]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    /// Human-readable identifier (used for sender annotation, e.g. "@mybot")
    pub name: String,
    /// User-configured @mention strings that route to this agent
    pub mentions: Vec<String>,
    /// Runtime backend id from the backend catalog.
    pub backend_id: String,
    /// Optional directory with SOUL.md / IDENTITY.md / MEMORY.md persona files
    #[serde(default)]
    pub persona_dir: Option<PathBuf>,
    /// Directory to use as working directory when spawning the agent subprocess.
    #[serde(default)]
    pub workspace_dir: Option<PathBuf>,
    /// Explicit extra skill directories for this agent (in addition to workspace-derived shared/private/agent-scoped skill dirs).
    #[serde(default)]
    pub extra_skills_dirs: Vec<PathBuf>,
}

/// User-configured roster of agents for a gateway instance.
/// Each entry maps user-defined @mentions to a backend.
pub struct AgentRoster {
    agents: Vec<AgentEntry>,
}

impl AgentRoster {
    pub fn new(agents: Vec<AgentEntry>) -> Self {
        Self { agents }
    }

    /// Find agent by exact @mention string (case-insensitive).
    /// Returns the first entry whose `mentions` list contains `mention`.
    /// Used by SessionRegistry after the Channel has extracted target_agent.
    pub fn find_by_mention<'a>(&'a self, mention: &str) -> Option<&'a AgentEntry> {
        let mention_lower = mention.to_lowercase();
        self.agents.iter().find(|entry| {
            entry
                .mentions
                .iter()
                .any(|m| m.to_lowercase() == mention_lower)
        })
    }

    /// Find agent by name (case-insensitive). Useful for slash commands like `/backend mybot`.
    pub fn find_by_name<'a>(&'a self, name: &str) -> Option<&'a AgentEntry> {
        let name_lower = name.to_lowercase();
        self.agents
            .iter()
            .find(|entry| entry.name.to_lowercase() == name_lower)
    }

    /// Returns true if message text contains "@all" broadcast trigger.
    pub fn is_broadcast(text: &str) -> bool {
        text.to_lowercase().contains("@all")
    }

    /// All agents in roster order.
    pub fn all_agents(&self) -> &[AgentEntry] {
        &self.agents
    }

    /// Default agent (first in roster). Used when no target_agent is set.
    pub fn default_agent(&self) -> Option<&AgentEntry> {
        self.agents.first()
    }
}

impl AgentEntry {
    pub fn runtime_backend_id(&self) -> &str {
        self.backend_id.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_roster() -> AgentRoster {
        AgentRoster::new(vec![
            AgentEntry {
                name: "mybot".to_string(),
                mentions: vec!["@mybot".to_string(), "@dev-assistant".to_string()],
                backend_id: "claude-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
            AgentEntry {
                name: "reviewer".to_string(),
                mentions: vec!["@reviewer".to_string()],
                backend_id: "codex-main".to_string(),
                persona_dir: None,
                workspace_dir: None,
                extra_skills_dirs: vec![],
            },
        ])
    }

    #[test]
    fn test_find_by_mention_exact() {
        let roster = make_roster();
        let entry = roster.find_by_mention("@mybot").unwrap();
        assert_eq!(entry.name, "mybot");
    }

    #[test]
    fn test_find_by_mention_alias() {
        let roster = make_roster();
        let entry = roster.find_by_mention("@dev-assistant").unwrap();
        assert_eq!(entry.name, "mybot");
    }

    #[test]
    fn test_find_by_mention_case_insensitive() {
        let roster = make_roster();
        let entry = roster.find_by_mention("@MYBOT").unwrap();
        assert_eq!(entry.name, "mybot");
    }

    #[test]
    fn test_find_by_mention_no_match() {
        let roster = make_roster();
        assert!(roster.find_by_mention("@unknown-bot").is_none());
    }

    #[test]
    fn test_find_by_mention_empty_string() {
        let roster = make_roster();
        assert!(roster.find_by_mention("").is_none());
    }

    #[test]
    fn test_find_by_name() {
        let roster = make_roster();
        let entry = roster.find_by_name("reviewer").unwrap();
        assert_eq!(entry.name, "reviewer");
    }

    #[test]
    fn test_find_by_name_case_insensitive() {
        let roster = make_roster();
        let entry = roster.find_by_name("MYBOT").unwrap();
        assert_eq!(entry.name, "mybot");
    }

    #[test]
    fn test_find_by_name_no_match() {
        let roster = make_roster();
        assert!(roster.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_is_broadcast_true() {
        assert!(AgentRoster::is_broadcast("@all hello everyone"));
        assert!(AgentRoster::is_broadcast("@ALL"));
    }

    #[test]
    fn test_is_broadcast_false() {
        assert!(!AgentRoster::is_broadcast("@mybot please help"));
        assert!(!AgentRoster::is_broadcast("hello world"));
    }

    #[test]
    fn test_default_agent_is_first() {
        let roster = make_roster();
        assert_eq!(roster.default_agent().unwrap().name, "mybot");
    }

    #[test]
    fn test_empty_roster_default_none() {
        let roster = AgentRoster::new(vec![]);
        assert!(roster.default_agent().is_none());
    }

    #[test]
    fn test_agent_entry_workspace_dir_deserialises() {
        let toml = r#"
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"
workspace_dir = "/projects/my-app"
    "#;
        let entry: AgentEntry = toml::from_str(toml).unwrap();
        assert_eq!(
            entry.workspace_dir,
            Some(std::path::PathBuf::from("/projects/my-app"))
        );
    }

    #[test]
    fn test_agent_entry_workspace_dir_defaults_to_none() {
        let toml = r#"
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"
    "#;
        let entry: AgentEntry = toml::from_str(toml).unwrap();
        assert!(entry.workspace_dir.is_none());
    }

    #[test]
    fn test_agent_entry_extra_skills_dirs_deserialises() {
        let toml = r#"
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"
extra_skills_dirs = ["/custom/skills"]
        "#;
        let entry: AgentEntry = toml::from_str(toml).unwrap();
        assert_eq!(entry.extra_skills_dirs.len(), 1);
        assert_eq!(
            entry.extra_skills_dirs[0],
            std::path::PathBuf::from("/custom/skills")
        );
    }

    #[test]
    fn test_agent_entry_backend_id_deserialises() {
        let toml = r#"
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"
        "#;
        let entry: AgentEntry = toml::from_str(toml).unwrap();
        assert_eq!(entry.backend_id, "claude-main");
        assert_eq!(entry.runtime_backend_id(), "claude-main");
    }

    #[test]
    fn test_agent_entry_extra_skills_dirs_defaults_empty() {
        let toml = r#"
name = "claude"
mentions = ["@claude"]
backend_id = "claude-main"
        "#;
        let entry: AgentEntry = toml::from_str(toml).unwrap();
        assert!(entry.extra_skills_dirs.is_empty());
    }
}
