pub mod fs;
pub mod shell;

use rig::tool::ToolDyn;

pub fn all_tools(working_dir: &str) -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(fs::ReadFile::new(working_dir)),
        Box::new(fs::WriteFile::new(working_dir)),
        Box::new(fs::PatchFile::new(working_dir)),
        Box::new(fs::ListDir::new(working_dir)),
        Box::new(fs::SearchFile::new(working_dir)),
        Box::new(shell::RunShell::new(working_dir)),
    ]
}
