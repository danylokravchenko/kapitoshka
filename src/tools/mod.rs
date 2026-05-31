pub mod fs;
pub mod shell;
pub mod spawn_agent;
pub mod todo;

use crate::permission::PermissionHandler;
use rig::tool::ToolDyn;
use std::sync::{Arc, Mutex};

/// Core tools shared by both the orchestrator and subagents.
/// Does NOT include `SpawnAgent` so subagents cannot recursively spawn peers.
pub fn base_tools(working_dir: &str, permission: PermissionHandler) -> Vec<Box<dyn ToolDyn>> {
    let todo_store = Arc::new(Mutex::new(Vec::new()));
    vec![
        Box::new(fs::ReadFile::new(working_dir)),
        Box::new(fs::WriteFile::new(working_dir, permission.clone())),
        Box::new(fs::PatchFile::new(working_dir, permission.clone())),
        Box::new(fs::ListDir::new(working_dir)),
        Box::new(fs::SearchFile::new(working_dir)),
        Box::new(shell::RunShell::new(working_dir, permission)),
        Box::new(todo::TodoWrite::new(todo_store)),
    ]
}

/// Full tool set for the orchestrator agent, including `SpawnAgent`.
pub fn all_tools(
    working_dir: &str,
    permission: PermissionHandler,
    model: &str,
    provider: &str,
    thinking: bool,
) -> Vec<Box<dyn ToolDyn>> {
    let mut tools = base_tools(working_dir, permission);
    tools.push(Box::new(spawn_agent::SpawnAgent::new(
        working_dir,
        model,
        provider,
        thinking,
    )));
    tools
}
