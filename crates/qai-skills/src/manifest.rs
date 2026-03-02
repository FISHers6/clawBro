use serde::{Deserialize, Serialize};

/// openclaw.plugin.json / quickai.plugin.json 清单格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// skill.md 相对路径（技能指令文本）
    #[serde(default = "default_skill_md")]
    pub skill_md: String,
    /// 此 skill 提供的额外工具列表（可选）
    #[serde(default)]
    pub tools: Vec<String>,
    /// Whether this skill was explicitly trusted by the user.
    /// Skills installed from external sources default to untrusted.
    #[serde(default)]
    pub trusted: Option<bool>,
}

fn default_skill_md() -> String {
    "skill.md".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_deserialize_minimal() {
        let json = r#"{"id":"test","name":"Test","version":"1.0.0"}"#;
        let m: SkillManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "test");
        assert_eq!(m.skill_md, "skill.md");
        assert!(m.tools.is_empty());
    }

    #[test]
    fn test_manifest_deserialize_full() {
        let json = r#"{"id":"coding","name":"Coding","version":"2.0","description":"Help code","skill_md":"prompt.md","tools":["bash","read"]}"#;
        let m: SkillManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.skill_md, "prompt.md");
        assert_eq!(m.tools.len(), 2);
    }

    #[test]
    fn test_manifest_trusted_defaults_to_none() {
        let json = r#"{"id":"s","name":"S","version":"1.0.0"}"#;
        let m: SkillManifest = serde_json::from_str(json).unwrap();
        assert!(m.trusted.is_none());
    }

    #[test]
    fn test_manifest_trusted_explicit_true() {
        let json = r#"{"id":"s","name":"S","version":"1.0.0","trusted":true}"#;
        let m: SkillManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.trusted, Some(true));
    }

    #[test]
    fn test_manifest_trusted_explicit_false() {
        let json = r#"{"id":"s","name":"S","version":"1.0.0","trusted":false}"#;
        let m: SkillManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.trusted, Some(false));
    }
}
