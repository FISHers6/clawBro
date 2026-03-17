//! BashTool — executes shell commands via tokio::process.
//! Extracted from quick-ai, no Tauri dependencies.

use rig::completion::ToolDefinition;
use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Banned commands (security policy)
const BANNED_COMMANDS: &[&str] = &[
    "curl", "wget", "axel", "aria2c", "nc", "telnet", "lynx", "w3m",
];

const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
const BASH_MAX_OUTPUT_CHARS: usize = 30_000;

#[derive(Debug, Deserialize)]
pub struct BashArgs {
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub working_directory: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BashOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct BashTool;

impl Tool for BashTool {
    const NAME: &'static str = "Bash";

    type Error = ToolError;
    type Args = BashArgs;
    type Output = BashOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute a bash command. Returns stdout, stderr, and exit_code."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Optional timeout in milliseconds (max 600000). If omitted, no timeout is applied."
                    },
                    "working_directory": {
                        "type": "string",
                        "description": "Optional working directory (absolute path)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: BashArgs) -> Result<BashOutput, ToolError> {
        let first_token = args.command.split_whitespace().next().unwrap_or("");
        if BANNED_COMMANDS.contains(&first_token) {
            return Err(ToolError::ToolCallError(
                format!("Command '{first_token}' is not allowed").into(),
            ));
        }

        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(&args.command);
        if let Some(dir) = &args.working_directory {
            cmd.current_dir(dir);
        }

        let output = if let Some(timeout_ms) = args.timeout_ms {
            tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms.min(BASH_MAX_TIMEOUT_MS)),
                cmd.output(),
            )
            .await
            .map_err(|_| ToolError::ToolCallError("Command timed out".into()))?
            .map_err(|e| ToolError::ToolCallError(e.into()))?
        } else {
            cmd.output()
                .await
                .map_err(|e| ToolError::ToolCallError(e.into()))?
        };

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Truncate to avoid overwhelming context
        if stdout.len() > BASH_MAX_OUTPUT_CHARS {
            stdout.truncate(BASH_MAX_OUTPUT_CHARS);
            stdout.push_str("\n[truncated]");
        }
        if stderr.len() > BASH_MAX_OUTPUT_CHARS {
            stderr.truncate(BASH_MAX_OUTPUT_CHARS);
            stderr.push_str("\n[truncated]");
        }

        Ok(BashOutput {
            stdout,
            stderr,
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool;
        let result = tool
            .call(BashArgs {
                command: "echo hello".to_string(),
                timeout_ms: None,
                working_directory: None,
            })
            .await;
        assert!(result.is_ok(), "bash echo should succeed");
        let out = result.unwrap();
        assert!(
            out.stdout.contains("hello"),
            "stdout should contain 'hello'"
        );
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool;
        let result = tool
            .call(BashArgs {
                command: "exit 42".to_string(),
                timeout_ms: None,
                working_directory: None,
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 42);
    }

    #[tokio::test]
    async fn test_bash_banned_command() {
        let tool = BashTool;
        let result = tool
            .call(BashArgs {
                command: "curl http://example.com".to_string(),
                timeout_ms: None,
                working_directory: None,
            })
            .await;
        assert!(result.is_err(), "banned command should return error");
    }

    #[tokio::test]
    async fn test_bash_explicit_timeout_still_applies() {
        let tool = BashTool;
        let result = tool
            .call(BashArgs {
                command: "sleep 1".to_string(),
                timeout_ms: Some(10),
                working_directory: None,
            })
            .await;
        assert!(
            result.is_err(),
            "explicit timeout should still return error"
        );
    }

    #[tokio::test]
    async fn test_bash_definition() {
        let tool = BashTool;
        let def = tool.definition("".to_string()).await;
        assert_eq!(def.name, "Bash");
    }
}
