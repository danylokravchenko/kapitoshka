use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
#[error("shell error: {0}")]
pub struct ShellError(String);

#[derive(Deserialize)]
pub struct RunShellArgs {
    /// The shell command to execute
    command: String,
}

#[derive(Serialize)]
pub struct ShellOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

pub struct RunShell {
    working_dir: String,
}

impl RunShell {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: working_dir.to_string(),
        }
    }
}

impl Tool for RunShell {
    const NAME: &'static str = "run_shell";
    type Error = ShellError;
    type Args = RunShellArgs;
    type Output = ShellOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Run a shell command in the working directory. \
                Use for: running tests, building the project, searching with grep/find, \
                checking git status, and other read-only or build operations. \
                Avoid destructive commands unless explicitly requested."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute (e.g. 'cargo test', 'grep -r foo src/')"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        tracing::info!(command = %args.command, "running shell command");

        let output = Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(&self.working_dir)
            .output()
            .map_err(|e| ShellError(e.to_string()))?;

        let result = ShellOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        };

        tracing::debug!(exit_code = result.exit_code, "shell command finished");

        Ok(result)
    }
}
