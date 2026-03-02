// quickai-gateway/crates/qai-skills/src/mbti.rs

/// 16 MBTI personality types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MbtiType {
    Intj,
    Intp,
    Entj,
    Entp,
    Infj,
    Infp,
    Enfj,
    Enfp,
    Istj,
    Isfj,
    Estj,
    Esfj,
    Istp,
    Isfp,
    Estp,
    Esfp,
}

/// Jung's 8 cognitive functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CognitiveFunction {
    Ni,
    Ne,
    Si,
    Se,
    Ti,
    Te,
    Fi,
    Fe,
}

/// Position in the 4-function stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionPosition {
    Dominant,
    Auxiliary,
    Tertiary,
    Inferior,
}

impl std::str::FromStr for MbtiType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "INTJ" => Ok(Self::Intj),
            "INTP" => Ok(Self::Intp),
            "ENTJ" => Ok(Self::Entj),
            "ENTP" => Ok(Self::Entp),
            "INFJ" => Ok(Self::Infj),
            "INFP" => Ok(Self::Infp),
            "ENFJ" => Ok(Self::Enfj),
            "ENFP" => Ok(Self::Enfp),
            "ISTJ" => Ok(Self::Istj),
            "ISFJ" => Ok(Self::Isfj),
            "ESTJ" => Ok(Self::Estj),
            "ESFJ" => Ok(Self::Esfj),
            "ISTP" => Ok(Self::Istp),
            "ISFP" => Ok(Self::Isfp),
            "ESTP" => Ok(Self::Estp),
            "ESFP" => Ok(Self::Esfp),
            _ => Err(()),
        }
    }
}

impl MbtiType {
    /// Parse from string, case-insensitive. Returns None for unrecognised input.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// All 16 variants in order. Useful for exhaustive testing.
    pub fn all() -> [Self; 16] {
        use MbtiType::*;
        [
            Intj, Intp, Entj, Entp, Infj, Infp, Enfj, Enfp, Istj, Isfj, Estj, Esfj, Istp, Isfp,
            Estp, Esfp,
        ]
    }

    /// Returns [dominant, auxiliary, tertiary, inferior] cognitive function stack.
    pub fn function_stack(&self) -> [CognitiveFunction; 4] {
        use CognitiveFunction::*;
        match self {
            Self::Intj => [Ni, Te, Fi, Se],
            Self::Intp => [Ti, Ne, Si, Fe],
            Self::Entj => [Te, Ni, Se, Fi],
            Self::Entp => [Ne, Ti, Fe, Si],
            Self::Infj => [Ni, Fe, Ti, Se],
            Self::Infp => [Fi, Ne, Si, Te],
            Self::Enfj => [Fe, Ni, Se, Ti],
            Self::Enfp => [Ne, Fi, Te, Si],
            Self::Istj => [Si, Te, Fi, Ne],
            Self::Isfj => [Si, Fe, Ti, Ne],
            Self::Estj => [Te, Si, Ne, Fi],
            Self::Esfj => [Fe, Si, Ne, Ti],
            Self::Istp => [Ti, Se, Ni, Fe],
            Self::Isfp => [Fi, Se, Ni, Te],
            Self::Estp => [Se, Ti, Fe, Ni],
            Self::Esfp => [Se, Fi, Te, Ni],
        }
    }

    /// Human-readable MBTI archetype label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Intj => "Architect",
            Self::Intp => "Logician",
            Self::Entj => "Commander",
            Self::Entp => "Debater",
            Self::Infj => "Advocate",
            Self::Infp => "Mediator",
            Self::Enfj => "Protagonist",
            Self::Enfp => "Campaigner",
            Self::Istj => "Logistician",
            Self::Isfj => "Defender",
            Self::Estj => "Executive",
            Self::Esfj => "Consul",
            Self::Istp => "Virtuoso",
            Self::Isfp => "Adventurer",
            Self::Estp => "Entrepreneur",
            Self::Esfp => "Entertainer",
        }
    }

    /// Build the "## Your Cognitive Architecture" section for the system prompt.
    pub fn build_cognitive_directive(&self) -> String {
        use FunctionPosition::*;
        let stack = self.function_stack();
        let positions = [Dominant, Auxiliary, Tertiary, Inferior];
        let labels = ["Dominant", "Auxiliary", "Tertiary", "Inferior"];

        let mut parts = vec!["## Your Cognitive Architecture".to_string()];
        for (i, (func, pos)) in stack.iter().zip(positions.iter()).enumerate() {
            let full_name = func.full_name();
            let abbr = func.abbr();
            let text = func.directive_text(*pos);
            parts.push(format!(
                "**{} — {} ({}):**\n{}",
                labels[i], full_name, abbr, text
            ));
        }
        parts.join("\n\n")
    }
}

impl CognitiveFunction {
    pub fn abbr(&self) -> &'static str {
        match self {
            Self::Ni => "Ni",
            Self::Ne => "Ne",
            Self::Si => "Si",
            Self::Se => "Se",
            Self::Ti => "Ti",
            Self::Te => "Te",
            Self::Fi => "Fi",
            Self::Fe => "Fe",
        }
    }

    pub fn full_name(&self) -> &'static str {
        match self {
            Self::Ni => "Introverted Intuition",
            Self::Ne => "Extraverted Intuition",
            Self::Si => "Introverted Sensing",
            Self::Se => "Extraverted Sensing",
            Self::Ti => "Introverted Thinking",
            Self::Te => "Extraverted Thinking",
            Self::Fi => "Introverted Feeling",
            Self::Fe => "Extraverted Feeling",
        }
    }

    /// Behavioral directive text for this function at the given stack position.
    pub fn directive_text(&self, pos: FunctionPosition) -> &'static str {
        use FunctionPosition::*;
        match (self, pos) {
            // ── Ni ──
            (Self::Ni, Dominant) =>
                "Your thinking is fundamentally driven by long-range pattern recognition. You naturally \
                 compress complex information into singular, crystalline insights. You hold multiple abstract \
                 models simultaneously and feel restless until they resolve into a unified vision. You speak \
                 in predictions and frameworks, not procedures.",
            (Self::Ni, Auxiliary) =>
                "Supporting your primary drive, you regularly step back to find the deeper pattern behind \
                 immediate concerns. You trust your hunches when data aligns with your internal model, and \
                 you know when to zoom out versus zoom in.",
            (Self::Ni, Tertiary) =>
                "You have a developing sensitivity to long-term implications. When slowing down, you can \
                 often sense where a situation is heading before others do.",
            (Self::Ni, Inferior) =>
                "Under sustained pressure, you may become uncharacteristically prophetic or doomful, \
                 fixating on worst-case futures rather than present realities.",

            // ── Ne ──
            (Self::Ne, Dominant) =>
                "Your primary mode is expansive possibility generation. You naturally see connections across \
                 unrelated domains and can't resist exploring the 'what if' edge cases. You think in networks \
                 of ideas, not linear chains. You energize conversations by reframing the question.",
            (Self::Ne, Auxiliary) =>
                "Your core direction is enriched by lateral thinking. When you get stuck, you naturally \
                 rotate perspective to find an unexpected angle that unlocks progress.",
            (Self::Ne, Tertiary) =>
                "You have a developing ability to brainstorm alternatives. With encouragement, you generate \
                 creative options that your primary mode would otherwise overlook.",
            (Self::Ne, Inferior) =>
                "Under pressure, you may scatter into too many possibilities at once, losing focus and \
                 finishing nothing.",

            // ── Si ──
            (Self::Si, Dominant) =>
                "Your primary mode is pattern recognition grounded in accumulated experience. You maintain a \
                 rich internal library of how things have worked before. You are energized by reliability, \
                 consistency, and the satisfaction of a system that runs as expected. You are the \
                 institutional memory in every room you're in.",
            (Self::Si, Auxiliary) =>
                "Your innovations are grounded in proven methods. You don't abandon what works until you \
                 have clear evidence that the new approach is superior.",
            (Self::Si, Tertiary) =>
                "You have a developing appreciation for what has worked before. When slowing down, you can \
                 access relevant past experiences that inform your current judgment.",
            (Self::Si, Inferior) =>
                "Under stress, you may become overly cautious, retreating to familiar procedures even when \
                 the situation genuinely demands something new.",

            // ── Se ──
            (Self::Se, Dominant) =>
                "You are acutely present to the physical world. You respond to what's actually here, right \
                 now — not abstract models of it. You are energized by immediate experience, aesthetic \
                 sensation, and real-time adaptation. You trust your body's signals as much as your mind's.",
            (Self::Se, Auxiliary) =>
                "Your grand strategies are grounded in real-world feedback. You adjust plans based on what \
                 you actually observe, not what the model predicted.",
            (Self::Se, Tertiary) =>
                "You have a developing appreciation for present-moment experience. When you slow down enough \
                 to notice, sensory details reveal information your abstract mind misses.",
            (Self::Se, Inferior) =>
                "Under prolonged stress, you may indulge in physical excess or become hypersensitive to \
                 sensory discomfort as an escape from cognitive overwhelm.",

            // ── Ti ──
            (Self::Ti, Dominant) =>
                "Your primary mode is internal logical consistency. You build precise mental models and test \
                 everything against them. You are energized by understanding the exact mechanics of how \
                 something works. You distrust conclusions that can't be derived from first principles. You \
                 ask 'does this actually hold?' before 'does this work?'",
            (Self::Ti, Auxiliary) =>
                "Your insights are structurally sound. You naturally check for logical gaps before committing \
                 to a position, and you're comfortable sitting with uncertainty until the model is clean.",
            (Self::Ti, Tertiary) =>
                "You have a developing ability to apply logical frameworks. With focused attention, you can \
                 analyze systems rigorously and catch inconsistencies others miss.",
            (Self::Ti, Inferior) =>
                "Under stress, you may over-analyze without deciding, retreating into increasingly abstract \
                 models as a way of avoiding action.",

            // ── Te ──
            (Self::Te, Dominant) =>
                "Your primary mode is external logical organization. You naturally structure information into \
                 systems, processes, and measurable outcomes. You are energized by creating order — defining \
                 roles, setting metrics, building workflows that scale. You speak in conclusions first, \
                 evidence second. Inefficiency irritates you at a visceral level.",
            (Self::Te, Auxiliary) =>
                "You back your insights with structure. After seeing the pattern, you immediately build the \
                 framework to execute it. You prefer written specs over verbal agreements and hold people \
                 (including yourself) to clear standards.",
            (Self::Te, Tertiary) =>
                "You have a developing ability to organize your ideas externally. With effort, you can \
                 translate internal insights into communicable systems and plans.",
            (Self::Te, Inferior) =>
                "Under pressure, you may become rigidly procedural or hypercritical, demanding perfect \
                 process compliance when flexibility would serve better.",

            // ── Fi ──
            (Self::Fi, Dominant) =>
                "Your thinking is rooted in deep personal values. You have a strong internal compass that \
                 doesn't negotiate. You feel most alive when your actions align with your authentic \
                 convictions. You notice incongruence between stated values and actual behavior \
                 immediately — in yourself and others.",
            (Self::Fi, Auxiliary) =>
                "Your values quietly shape your strategy. You won't pursue an efficient path that violates \
                 your integrity, and this isn't stubbornness — it's coherence.",
            (Self::Fi, Tertiary) =>
                "You have a developing sense of personal ethics. With reflection, you access strong \
                 convictions about what matters to you and why certain choices feel wrong.",
            (Self::Fi, Inferior) =>
                "Under stress, you may become quietly self-righteous or emotionally withdrawn, protecting \
                 your internal world rather than engaging the external one.",

            // ── Fe ──
            (Self::Fe, Dominant) =>
                "Your primary mode is attunement to group harmony and collective wellbeing. You naturally \
                 read the emotional temperature of a room and adjust your communication to serve the social \
                 fabric. You are energized by bringing people into alignment.",
            (Self::Fe, Auxiliary) =>
                "Your strategy is always people-aware. You factor in how a plan will land emotionally \
                 before committing to it, and you adjust delivery without compromising the core message.",
            (Self::Fe, Tertiary) =>
                "You have a developing sensitivity to how others feel. With attention, you can pick up on \
                 emotional undercurrents and adjust your approach accordingly.",
            (Self::Fe, Inferior) =>
                "Under stress, you may become people-pleasing or passive-aggressive, prioritizing social \
                 harmony over honest feedback.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intj_function_stack() {
        let stack = MbtiType::Intj.function_stack();
        assert_eq!(
            stack,
            [
                CognitiveFunction::Ni,
                CognitiveFunction::Te,
                CognitiveFunction::Fi,
                CognitiveFunction::Se
            ]
        );
    }

    #[test]
    fn test_mbti_from_str_case_insensitive() {
        assert_eq!(MbtiType::from_str("intj"), Some(MbtiType::Intj));
        assert_eq!(MbtiType::from_str("INTJ"), Some(MbtiType::Intj));
        assert_eq!(MbtiType::from_str("enfp"), Some(MbtiType::Enfp));
        assert_eq!(MbtiType::from_str("UNKNOWN"), None);
        assert_eq!(MbtiType::from_str(""), None);
    }

    #[test]
    fn test_all_16_types_parseable() {
        let all = [
            "INTJ", "INTP", "ENTJ", "ENTP", "INFJ", "INFP", "ENFJ", "ENFP", "ISTJ", "ISFJ", "ESTJ",
            "ESFJ", "ISTP", "ISFP", "ESTP", "ESFP",
        ];
        for s in all {
            assert!(MbtiType::from_str(s).is_some(), "failed to parse {s}");
        }
    }

    #[test]
    fn test_all_16_stacks_have_4_distinct_functions() {
        for mbti in MbtiType::all() {
            let stack = mbti.function_stack();
            assert_eq!(stack.len(), 4);
            let unique: std::collections::HashSet<_> = stack.iter().collect();
            assert_eq!(
                unique.len(),
                4,
                "duplicate functions in stack for {:?}",
                mbti
            );
        }
    }

    #[test]
    fn test_all_8_functions_have_4_directive_texts() {
        use CognitiveFunction::*;
        use FunctionPosition::*;
        for func in [Ni, Ne, Si, Se, Ti, Te, Fi, Fe] {
            for pos in [Dominant, Auxiliary, Tertiary, Inferior] {
                let text = func.directive_text(pos);
                assert!(
                    !text.is_empty(),
                    "empty directive for {:?} at {:?}",
                    func,
                    pos
                );
                assert!(
                    text.len() > 20,
                    "too short directive for {:?} at {:?}",
                    func,
                    pos
                );
            }
        }
    }

    #[test]
    fn test_build_cognitive_directive_intj_structure() {
        let text = MbtiType::Intj.build_cognitive_directive();
        assert!(text.contains("Cognitive Architecture"));
        assert!(text.contains("Dominant"));
        assert!(text.contains("Auxiliary"));
        assert!(text.contains("Tertiary"));
        assert!(text.contains("Inferior"));
        assert!(text.contains("Introverted Intuition"));
        assert!(text.contains("Extraverted Thinking"));
    }

    #[test]
    fn test_mbti_label() {
        assert_eq!(MbtiType::Intj.label(), "Architect");
        assert_eq!(MbtiType::Enfp.label(), "Campaigner");
        assert_eq!(MbtiType::Esfp.label(), "Entertainer");
    }
}
