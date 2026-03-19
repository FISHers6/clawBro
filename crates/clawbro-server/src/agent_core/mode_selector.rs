//! ModeSelector — 关键词触发的 Solo→Team 自动升级
//!
//! 当群组配置了 `auto_promote = true` 时，
//! 检测用户消息是否包含团队协作触发关键词。
//! 触发时将当前消息按 Lead 角色处理。

const TEAM_TRIGGER_KEYWORDS: &[&str] = &[
    "组建团队",
    "多agent",
    "multi-agent",
    "team mode",
    "并行执行",
    "swarm",
    "团队协作",
    "分配任务",
    "协同完成",
];

const TEAM_DELEGATION_KEYWORDS: &[&str] = &[
    "分配任务",
    "让其他bot",
    "让其他 bot",
    "其他bot",
    "其他 bot",
    "新任务",
    "做个任务",
    "交给",
    "委派",
    "delegate",
    "assign",
];

fn contains_keyword(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|kw| lower.contains(kw))
}

/// Returns true if `text` contains any team-trigger keyword.
pub fn is_team_trigger(text: &str) -> bool {
    contains_keyword(text, TEAM_TRIGGER_KEYWORDS)
}

/// Returns true if `text` is explicitly asking the lead to create or delegate team work.
pub fn is_team_delegation_request(text: &str) -> bool {
    contains_keyword(text, TEAM_DELEGATION_KEYWORDS) || is_team_trigger(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_team_trigger_detected() {
        assert!(is_team_trigger("我们需要组建团队来完成这个项目"));
        assert!(is_team_trigger("Use multi-agent swarm for this"));
        assert!(is_team_trigger("分配任务给各个agent"));
    }

    #[test]
    fn test_non_trigger_not_detected() {
        assert!(!is_team_trigger("帮我写一首诗"));
        assert!(!is_team_trigger("What is the weather today?"));
        assert!(!is_team_trigger(""));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(is_team_trigger("Team Mode please"));
        assert!(is_team_trigger("SWARM approach"));
    }

    #[test]
    fn test_team_delegation_request_detected() {
        assert!(is_team_delegation_request(
            "让其他bot做个任务：讲解一下clawbro"
        ));
        assert!(is_team_delegation_request("请委派给 specialist"));
        assert!(is_team_delegation_request("delegate this to another bot"));
    }
}
