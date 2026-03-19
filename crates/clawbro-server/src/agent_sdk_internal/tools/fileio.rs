//! File I/O tools: ViewFileTool, WriteFileTool, EditFileTool
//! Extracted from quick-ai, no Tauri dependencies.

use rig::completion::ToolDefinition;
use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

const MAX_VIEW_LINES: usize = 2000;
const MAX_LINE_LEN: usize = 2000;
const MAX_DIR_ENTRIES: usize = 200;

// ─── ViewFileTool ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ViewFileArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ViewFileOutput {
    pub content: String,
    pub total_lines: usize,
}

pub struct ViewFileTool;

async fn render_directory_listing(path: &Path) -> Result<ViewFileOutput, ToolError> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|e| ToolError::ToolCallError(e.into()))?;
    let mut names = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| ToolError::ToolCallError(e.into()))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;
        let suffix = if file_type.is_dir() { "/" } else { "" };
        names.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
    }

    names.sort();
    let total_entries = names.len();
    let content = if total_entries == 0 {
        format!("{} is an empty directory", path.display())
    } else {
        let mut lines = vec![format!(
            "{} is a directory. Entries (showing up to {} of {}):",
            path.display(),
            MAX_DIR_ENTRIES,
            total_entries
        )];
        lines.extend(names.into_iter().take(MAX_DIR_ENTRIES));
        lines.join("\n")
    };

    Ok(ViewFileOutput {
        content,
        total_lines: total_entries,
    })
}

impl Tool for ViewFileTool {
    const NAME: &'static str = "View";
    type Error = ToolError;
    type Args = ViewFileArgs;
    type Output = ViewFileOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read a file from the local filesystem. path must be absolute."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute file path" },
                    "offset": { "type": "integer", "description": "Start line (1-based)" },
                    "limit": { "type": "integer", "description": "Max lines to return" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: ViewFileArgs) -> Result<ViewFileOutput, ToolError> {
        let path = Path::new(&args.path);
        if tokio::fs::metadata(path)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?
            .is_dir()
        {
            return render_directory_listing(path).await;
        }

        let raw = tokio::fs::read_to_string(&args.path)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;

        let all_lines: Vec<&str> = raw.lines().collect();
        let total_lines = all_lines.len();

        let start = args.offset.unwrap_or(1).saturating_sub(1);
        let take = args.limit.unwrap_or(MAX_VIEW_LINES);

        let content = all_lines
            .iter()
            .enumerate()
            .skip(start)
            .take(take)
            .map(|(i, line)| {
                let line = if line.len() > MAX_LINE_LEN {
                    &line[..MAX_LINE_LEN]
                } else {
                    line
                };
                format!("{:>6}\t{}", i + 1, line)
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ViewFileOutput {
            content,
            total_lines,
        })
    }
}

// ─── WriteFileTool ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct WriteFileOutput {
    pub written_bytes: usize,
}

pub struct WriteFileTool;

impl Tool for WriteFileTool {
    const NAME: &'static str = "Write";
    type Error = ToolError;
    type Args = WriteFileArgs;
    type Output = WriteFileOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Write content to a file (overwrites if exists). path must be absolute."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute file path" },
                    "content": { "type": "string", "description": "File content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: WriteFileArgs) -> Result<WriteFileOutput, ToolError> {
        // Create parent directories if needed
        if let Some(parent) = std::path::Path::new(&args.path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ToolCallError(e.into()))?;
        }
        let bytes = args.content.len();
        tokio::fs::write(&args.path, &args.content)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;
        Ok(WriteFileOutput {
            written_bytes: bytes,
        })
    }
}

// ─── EditFileTool ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Serialize)]
pub struct EditFileOutput {
    pub replacements: usize,
}

pub struct EditFileTool;

impl Tool for EditFileTool {
    const NAME: &'static str = "Edit";
    type Error = ToolError;
    type Args = EditFileArgs;
    type Output = EditFileOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Perform an exact string replacement in a file. old_string must be unique."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute file path" },
                    "old_string": { "type": "string", "description": "Exact text to find" },
                    "new_string": { "type": "string", "description": "Replacement text" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn call(&self, args: EditFileArgs) -> Result<EditFileOutput, ToolError> {
        let content = tokio::fs::read_to_string(&args.path)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;

        let replacements = content.matches(&args.old_string).count();
        if replacements == 0 {
            return Err(ToolError::ToolCallError(
                format!("old_string not found in {}", args.path).into(),
            ));
        }

        let new_content = content.replace(&args.old_string, &args.new_string);
        tokio::fs::write(&args.path, new_content)
            .await
            .map_err(|e| ToolError::ToolCallError(e.into()))?;

        Ok(EditFileOutput { replacements })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[tokio::test]
    async fn test_view_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "line1\nline2\nline3").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let tool = ViewFileTool;
        let result = tool
            .call(ViewFileArgs {
                path,
                offset: None,
                limit: None,
            })
            .await;
        assert!(result.is_ok());
        let out = result.unwrap();
        assert!(out.content.contains("line1"));
        assert_eq!(out.total_lines, 3);
    }

    #[tokio::test]
    async fn test_view_directory_returns_listing_instead_of_error() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("notes.txt");
        let nested_dir = dir.path().join("nested");
        std::fs::write(&file_path, "hello").unwrap();
        std::fs::create_dir(&nested_dir).unwrap();

        let tool = ViewFileTool;
        let out = tool
            .call(ViewFileArgs {
                path: dir.path().to_string_lossy().to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();

        assert!(out.content.contains("is a directory"));
        assert!(out.content.contains("notes.txt"));
        assert!(out.content.contains("nested/"));
        assert_eq!(out.total_lines, 2);
    }

    #[tokio::test]
    async fn test_write_and_view() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let write_tool = WriteFileTool;
        write_tool
            .call(WriteFileArgs {
                path: path.clone(),
                content: "hello world".to_string(),
            })
            .await
            .unwrap();

        let view_tool = ViewFileTool;
        let out = view_tool
            .call(ViewFileArgs {
                path,
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(out.content.contains("hello world"));
    }

    #[tokio::test]
    async fn test_edit_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "foo bar").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let tool = EditFileTool;
        let out = tool
            .call(EditFileArgs {
                path: path.clone(),
                old_string: "foo".to_string(),
                new_string: "baz".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(out.replacements, 1);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "baz bar");
    }
}
