use std::sync::Arc;

/// A sync function that receives a tool name and a description of the action
/// and returns `true` if the user approves.
pub type PermissionHandler = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// Interactive terminal handler — prints the action and reads y/N from stdin.
pub fn interactive() -> PermissionHandler {
    Arc::new(|tool: &str, action: &str| crate::ui::ask_permission(tool, action))
}

/// Non-interactive handler for subagents — auto-approves all tool calls.
pub fn auto_approve() -> PermissionHandler {
    Arc::new(|_tool: &str, _action: &str| true)
}
