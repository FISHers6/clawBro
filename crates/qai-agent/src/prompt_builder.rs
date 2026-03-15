// quickai-gateway/crates/qai-agent/src/prompt_builder.rs
//! SystemPromptBuilder: assembles the 7-layer system prompt in the canonical order
//! defined in docs/人格实现研究.md.

use crate::memory::cap_to_words;
use crate::traits::AgentRole;
use qai_skills::PersonaSkillData;

/// System-level protocol guidance injected into Lead agent turns only.
/// Explains how the Lead should handle [团队通知] milestone messages from TeamNotify.
const TEAM_NOTIFY_PROTOCOL: &str = "\
## TeamNotify 协议（系统内部通知）

当你收到以 `[团队通知]` 开头的消息时，这是来自系统的内部协调信号，不是用户消息。

**重要：你对 `[团队通知]` 的文字回复不会发给用户。**
要向用户发送信息，必须调用 `post_update` 工具。
如果没有新信息需要告知用户，直接结束回复即可，不需要调用任何工具。

**收到任务完成通知时：**
- 如果显示「所有任务已完成」：调用 `post_update` 工具生成最终汇总，向用户报告成果
- 如果显示部分完成：无需立即响应，等待所有任务完成

**收到任务提交待验收通知时：**
- 检查结果后调用 `accept_task(task_id)` 验收，或
- 调用 `reopen_task(task_id, reason)` 退回给 Specialist 修正
- 这一轮不要直接调用 `post_update` 给用户发送最终汇总；最终结果只在收到“已验收 / 所有任务现已完成”后再发

**收到检查点或求助通知时：**
- 可直接继续观察，不一定要立即发给用户
- 如对用户可见，调用 `post_update`
- 如需要调整执行，调用 `assign_task`

**收到任务阻塞通知时：**
- 调用 `assign_task(task_id, new_assignee)` 重新分配，或
- 调用 `post_update` 向用户说明情况

**收到任务永久失败通知时：**
- 调用 `post_update` 向用户报告失败，并说明原因

**不要**将 `[团队通知]` 内容直接转发给用户——总结后再通过 `post_update` 发送。";

/// Conversation continuity contract injected for all runtime-backed turns.
/// Makes session continuity an explicit system invariant instead of relying on
/// model defaults.
const CONVERSATION_CONTINUITY_PROTOCOL: &str = "\
## 会话连续性协议（系统保证）

你正在一个持久会话中工作。系统已经把最近的真实对话历史提供给你。

- 当用户询问“我刚才说了什么”“上一条消息是什么”“延续刚才的话题”时，必须只基于系统显式提供的用户可见聊天历史回答。
- 只要上下文里存在先前消息，就不要声称“无法查看历史”“只能看到当前消息”。
- 不要把 AGENTS.md、CLAUDE.md、skills、环境上下文、系统提示、开发者提示、工具结果、session bootstrap 内容当作“上一条用户消息”或“用户历史”。
- 如果系统显式提供的用户可见聊天历史为空，就要明确说明当前会话里没有可引用的先前用户消息，而不是猜测或引用内部上下文。
- 如果历史里存在多个候选消息，应优先回答离当前用户消息最近的一条相关用户输入。";

/// Assembles the full system prompt for a single agent turn.
///
/// Layer order (persona present):
///   0. Task reminder（最高优先级，仅 Lead/Specialist 有任务时注入）
///   1. IDENTITY block (name, emoji, MBTI label, vibe)
///   2. Cognitive stack (4 Jung functions with position-weighted directive texts)
///   3. soul-injection.md (full persona narrative)
///   4. SOUL.md (operator customization, raw text from persona_dir)
///      Team manifest（TEAM.md，Lead/Specialist 模式）
///   5. Shared memory（Solo/Lead: 群组历史摘要；Specialist: CONTEXT.md 任务背景）
///      Agent memory（仅 Solo/Lead；Specialist 跳过，Ralph Loop 核心）
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
    /// Solo/Lead: FileMemoryStore 群组历史摘要；Specialist: CONTEXT.md 任务背景
    pub shared_memory: &'a str,
    /// Per-agent memory text（Specialist 模式下忽略，不注入）
    pub agent_memory: &'a str,
    pub shared_max_words: usize,
    pub agent_max_words: usize,
    /// Agent 在团队中的角色（控制 memory 注入行为）
    pub agent_role: AgentRole,
    /// Layer 0 任务提醒（最高优先级，覆盖一切；None 时跳过）
    pub task_reminder: Option<&'a str>,
    /// TEAM.md 内容（团队职责说明，Lead/Specialist 有效）
    pub team_manifest: Option<&'a str>,
}

impl<'a> SystemPromptBuilder<'a> {
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // ── Layer 0: Task Reminder（最高优先级，仅 Lead/Specialist 有任务时注入）──
        if let Some(reminder) = self.task_reminder {
            if !reminder.trim().is_empty() {
                parts.push(format!(
                    "══════ 当前任务（自动注入，最高优先级）══════\n{}\n══════════════════════════════════════════",
                    reminder
                ));
            }
        }

        if let Some(persona) = self.persona {
            // ── Layer 1: IDENTITY block ──
            let id = &persona.identity;
            let name_line = match &id.emoji {
                Some(e) => format!("You are {} {}.", id.name, e),
                None => format!("You are {}.", id.name),
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

        // ── Layer 4c: TEAM.md（Lead/Specialist 模式下的团队职责说明）──
        if let Some(manifest) = self.team_manifest {
            if !manifest.trim().is_empty() {
                parts.push(format!("## 团队职责\n\n{}", manifest));
            }
        }

        // ── Layer 5a: Shared memory ──
        // Solo/Lead → 群组历史摘要；Specialist → CONTEXT.md 任务背景（调用方负责传入正确内容）
        if !self.shared_memory.trim().is_empty() {
            let capped = cap_to_words(self.shared_memory, self.shared_max_words);
            let label = match self.agent_role {
                AgentRole::Specialist => "## 任务背景（团队上下文）",
                _ => "## 群组共享记忆",
            };
            parts.push(format!("{label}\n\n{capped}"));
        }

        // ── Layer 5b: Agent memory（仅 Solo/Lead；Specialist 跳过 ← Ralph Loop 核心）──
        if !matches!(self.agent_role, AgentRole::Specialist) && !self.agent_memory.trim().is_empty()
        {
            let capped = cap_to_words(self.agent_memory, self.agent_max_words);
            parts.push(format!("## 长期记忆\n\n{capped}"));
        }

        // ── Layer 6: Skills injection ──
        if !self.skills_injection.trim().is_empty() {
            parts.push(self.skills_injection.to_string());
        }

        // ── Layer 7: TeamNotify protocol（仅 Lead 模式注入）──
        if matches!(self.agent_role, AgentRole::Lead) {
            parts.push(TEAM_NOTIFY_PROTOCOL.to_string());
        }

        // ── Layer 8: conversation continuity contract（所有模式）──
        parts.push(CONVERSATION_CONTINUITY_PROTOCOL.to_string());

        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_skills::{IdentityData, PersonaSkillData};

    fn make_persona(
        name: &str,
        emoji: Option<&str>,
        mbti: Option<&str>,
        soul: &str,
    ) -> PersonaSkillData {
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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(result.contains("SOUL content"));
        assert!(result.contains("IDENTITY content"));
        assert!(result.contains("SKILLS content"));
        assert!(result.contains("会话连续性协议"));
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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        let identity_pos = result.find("You are Rex").unwrap();
        let cognitive_pos = result.find("Cognitive Architecture").unwrap();
        let soul_injection_pos = result.find("SOUL_INJECTION").unwrap();
        let soul_md_pos = result.find("SOUL_MD").unwrap();
        let skills_pos = result.find("SKILLS").unwrap();

        assert!(identity_pos < cognitive_pos, "identity before cognitive");
        assert!(
            cognitive_pos < soul_injection_pos,
            "cognitive before soul-injection"
        );
        assert!(
            soul_injection_pos < soul_md_pos,
            "soul-injection before SOUL.md"
        );
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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(result.contains("会话连续性协议"));
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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(
            result.contains("会话连续性协议"),
            "whitespace-only inputs should still include the continuity contract"
        );
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
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        let shared_pos = result.find("SHARED_MEM").unwrap();
        let agent_pos = result.find("AGENT_MEM").unwrap();
        let skills_pos = result.find("SKILLS").unwrap();

        assert!(
            shared_pos < skills_pos,
            "shared memory must appear before skills"
        );
        assert!(
            agent_pos < skills_pos,
            "agent memory must appear before skills"
        );
    }

    #[test]
    fn test_specialist_excludes_agent_memory() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "soul content",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "secret project memory",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: AgentRole::Specialist,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(
            !result.contains("secret project memory"),
            "Specialist should NOT see MEMORY.md"
        );
        assert!(
            result.contains("soul content"),
            "Specialist always sees SOUL.md"
        );
    }

    #[test]
    fn test_specialist_shared_memory_label() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "task context background",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: AgentRole::Specialist,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(
            result.contains("任务背景"),
            "Specialist shared_memory label should be 任务背景"
        );
        assert!(
            !result.contains("群组共享记忆"),
            "Specialist must NOT see 群组共享记忆"
        );
    }

    #[test]
    fn test_task_reminder_appears_first() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "soul content",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: AgentRole::Specialist,
            task_reminder: Some("URGENT: T003 implement JWT"),
            team_manifest: None,
        }
        .build();

        let reminder_pos = result.find("URGENT: T003").unwrap();
        let soul_pos = result.find("soul content").unwrap();
        assert!(
            reminder_pos < soul_pos,
            "task_reminder must appear before SOUL.md"
        );
    }

    #[test]
    fn test_solo_includes_agent_memory() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "long term memory",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: AgentRole::Solo,
            task_reminder: None,
            team_manifest: None,
        }
        .build();

        assert!(
            result.contains("long term memory"),
            "Solo MUST see MEMORY.md"
        );
        assert!(result.contains("长期记忆"));
    }

    #[test]
    fn test_team_manifest_injected() {
        let result = SystemPromptBuilder {
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            shared_max_words: 300,
            agent_max_words: 500,
            agent_role: AgentRole::Lead,
            task_reminder: None,
            team_manifest: Some("Claude: Lead\nCodex: Specialist"),
        }
        .build();

        assert!(result.contains("团队职责"));
        assert!(result.contains("Claude: Lead"));
    }

    #[test]
    fn test_lead_prompt_includes_team_notify_protocol() {
        let builder = SystemPromptBuilder {
            agent_role: AgentRole::Lead,
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            task_reminder: None,
            team_manifest: None,
            shared_max_words: 300,
            agent_max_words: 500,
        };
        let prompt = builder.build();
        assert!(
            prompt.contains("TeamNotify"),
            "Lead prompt must include TeamNotify protocol"
        );
        assert!(
            prompt.contains("post_update"),
            "Lead prompt must mention post_update tool"
        );
        assert!(
            prompt.contains("这一轮不要直接调用 `post_update`"),
            "Lead prompt must forbid final post_update during submitted review"
        );
    }

    #[test]
    fn test_solo_prompt_excludes_team_notify_protocol() {
        let builder = SystemPromptBuilder {
            agent_role: AgentRole::Solo,
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            task_reminder: None,
            team_manifest: None,
            shared_max_words: 300,
            agent_max_words: 500,
        };
        let prompt = builder.build();
        assert!(
            !prompt.contains("TeamNotify"),
            "Solo prompt must NOT include TeamNotify protocol"
        );
    }

    #[test]
    fn test_specialist_prompt_excludes_team_notify_protocol() {
        let builder = SystemPromptBuilder {
            agent_role: AgentRole::Specialist,
            persona: None,
            soul_md: "",
            identity_raw: "",
            skills_injection: "",
            shared_memory: "",
            agent_memory: "",
            task_reminder: None,
            team_manifest: None,
            shared_max_words: 300,
            agent_max_words: 500,
        };
        let prompt = builder.build();
        assert!(
            !prompt.contains("TeamNotify"),
            "Specialist prompt must NOT include TeamNotify protocol"
        );
    }
}
