use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai::CompletionsClient;

use crate::tools::all_tools;

const SYSTEM_PROMPT: &str = "\
You are kapitoshka, an expert coding agent. You help users with software engineering tasks.

You have access to tools to read files, write files, list directories, and run shell commands.
Always explore the codebase before making changes. Prefer targeted edits over full rewrites.
After making changes, verify them by reading the modified files or running relevant commands.

Working directory is provided in the task. Use it as the root for all file operations.";

pub async fn run(task: &str, dir: &str, model: &str) -> Result<String> {
    // Uses /v1/chat/completions — compatible with Ollama, vLLM, LM Studio, etc.
    // Reads OPENAI_API_KEY and optionally OPENAI_BASE_URL from the environment.
    let client = CompletionsClient::from_env()?;

    let agent = client
        .agent(model)
        .tools(all_tools(dir))
        .max_tokens(8192)
        .default_max_turns(20)
        .build();

    // Prepend the system prompt to the user task instead of using .preamble().
    // Many local servers (llama.cpp, etc.) reject system message content serialized
    // as an array — rig has no plain-string serializer for system messages yet.
    let prompt = format!("{SYSTEM_PROMPT}\n\n---\n\n{task}");

    tracing::debug!("sending task to agent");

    let response = agent
        .prompt(prompt.as_str())
        .await
        .map_err(|e| anyhow::anyhow!("agent error: {e}"))?;

    Ok(response)
}
