pub mod fs;
pub mod shell;
pub mod todo;

use crate::permission::PermissionHandler;
use rig::tool::ToolDyn;
use std::sync::{Arc, Mutex};

pub fn all_tools(working_dir: &str, permission: PermissionHandler) -> Vec<Box<dyn ToolDyn>> {
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
