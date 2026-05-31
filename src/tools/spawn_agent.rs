use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("spawn_agent error: {0}")]
pub struct SpawnAgentError(String);

#[derive(Deserialize)]
pub struct SpawnAgentArgs {
    /// The task description to hand off to the subagent.
    pub task: String,
    /// Optional extra context prepended to the task (e.g. role or constraints).
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Serialize)]
pub struct SpawnAgentOutput {
    pub result: String,
}

pub struct SpawnAgent {
    dir: String,
    model: String,
    provider: String,
    thinking: bool,
}

impl SpawnAgent {
    pub fn new(dir: &str, model: &str, provider: &str, thinking: bool) -> Self {
        Self {
            dir: dir.to_string(),
            model: model.to_string(),
            provider: provider.to_string(),
            thinking,
        }
    }
}

impl Tool for SpawnAgent {
    const NAME: &'static str = "spawn_agent";
    type Error = SpawnAgentError;
    type Args = SpawnAgentArgs;
    type Output = SpawnAgentOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Spawn a subagent to complete a self-contained task and return its \
                result. Use this to delegate focused sub-tasks (e.g. researching a module, \
                writing a specific file, running tests and summarising failures) that can run \
                independently. The subagent has access to all file and shell tools but cannot \
                spawn further subagents. Returns the subagent's final response."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Full task description for the subagent. Be explicit: \
                            include file paths, goals, and any constraints. The subagent has \
                            no memory of the current conversation."
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional extra context or constraints prepended to \
                            the task (e.g. 'You are a Rust expert. Only edit src/lib.rs.')."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        crate::ui::print_subagent_start(&args.task);

        let full_task = match args.context {
            Some(ctx) if !ctx.trim().is_empty() => format!("{}\n\n{}", ctx.trim(), args.task),
            _ => args.task,
        };

        tracing::info!(
            task = %full_task,
            model = %self.model,
            provider = %self.provider,
            "spawning subagent"
        );

        let result = crate::agent::run_task(
            &self.dir,
            &self.model,
            &self.provider,
            &full_task,
            self.thinking,
            None,
        )
        .await
        .map_err(|e| SpawnAgentError(e.to_string()))?;

        tracing::info!(len = result.len(), "subagent finished");
        crate::ui::print_subagent_done();

        Ok(SpawnAgentOutput { result })
    }
}
