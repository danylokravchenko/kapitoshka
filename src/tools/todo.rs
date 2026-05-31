use crate::ui;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
#[error("todo error: {0}")]
pub struct TodoError(String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Deserialize)]
pub struct TodoWriteArgs {
    todos: Vec<TodoItem>,
}

pub struct TodoWrite {
    store: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoWrite {
    pub fn new(store: Arc<Mutex<Vec<TodoItem>>>) -> Self {
        Self { store }
    }
}

impl Tool for TodoWrite {
    const NAME: &'static str = "todo_write";
    type Error = TodoError;
    type Args = TodoWriteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Create or update the task plan. Call this at the start of every \
                non-trivial task to lay out the steps, then call it again to mark steps \
                in_progress or completed as you work. Replaces the entire list each time. \
                Statuses: pending | in_progress | completed."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete, up-to-date list of todo items.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id":      { "type": "string", "description": "Short stable identifier, e.g. \"1\", \"2\"." },
                                "content": { "type": "string", "description": "One-line description of the step." },
                                "status":  { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                            },
                            "required": ["id", "content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut store = self.store.lock().map_err(|e| TodoError(e.to_string()))?;
        *store = args.todos.clone();
        drop(store);
        ui::print_todo_list(&args.todos);
        Ok("Plan updated.".to_string())
    }
}
