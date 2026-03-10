use crate::config::ProgressPresentationMode;

pub fn format_tool_start(mode: ProgressPresentationMode, tool_name: &str) -> Option<String> {
    match mode {
        ProgressPresentationMode::FinalOnly => None,
        ProgressPresentationMode::ProgressCompact => Some(match_tool_start(tool_name)),
    }
}

pub fn format_tool_result(
    mode: ProgressPresentationMode,
    tool_name: Option<&str>,
) -> Option<String> {
    match mode {
        ProgressPresentationMode::FinalOnly => None,
        ProgressPresentationMode::ProgressCompact => Some(match tool_name {
            Some(name) if is_file_tool(name) => "⏳ 正在整理文件内容".to_string(),
            Some(name) if is_search_tool(name) => "⏳ 正在整理搜索结果".to_string(),
            Some(name) if is_command_tool(name) => "⏳ 正在整理命令输出".to_string(),
            Some(name) if is_write_tool(name) => "⏳ 正在整理修改结果".to_string(),
            _ => "⏳ 正在整理结果".to_string(),
        }),
    }
}

pub fn format_tool_failure(mode: ProgressPresentationMode, tool_name: &str) -> Option<String> {
    match mode {
        ProgressPresentationMode::FinalOnly => None,
        ProgressPresentationMode::ProgressCompact => Some(if is_command_tool(tool_name) {
            "⚠️ 命令执行失败，正在整理错误".to_string()
        } else {
            "⚠️ 工具执行失败，正在整理错误".to_string()
        }),
    }
}

fn match_tool_start(tool_name: &str) -> String {
    if is_file_tool(tool_name) {
        "⏳ 正在查看文件".to_string()
    } else if is_search_tool(tool_name) {
        "⏳ 正在搜索代码".to_string()
    } else if is_command_tool(tool_name) {
        "⏳ 正在执行命令".to_string()
    } else if is_write_tool(tool_name) {
        "⏳ 正在修改文件".to_string()
    } else if is_team_tool(tool_name) {
        "⏳ 正在协调团队任务".to_string()
    } else {
        "⏳ 正在调用工具".to_string()
    }
}

fn is_file_tool(tool_name: &str) -> bool {
    matches_ci(tool_name, &["view", "read", "cat"])
}

fn is_search_tool(tool_name: &str) -> bool {
    matches_ci(tool_name, &["grep", "glob", "ls", "search", "find"])
}

fn is_command_tool(tool_name: &str) -> bool {
    matches_ci(tool_name, &["bash", "shell", "exec", "command"])
}

fn is_write_tool(tool_name: &str) -> bool {
    matches_ci(tool_name, &["write", "edit", "patch"])
}

fn is_team_tool(tool_name: &str) -> bool {
    matches_ci(tool_name, &["team", "task"])
}

fn matches_ci(tool_name: &str, patterns: &[&str]) -> bool {
    let name = tool_name.to_ascii_lowercase();
    patterns.iter().any(|pattern| name.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_start_maps_to_compact_file_progress() {
        assert_eq!(
            format_tool_start(ProgressPresentationMode::ProgressCompact, "View"),
            Some("⏳ 正在查看文件".to_string())
        );
    }

    #[test]
    fn test_tool_start_maps_search_without_exposing_args() {
        assert_eq!(
            format_tool_start(ProgressPresentationMode::ProgressCompact, "grep"),
            Some("⏳ 正在搜索代码".to_string())
        );
    }

    #[test]
    fn test_final_only_suppresses_progress() {
        assert_eq!(
            format_tool_start(ProgressPresentationMode::FinalOnly, "View"),
            None
        );
        assert_eq!(
            format_tool_result(ProgressPresentationMode::FinalOnly, Some("View")),
            None
        );
    }
}
