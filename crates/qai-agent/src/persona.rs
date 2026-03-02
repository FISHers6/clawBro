// quickai-gateway/crates/qai-agent/src/persona.rs
//! AgentPersona: loads per-agent identity configuration from disk.
//! Based on OpenClaw's SOUL.md / IDENTITY.md / MEMORY.md design.
//!
//! Directory layout (user-configured, optional):
//!   <persona_dir>/
//!     SOUL.md      — values, behavior principles
//!     IDENTITY.md  — name, communication style
//!     MEMORY.md    — long-term memory (auto-updated in future)
//!     memory/
//!       {channel}_{scope}.md  — scoped per-session memory (V2)

use std::path::Path;
use qai_protocol::SessionKey;

/// Per-agent persona loaded from a user-configured directory.
/// Missing files result in empty strings (graceful degradation).
pub struct AgentPersona {
    pub soul: String,
    pub identity: String,
    pub memory: String,
}

impl AgentPersona {
    /// Load persona from directory. Files that don't exist produce empty strings.
    pub fn load_from_dir(dir: &Path) -> Self {
        Self {
            soul: read_optional(&dir.join("SOUL.md")),
            identity: read_optional(&dir.join("IDENTITY.md")),
            memory: read_optional(&dir.join("MEMORY.md")),
        }
    }

    /// 新接口：按 session_key 加载 scoped 记忆文件
    /// 路径：{dir}/memory/{channel}_{scope}.md
    pub fn load_from_dir_scoped(dir: &Path, scope: &SessionKey) -> Self {
        let scope_key = format!("{}_{}", scope.channel, scope.scope);
        let memory_path = dir.join("memory").join(format!("{scope_key}.md"));
        Self {
            soul: read_optional(&dir.join("SOUL.md")),
            identity: read_optional(&dir.join("IDENTITY.md")),
            memory: read_optional(&memory_path),
        }
    }

    /// Compose the final system_injection string by combining persona sections
    /// with the gateway-level skills_injection. Returns skills_injection alone
    /// when persona is fully empty.
    pub fn build_system_injection(&self, skills_injection: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if !self.soul.is_empty() {
            parts.push(&self.soul);
        }
        if !self.identity.is_empty() {
            parts.push(&self.identity);
        }
        let mem_section: String;
        if !self.memory.is_empty() {
            mem_section = format!("## 长期记忆\n\n{}", self.memory);
            parts.push(&mem_section);
        }
        if !skills_injection.is_empty() {
            parts.push(skills_injection);
        }
        parts.join("\n\n")
    }

    /// Returns the conventional default persona directory for a named agent:
    /// `~/.quickai/agents/{sanitised_name}/`.
    ///
    /// The name is sanitised to prevent path traversal: only alphanumeric chars,
    /// hyphens, and underscores are kept; everything else is replaced with `-`.
    pub fn default_dir_for(name: &str) -> std::path::PathBuf {
        let safe_name: String = {
            let s: String = name
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                .collect();
            if s.is_empty() { "unknown-agent".to_string() } else { s }
        };
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".quickai")
            .join("agents")
            .join(safe_name)
    }

    /// Ensures the persona directory exists and contains a skeleton SOUL.md.
    /// Idempotent: safe to call if directory already exists.
    pub fn ensure_default_dir(dir: &std::path::Path, agent_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let soul_path = dir.join("SOUL.md");
        if !soul_path.exists() {
            std::fs::write(
                &soul_path,
                format!(
                    "# {name}\n\n\
                     <!-- Describe this agent's personality, values, and behavior principles. -->\n\n\
                     ## 角色定位\n\n\
                     你是一个 AI 助手。\n\n\
                     ## 核心原则\n\n\
                     - 诚实、主动、简洁\n",
                    name = agent_name
                ),
            )?;
        }
        Ok(())
    }

    /// V2: inject both shared group memory and agent private memory with word caps
    pub fn build_system_injection_v2(
        &self,
        skills_injection: &str,
        shared_memory: &str,
        shared_max_words: usize,
        agent_max_words: usize,
    ) -> String {
        use crate::memory::cap_to_words;
        let mut parts: Vec<String> = Vec::new();
        if !self.soul.is_empty() { parts.push(self.soul.clone()); }
        if !self.identity.is_empty() { parts.push(self.identity.clone()); }
        if !shared_memory.is_empty() {
            let capped = cap_to_words(shared_memory, shared_max_words);
            parts.push(format!("## 群组共享记忆\n\n{capped}"));
        }
        if !self.memory.is_empty() {
            let capped = cap_to_words(&self.memory, agent_max_words);
            parts.push(format!("## 长期记忆\n\n{capped}"));
        }
        if !skills_injection.is_empty() { parts.push(skills_injection.to_string()); }
        parts.join("\n\n")
    }
}

fn read_optional(path: &std::path::PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_from_nonexistent_dir_returns_empty() {
        let persona =
            AgentPersona::load_from_dir(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(persona.soul.is_empty());
        assert!(persona.identity.is_empty());
        assert!(persona.memory.is_empty());
    }

    #[test]
    fn test_load_partial_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "You are strict.").unwrap();
        // IDENTITY.md intentionally missing
        std::fs::write(dir.path().join("MEMORY.md"), "Alice likes Rust.").unwrap();

        let persona = AgentPersona::load_from_dir(dir.path());
        assert_eq!(persona.soul, "You are strict.");
        assert!(persona.identity.is_empty());
        assert_eq!(persona.memory, "Alice likes Rust.");
    }

    #[test]
    fn test_load_all_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "soul content").unwrap();
        std::fs::write(dir.path().join("IDENTITY.md"), "identity content").unwrap();
        std::fs::write(dir.path().join("MEMORY.md"), "memory content").unwrap();

        let persona = AgentPersona::load_from_dir(dir.path());
        assert_eq!(persona.soul, "soul content");
        assert_eq!(persona.identity, "identity content");
        assert_eq!(persona.memory, "memory content");
    }

    #[test]
    fn test_build_system_injection_combines_all() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "SOUL").unwrap();
        std::fs::write(dir.path().join("IDENTITY.md"), "IDENTITY").unwrap();
        std::fs::write(dir.path().join("MEMORY.md"), "MEMORY").unwrap();

        let persona = AgentPersona::load_from_dir(dir.path());
        let result = persona.build_system_injection("SKILLS");

        assert!(result.contains("SOUL"));
        assert!(result.contains("IDENTITY"));
        assert!(result.contains("MEMORY"));
        assert!(result.contains("SKILLS"));
        assert!(result.contains("## 长期记忆"));
    }

    #[test]
    fn test_build_system_injection_empty_persona_returns_skills_only() {
        let persona = AgentPersona {
            soul: String::new(),
            identity: String::new(),
            memory: String::new(),
        };
        let result = persona.build_system_injection("skills here");
        assert_eq!(result, "skills here");
    }

    #[test]
    fn test_build_system_injection_no_skills_returns_persona_only() {
        let persona = AgentPersona {
            soul: "soul".to_string(),
            identity: String::new(),
            memory: String::new(),
        };
        let result = persona.build_system_injection("");
        assert_eq!(result, "soul");
    }

    #[test]
    fn test_build_system_injection_both_empty() {
        let persona = AgentPersona {
            soul: String::new(),
            identity: String::new(),
            memory: String::new(),
        };
        let result = persona.build_system_injection("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_load_scoped_memory_uses_scope_file() {
        let dir = tempdir().unwrap();
        let scope = SessionKey::new("dingtalk", "group_C123");
        std::fs::create_dir_all(dir.path().join("memory")).unwrap();
        std::fs::write(
            dir.path().join("memory").join("dingtalk_group_C123.md"),
            "scoped memory content"
        ).unwrap();
        std::fs::write(dir.path().join("MEMORY.md"), "global WRONG content").unwrap();

        let persona = AgentPersona::load_from_dir_scoped(dir.path(), &scope);
        assert_eq!(persona.memory, "scoped memory content");
        assert!(!persona.memory.contains("WRONG"));
    }

    #[test]
    fn test_load_scoped_memory_falls_back_empty_when_no_file() {
        let dir = tempdir().unwrap();
        let scope = SessionKey::new("lark", "user_alice");
        let persona = AgentPersona::load_from_dir_scoped(dir.path(), &scope);
        assert!(persona.memory.is_empty());
    }

    #[test]
    fn test_build_system_injection_v2_includes_shared_and_agent_memory() {
        let persona = AgentPersona {
            soul: "soul".to_string(),
            identity: String::new(),
            memory: "agent_mem".to_string(),
        };
        let result = persona.build_system_injection_v2("skills", "shared_mem", 300, 500);
        assert!(result.contains("soul"));
        assert!(result.contains("shared_mem"));
        assert!(result.contains("agent_mem"));
        assert!(result.contains("群组共享记忆"));
        assert!(result.contains("长期记忆"));
    }

    #[test]
    fn test_default_persona_dir_for_agent_name() {
        let dir = AgentPersona::default_dir_for("claude");
        let expected = dirs::home_dir()
            .unwrap()
            .join(".quickai/agents/claude");
        assert_eq!(dir, expected);
    }

    #[test]
    fn test_default_persona_dir_sanitises_name() {
        // Path traversal characters should be replaced with '-'
        let dir = AgentPersona::default_dir_for("my agent/../../etc");
        let path_str = dir.to_str().unwrap();
        assert!(!path_str.contains(".."));
    }

    #[test]
    fn test_default_persona_dir_empty_name_fallback() {
        let dir = AgentPersona::default_dir_for("");
        // Should not end with "agents/" — must have a non-empty component
        let path_str = dir.to_str().unwrap();
        assert!(!path_str.ends_with("agents/") && !path_str.ends_with("agents"));
        // Should use the "unknown-agent" fallback
        assert!(path_str.ends_with("unknown-agent") || !path_str.ends_with("/"));
    }
}
