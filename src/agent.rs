use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Chat;
use rig::providers::openai::CompletionsClient;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::permission;
use crate::session::Session;
use crate::tools::all_tools;
use crate::ui;

const SYSTEM_PROMPT: &str = "\
You are kapitoshka, an expert coding agent. You help users with software engineering tasks.

You have access to tools to read files, write files, list directories, and run shell commands.
Always explore the codebase before making changes. Prefer targeted edits over full rewrites.
After making changes, verify them by reading the modified files or running relevant commands.

Working directory is provided in the task. Use it as the root for all file operations.";

pub async fn run_interactive(dir: &str, model: &str) -> Result<()> {
    let client = CompletionsClient::from_env()?;
    let perm = permission::interactive();

    let agent = client
        .agent(model)
        // Do NOT use .preamble() — many local servers (Qwen, llama.cpp, Ollama)
        // reject system-message content serialized as an array of objects.
        // Instead the system prompt is injected into the first user message.
        .tools(all_tools(dir, perm))
        .max_tokens(8192)
        .default_max_turns(20)
        .build();

    let mut session = Session::new(dir, model)?;
    let session_path = session.path.display().to_string();

    ui::print_banner(model, dir, &session_path);

    let mut rl = DefaultEditor::new()?;
    let mut first_turn = true;

    loop {
        // \x01 / \x02 bracket invisible sequences so rustyline measures width correctly.
        let prompt = "\x01\x1b[32m\x02❯ \x01\x1b[0m\x02";
        match rl.readline(prompt) {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "exit" || line == "quit" {
                    break;
                }
                let _ = rl.add_history_entry(&line);

                // Inject system prompt into the first message — local servers reject
                // the system role when its content is serialized as an array.
                let message = if first_turn {
                    first_turn = false;
                    format!("{SYSTEM_PROMPT}\n\n---\n\n{line}")
                } else {
                    line.clone()
                };

                session.log_user(&line)?;
                ui::print_thinking();

                match agent.chat(message.as_str(), &mut session.history).await {
                    Ok(response) => {
                        ui::print_response(&response);
                        session.log_agent(&response)?;
                    }
                    Err(e) => {
                        ui::print_error(&e.to_string());
                    }
                }
            }
            Err(ReadlineError::Eof | ReadlineError::Interrupted) => break,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
