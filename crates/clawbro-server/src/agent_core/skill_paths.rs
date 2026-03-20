use std::path::{Path, PathBuf};

pub fn project_universal_skills_dir(workspace: &Path) -> PathBuf {
    workspace.join(".agents").join("skills")
}

pub fn workspace_private_skills_dir(workspace: &Path) -> PathBuf {
    workspace.join("skills")
}

pub fn agent_scoped_skills_dir(workspace: &Path, agent_name: &str) -> PathBuf {
    workspace
        .join(".agents")
        .join("agents")
        .join(sanitize_agent_name_for_path(agent_name))
        .join("skills")
}

pub fn sanitize_agent_name_for_path(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown-agent".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_universal_dir_is_under_agents_skills() {
        let root = Path::new("/tmp/workspace");
        assert_eq!(
            project_universal_skills_dir(root),
            PathBuf::from("/tmp/workspace/.agents/skills")
        );
    }

    #[test]
    fn workspace_private_dir_is_workspace_skills() {
        let root = Path::new("/tmp/workspace");
        assert_eq!(
            workspace_private_skills_dir(root),
            PathBuf::from("/tmp/workspace/skills")
        );
    }

    #[test]
    fn agent_scoped_dir_is_nested_under_agents_agents() {
        let root = Path::new("/tmp/workspace");
        assert_eq!(
            agent_scoped_skills_dir(root, "alpha"),
            PathBuf::from("/tmp/workspace/.agents/agents/alpha/skills")
        );
    }

    #[test]
    fn sanitize_agent_name_for_path_replaces_path_traversal_chars() {
        assert_eq!(
            sanitize_agent_name_for_path("my agent/../../etc"),
            "my-agent-------etc"
        );
        assert_eq!(sanitize_agent_name_for_path(""), "unknown-agent");
    }
}
