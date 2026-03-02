// quickai-gateway/crates/qai-agent/src/prompt_builder.rs
//! SystemPromptBuilder: assembles the 6-layer system prompt in the canonical order
//! defined in docs/人格实现研究.md.

use crate::memory::cap_to_words;
use qai_skills::PersonaSkillData;

/// Assembles the full system prompt for a single agent turn.
///
/// Layer order (persona present):
///   1. IDENTITY block (name, emoji, MBTI label, vibe)
///   2. Cognitive stack (4 Jung functions with position-weighted directive texts)
///   3. soul-injection.md (full persona narrative)
///   4. SOUL.md (operator customization, raw text from persona_dir)
///   5. Shared memory + agent memory (word-capped)
///   6. Skills injection (capability text)
///
/// Without persona (backward compat):
///   SOUL.md → `identity_raw` (raw IDENTITY.md text) → memory → skills.
#[derive(Debug)]
pub struct SystemPromptBuilder<'a> {
    /// Loaded persona skill (type: persona), if any.
    pub persona: Option<&'a PersonaSkillData>,
    /// Raw content of SOUL.md from the persona_dir (operator customization).
    pub soul_md: &'a str,
    /// Raw content of IDENTITY.md from persona_dir (only used when persona == None).
    pub identity_raw: &'a str,
    /// Combined skills capability text (regular skills + persona capability body).
    pub skills_injection: &'a str,
    /// Shared group memory text.
    pub shared_memory: &'a str,
    /// Per-agent memory text.
    pub agent_memory: &'a str,
    pub shared_max_words: usize,
    pub agent_max_words: usize,
}

impl<'a> SystemPromptBuilder<'a> {
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if let Some(persona) = self.persona {
            // ── Layer 1: IDENTITY block ──
            let id = &persona.identity;
            let name_line = match &id.emoji {
                Some(e) => format!("You are {} {}.", id.name, e),
                None    => format!("You are {}.", id.name),
            };
            let mut identity_lines = vec![name_line];
            if let Some(mbti) = &id.mbti_str {
                if let Some(mt) = qai_skills::MbtiType::from_str(mbti) {
                    identity_lines.push(format!("Personality: {} — {}", mbti, mt.label()));
                } else {
                    identity_lines.push(format!("Personality: {}", mbti));
                }
            }
            if let Some(vibe) = &id.vibe {
                identity_lines.push(format!("Core vibe: {}", vibe));
            }
            parts.push(identity_lines.join("\n"));

            // ── Layer 2: Cognitive stack ──
            if let Some(mbti) = persona.mbti_type() {
                parts.push(mbti.build_cognitive_directive());
            }

            // ── Layer 3: soul-injection.md ──
            if !persona.soul_injection.trim().is_empty() {
                parts.push(persona.soul_injection.clone());
            }
        }

        // ── Layer 4: SOUL.md (operator customization) ──
        if !self.soul_md.trim().is_empty() {
            parts.push(self.soul_md.to_string());
        }

        // ── Layer 4b: raw IDENTITY.md (only when no persona — backward compat) ──
        if self.persona.is_none() && !self.identity_raw.trim().is_empty() {
            parts.push(self.identity_raw.to_string());
        }

        // ── Layer 5a: Shared memory ──
        if !self.shared_memory.trim().is_empty() {
            let capped = cap_to_words(self.shared_memory, self.shared_max_words);
            parts.push(format!("## 群组共享记忆\n\n{capped}"));
        }

        // ── Layer 5b: Agent memory ──
        if !self.agent_memory.trim().is_empty() {
            let capped = cap_to_words(self.agent_memory, self.agent_max_words);
            parts.push(format!("## 长期记忆\n\n{capped}"));
        }

        // ── Layer 6: Skills injection ──
        if !self.skills_injection.trim().is_empty() {
            parts.push(self.skills_injection.to_string());
        }

        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_skills::{IdentityData, PersonaSkillData};

    fn make_persona(name: &str, emoji: Option<&str>, mbti: Option<&str>, soul: &str) -> PersonaSkillData {
        PersonaSkillData {
            identity: IdentityData {
                name: name.to_string(),
                emoji: emoji.map(String::from),
                mbti_str: mbti.map(String::from),
                vibe: Some("Sharp and direct.".to_string()),
                avatar_url: None,
                color: None,
            },
            soul_injection: soul.to_string(),
            capability_body: "Rex can do X and Y.".to_string(),
        }
    }

    #[test]
    fn test_no_persona_backward_compat() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "SOUL content",
            identity_raw: "IDENTITY content",
            skills_injection: "SKILLS content",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.contains("SOUL content"));
        assert!(result.contains("IDENTITY content"));
        assert!(result.contains("SKILLS content"));
        // No persona blocks expected
        assert!(!result.contains("Cognitive Architecture"));
    }

    #[test]
    fn test_with_persona_includes_identity_block() {
        let persona = make_persona("Rex", Some("🦅"), Some("INTJ"), "You are Rex.");
        let result = SystemPromptBuilder {
            persona: Some(&persona),
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.contains("You are Rex 🦅."));
        assert!(result.contains("INTJ"));
        assert!(result.contains("Sharp and direct."));
    }

    #[test]
    fn test_with_persona_includes_cognitive_stack() {
        let persona = make_persona("Rex", None, Some("INTJ"), "");
        let result = SystemPromptBuilder {
            persona: Some(&persona),
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.contains("Cognitive Architecture"));
        assert!(result.contains("Dominant"));
        assert!(result.contains("Introverted Intuition"));
    }

    #[test]
    fn test_with_persona_includes_soul_injection() {
        let persona = make_persona("Rex", None, None, "SOUL_INJECTION_TEXT");
        let result = SystemPromptBuilder {
            persona: Some(&persona),
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.contains("SOUL_INJECTION_TEXT"));
    }

    #[test]
    fn test_prompt_order_identity_before_cognitive_before_soul_injection() {
        let persona = make_persona("Rex", Some("🦅"), Some("INTJ"), "SOUL_INJECTION");
        let result = SystemPromptBuilder {
            persona: Some(&persona),
            soul_md: "SOUL_MD",
            identity_raw: "",
            skills_injection: "SKILLS",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        let identity_pos = result.find("You are Rex").unwrap();
        let cognitive_pos = result.find("Cognitive Architecture").unwrap();
        let soul_injection_pos = result.find("SOUL_INJECTION").unwrap();
        let soul_md_pos = result.find("SOUL_MD").unwrap();
        let skills_pos = result.find("SKILLS").unwrap();

        assert!(identity_pos < cognitive_pos, "identity before cognitive");
        assert!(cognitive_pos < soul_injection_pos, "cognitive before soul-injection");
        assert!(soul_injection_pos < soul_md_pos, "soul-injection before SOUL.md");
        assert!(soul_md_pos < skills_pos, "SOUL.md before skills");
    }

    #[test]
    fn test_with_persona_identity_raw_omitted() {
        // When persona is present, raw IDENTITY.md text should NOT appear
        let persona = make_persona("Rex", None, None, "");
        let result = SystemPromptBuilder {
            persona: Some(&persona),
            soul_md: "",
            identity_raw: "RAW_IDENTITY_SHOULD_NOT_APPEAR",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(!result.contains("RAW_IDENTITY_SHOULD_NOT_APPEAR"));
    }

    #[test]
    fn test_memory_sections_included() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "shared mem",
            agent_memory: "agent mem",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.contains("群组共享记忆"));
        assert!(result.contains("shared mem"));
        assert!(result.contains("长期记忆"));
        assert!(result.contains("agent mem"));
    }

    #[test]
    fn test_empty_sections_not_included() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.is_empty());
    }

    #[test]
    fn test_whitespace_only_sections_not_included() {
        // Whitespace-only strings (e.g. trailing newlines from file reads) should be treated as empty.
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "   \n  ",
            identity_raw: "\n",
            skills_injection: "\t",
            shared_memory: "  ",
            agent_memory: "\n\n",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        assert!(result.is_empty(), "whitespace-only inputs should produce an empty prompt");
    }

    #[test]
    fn test_memory_before_skills_ordering() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "SKILLS",
            shared_memory: "SHARED_MEM",
            agent_memory: "AGENT_MEM",
            shared_max_words: 300,
            agent_max_words: 500,
        }.build();

        let shared_pos  = result.find("SHARED_MEM").unwrap();
        let agent_pos   = result.find("AGENT_MEM").unwrap();
        let skills_pos  = result.find("SKILLS").unwrap();

        assert!(shared_pos < skills_pos, "shared memory must appear before skills");
        assert!(agent_pos  < skills_pos, "agent memory must appear before skills");
    }
}
