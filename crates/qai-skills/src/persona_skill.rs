// quickai-gateway/crates/qai-skills/src/persona_skill.rs

use crate::identity::IdentityData;
use crate::mbti::MbtiType;

/// Loaded persona skill data from a `type: persona` SKILL.md package.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaSkillData {
    /// Identity metadata from IDENTITY.md (with priority chain applied).
    pub identity: IdentityData,
    /// Full text of soul-injection.md (the narrative persona layer).
    pub soul_injection: String,
    /// SKILL.md body (capability instructions, injected as a skill).
    pub capability_body: String,
}

impl PersonaSkillData {
    /// IM display prefix, e.g. `"[Rex 🦅]: "` or `"[@Rex]: "` (no emoji fallback).
    pub fn display_prefix(&self) -> String {
        match &self.identity.emoji {
            Some(e) => format!("[{} {}]: ", self.identity.name, e),
            None => format!("[@{}]: ", self.identity.name),
        }
    }

    /// Parse the MBTI type from the identity's mbti_str field. Returns None if unset or unrecognised.
    pub fn mbti_type(&self) -> Option<MbtiType> {
        self.identity
            .mbti_str
            .as_deref()
            .and_then(MbtiType::from_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityData;

    fn make_identity(name: &str, emoji: Option<&str>, mbti: Option<&str>) -> IdentityData {
        IdentityData {
            name: name.to_string(),
            emoji: emoji.map(String::from),
            mbti_str: mbti.map(String::from),
            vibe: None,
            avatar_url: None,
            color: None,
        }
    }

    #[test]
    fn test_display_prefix_with_emoji() {
        let data = PersonaSkillData {
            identity: make_identity("Rex", Some("🦅"), Some("INTJ")),
            soul_injection: String::new(),
            capability_body: String::new(),
        };
        assert_eq!(data.display_prefix(), "[Rex 🦅]: ");
    }

    #[test]
    fn test_display_prefix_without_emoji() {
        let data = PersonaSkillData {
            identity: make_identity("Rex", None, Some("INTJ")),
            soul_injection: String::new(),
            capability_body: String::new(),
        };
        assert_eq!(data.display_prefix(), "[@Rex]: ");
    }

    #[test]
    fn test_mbti_type_parses_correctly() {
        let data = PersonaSkillData {
            identity: make_identity("Rex", None, Some("INTJ")),
            soul_injection: String::new(),
            capability_body: String::new(),
        };
        assert_eq!(data.mbti_type(), Some(crate::mbti::MbtiType::Intj));
    }

    #[test]
    fn test_mbti_type_none_when_no_mbti() {
        let data = PersonaSkillData {
            identity: make_identity("Rex", None, None),
            soul_injection: String::new(),
            capability_body: String::new(),
        };
        assert!(data.mbti_type().is_none());
    }

    #[test]
    fn test_mbti_type_none_when_invalid_mbti() {
        let data = PersonaSkillData {
            identity: make_identity("Rex", None, Some("INVALID")),
            soul_injection: String::new(),
            capability_body: String::new(),
        };
        assert!(data.mbti_type().is_none());
    }
}
