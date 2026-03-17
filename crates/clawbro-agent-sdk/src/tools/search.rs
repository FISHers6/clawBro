//! Search tools: GlobTool, GrepTool, LsTool
//! Extracted from quick-ai, no Tauri dependencies.

use rig::completion::ToolDefinition;
use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ─── GlobTool ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GlobArgs {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GlobOutput {
    pub matches: Vec<String>,
    pub count: usize,
}

pub struct GlobTool;

impl Tool for GlobTool {
    const NAME: &'static str = "Glob";
    type Error = ToolError;
    type Args = GlobArgs;
    type Output = GlobOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Find files matching a glob pattern. Returns file paths sorted by modification time.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. '**/*.rs'" },
                    "path": { "type": "string", "description": "Base directory (default: cwd)" }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: GlobArgs) -> Result<GlobOutput, ToolError> {
        let base = args
            .path
            .as_deref()
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new("."));

        let full_pattern = base.join(&args.pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> =
            glob::glob(&pattern_str)
                .map_err(|e| ToolError::ToolCallError(format!("Invalid glob pattern: {e}").into()))?
                .filter_map(|r| r.ok())
                .filter(|p| p.is_file())
                .filter_map(|p| {
                    let mtime = p.metadata().and_then(|m| m.modified()).ok()?;
                    Some((p, mtime))
                })
                .collect();

        // Sort by modification time, newest first
        entries.sort_by(|a, b| b.1.cmp(&a.1));

        let matches: Vec<String> = entries
            .into_iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        let count = matches.len();

        Ok(GlobOutput { matches, count })
    }
}

// ─── GrepTool ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include: Option<String>,
    #[serde(default)]
    pub case_insensitive: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct GrepMatch {
    pub file: String,
    pub line: usize,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct GrepOutput {
    pub matches: Vec<GrepMatch>,
    pub count: usize,
}

pub struct GrepTool;

impl Tool for GrepTool {
    const NAME: &'static str = "Grep";
    type Error = ToolError;
    type Args = GrepArgs;
    type Output = GrepOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search file contents using a regex pattern. Returns matching file paths, line numbers, and content.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Directory to search in (default: cwd)" },
                    "include": { "type": "string", "description": "Glob filter, e.g. '*.rs'" },
                    "case_insensitive": { "type": "boolean", "description": "Case insensitive search" }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: GrepArgs) -> Result<GrepOutput, ToolError> {
        let re = {
            let mut builder = regex::RegexBuilder::new(&args.pattern);
            builder.case_insensitive(args.case_insensitive.unwrap_or(false));
            builder
                .build()
                .map_err(|e| ToolError::ToolCallError(format!("Invalid regex: {e}").into()))?
        };

        let base = args.path.as_deref().unwrap_or(".");
        let include_pattern: Option<glob::Pattern> = args
            .include
            .as_deref()
            .and_then(|p| glob::Pattern::new(p).ok());

        let mut results: Vec<GrepMatch> = Vec::new();
        let max_results = 500;

        for entry in walkdir::WalkDir::new(base)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            if results.len() >= max_results {
                break;
            }

            let file_name = entry.file_name().to_string_lossy();
            if let Some(pat) = &include_pattern {
                if !pat.matches(file_name.as_ref()) {
                    continue;
                }
            }

            // Skip binary files and very large files
            let metadata = entry
                .metadata()
                .map_err(|e| ToolError::ToolCallError(e.into()))?;
            if metadata.len() > 10 * 1024 * 1024 {
                continue;
            }

            let path_str = entry.path().to_string_lossy().into_owned();
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                for (line_no, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        results.push(GrepMatch {
                            file: path_str.clone(),
                            line: line_no + 1,
                            content: line.chars().take(200).collect(),
                        });
                        if results.len() >= max_results {
                            break;
                        }
                    }
                }
            }
        }

        let count = results.len();
        Ok(GrepOutput {
            matches: results,
            count,
        })
    }
}

// ─── LsTool ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LsArgs {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct LsOutput {
    pub entries: Vec<String>,
}

pub struct LsTool;

impl Tool for LsTool {
    const NAME: &'static str = "LS";
    type Error = ToolError;
    type Args = LsArgs;
    type Output = LsOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List files and directories at a given path. path must be absolute."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute directory path" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: LsArgs) -> Result<LsOutput, ToolError> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&args.path)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }

        entries.sort();
        Ok(LsOutput { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_glob_rs_files() {
        let tool = GlobTool;
        let result = tool
            .call(GlobArgs {
                pattern: "**/*.rs".to_string(),
                path: Some(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string())),
            })
            .await;
        assert!(result.is_ok(), "glob should succeed");
        let out = result.unwrap();
        assert!(out.count > 0, "should find .rs files");
    }

    #[tokio::test]
    async fn test_grep_pattern() {
        let tool = GrepTool;
        let result = tool
            .call(GrepArgs {
                pattern: "pub fn".to_string(),
                path: Some(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string())),
                include: Some("*.rs".to_string()),
                case_insensitive: None,
            })
            .await;
        assert!(result.is_ok(), "grep should succeed");
        let out = result.unwrap();
        assert!(out.count > 0, "should find 'pub fn' in .rs files");
    }

    #[tokio::test]
    async fn test_ls() {
        let tool = LsTool;
        let path = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let result = tool.call(LsArgs { path }).await;
        assert!(result.is_ok(), "ls should succeed");
        let out = result.unwrap();
        assert!(!out.entries.is_empty(), "directory should not be empty");
    }
}
