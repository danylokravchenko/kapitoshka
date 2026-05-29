pub mod fs;
pub mod shell;

use crate::permission::PermissionHandler;
use rig::tool::ToolDyn;

pub fn all_tools(working_dir: &str, permission: PermissionHandler) -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(fs::ReadFile::new(working_dir)),
        Box::new(fs::WriteFile::new(working_dir, permission.clone())),
        Box::new(fs::PatchFile::new(working_dir, permission.clone())),
        Box::new(fs::ListDir::new(working_dir)),
        Box::new(fs::SearchFile::new(working_dir)),
        Box::new(shell::RunShell::new(working_dir, permission)),
    ]
}
