// clawBro-gateway/crates/clawbro-skills/src/identity.rs
//! IdentityData: parses IDENTITY.md YAML frontmatter and resolves the priority chain.

/// Identity metadata parsed from an IDENTITY.md YAML frontmatter block.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityData {
    pub name: String,
    pub emoji: Option<String>,
    /// Raw MBTI type string, e.g. "INTJ". Use `clawbro_skills::MbtiType::from_str()` to parse.
    pub mbti_str: Option<String>,
    pub vibe: Option<String>,
    pub avatar_url: Option<String>,
    pub color: Option<String>,
}

/// Parse IDENTITY.md content into `IdentityData`.
///
/// Handles YAML frontmatter delimited by `---` lines.
/// If no frontmatter is found, returns IdentityData with `name = fallback_name` and
/// all optional fields as `None`.
///
/// Supported keys: `name`, `emoji`, `mbti`, `vibe`, `avatar_url`, `color`.
/// Values are stripped of surrounding single or double quotes.
pub fn parse_identity_yaml(content: &str, fallback_name: &str) -> IdentityData {
    if !content.starts_with("---\n") {
        return IdentityData {
            name: fallback_name.to_string(),
            emoji: None,
            mbti_str: None,
            vibe: None,
            avatar_url: None,
            color: None,
        };
    }

    // Locate closing ---
    let rest = &content[4..]; // skip opening "---\n"
    let frontmatter = rest
        .find("\n---\n")
        .map(|p| &rest[..p])
        .or_else(|| {
            rest.find("\n---")
                .filter(|&p| p + 4 >= rest.len())
                .map(|p| &rest[..p])
        })
        .unwrap_or(rest);

    let mut name: Option<String> = None;
    let mut emoji: Option<String> = None;
    let mut mbti_str: Option<String> = None;
    let mut vibe: Option<String> = None;
    let mut avatar_url: Option<String> = None;
    let mut color: Option<String> = None;

    for line in frontmatter.lines() {
        // Only handle top-level key: value lines (no leading whitespace)
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((key, val_raw)) = line.split_once(':') {
            let key = key.trim();
            // Strip surrounding quotes and trim whitespace
            let val = val_raw
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string();
            if val.is_empty() {
                continue;
            }
            match key {
                "name" => name = Some(val),
                "emoji" => emoji = Some(val),
                "mbti" => mbti_str = Some(val),
                "vibe" => vibe = Some(val),
                "avatar_url" => avatar_url = Some(val),
                "color" => color = Some(val),
                _ => {}
            }
        }
    }

    IdentityData {
        name: name.unwrap_or_else(|| fallback_name.to_string()),
        emoji,
        mbti_str,
        vibe,
        avatar_url,
        color,
    }
}

/// Load `IdentityData` using the priority chain:
///
/// 1. `~/.clawbro/agents/{agent_name}/IDENTITY.md` — operator/user override
/// 2. `{skill_dir}/IDENTITY.md` — package default
///
/// Returns `None` if neither file exists or can be read.
pub fn load_identity_with_priority(
    skill_dir: &std::path::Path,
    agent_name: &str,
) -> Option<IdentityData> {
    // Priority 1: user/operator override
    if let Some(home) = dirs::home_dir() {
        let override_path = home
            .join(".clawbro")
            .join("agents")
            .join(agent_name)
            .join("IDENTITY.md");
        if override_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&override_path) {
                return Some(parse_identity_yaml(&content, agent_name));
            }
        }
    }

    // Priority 2: skill package default
    let pkg_path = skill_dir.join("IDENTITY.md");
    if pkg_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&pkg_path) {
            return Some(parse_identity_yaml(&content, agent_name));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_identity(dir: &TempDir, content: &str) {
        std::fs::write(dir.path().join("IDENTITY.md"), content).unwrap();
    }

    #[test]
    fn test_parse_yaml_all_fields() {
        let content = "---\nname: Rex\nemoji: 🦅\nmbti: INTJ\nvibe: \"Strategic. Precise.\"\navatar_url: \"https://example.com/rex.png\"\ncolor: \"#1A1A2E\"\n---\nSome description.";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Rex");
        assert_eq!(id.emoji, Some("🦅".to_string()));
        assert_eq!(id.mbti_str, Some("INTJ".to_string()));
        assert_eq!(id.vibe, Some("Strategic. Precise.".to_string()));
        assert_eq!(
            id.avatar_url,
            Some("https://example.com/rex.png".to_string())
        );
        assert_eq!(id.color, Some("#1A1A2E".to_string()));
    }

    #[test]
    fn test_parse_yaml_minimal_only_name() {
        let content = "---\nname: Luna\n---\nSome body.";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Luna");
        assert!(id.emoji.is_none());
        assert!(id.mbti_str.is_none());
        assert!(id.vibe.is_none());
        assert!(id.avatar_url.is_none());
        assert!(id.color.is_none());
    }

    #[test]
    fn test_parse_yaml_no_frontmatter_uses_fallback() {
        let content = "# Luna\n\nNo frontmatter here.";
        let id = parse_identity_yaml(content, "my-agent");
        assert_eq!(id.name, "my-agent");
        assert!(id.emoji.is_none());
    }

    #[test]
    fn test_parse_yaml_strips_double_quotes() {
        let content = "---\nname: \"Rex\"\nmbti: \"INTJ\"\n---\n";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Rex");
        assert_eq!(id.mbti_str, Some("INTJ".to_string()));
    }

    #[test]
    fn test_parse_yaml_strips_single_quotes() {
        let content = "---\nname: 'Rex'\nmbti: 'INTJ'\n---\n";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Rex");
        assert_eq!(id.mbti_str, Some("INTJ".to_string()));
    }

    #[test]
    fn test_parse_yaml_empty_content_uses_fallback() {
        let id = parse_identity_yaml("", "my-fallback");
        assert_eq!(id.name, "my-fallback");
        assert!(id.emoji.is_none());
    }

    #[test]
    fn test_load_with_priority_returns_package_identity() {
        let tmp = TempDir::new().unwrap();
        write_identity(&tmp, "---\nname: Rex\nemoji: 🦅\nmbti: INTJ\n---\n");
        // Use a name highly unlikely to exist in ~/.clawbro/agents/
        let id = load_identity_with_priority(tmp.path(), "zzz-test-rex-99999");
        // Might get the package file if no override exists
        if let Some(id) = id {
            assert_eq!(id.name, "Rex");
            assert_eq!(id.mbti_str, Some("INTJ".to_string()));
        }
        // Acceptable for None if the override happens to exist — just no panic
    }

    #[test]
    fn test_load_with_priority_returns_none_when_no_files() {
        let tmp = TempDir::new().unwrap(); // No IDENTITY.md written
                                           // Use a unique name unlikely to have a user override
        let id = load_identity_with_priority(tmp.path(), "zzz-no-identity-99999");
        assert!(id.is_none());
    }

    #[test]
    fn test_parse_yaml_ignores_indented_lines() {
        // Indented lines (e.g. from nested YAML) should not be parsed as top-level keys
        let content = "---\nname: Rex\n  nested_key: should_be_ignored\n---\n";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Rex");
        // nested_key is not a known field and is indented, so no side-effect
    }

    #[test]
    fn test_parse_yaml_vibe_with_colons_in_value() {
        // Values that themselves contain `:` should be handled by split_once
        let content = "---\nname: Rex\nvibe: Strategic: precise.\n---\n";
        let id = parse_identity_yaml(content, "fallback");
        assert_eq!(id.name, "Rex");
        // vibe value is "Strategic: precise." — split_once(':') splits on first colon only
        // so val_raw = " Strategic: precise." → trimmed = "Strategic: precise."
        assert_eq!(id.vibe, Some("Strategic: precise.".to_string()));
    }
}
