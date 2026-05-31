use serde::Deserialize;

fn default_runs() -> usize {
    1
}

/// A single benchmark task loaded from TOML.
#[derive(Debug, Deserialize)]
pub struct Task {
    /// Unique identifier used in reports (e.g. "create_file").
    pub id: String,
    /// One-line description printed in the report header.
    pub description: String,
    /// The prompt sent to the agent verbatim.
    pub prompt: String,
    /// Shell commands run in the task's temp directory before the agent starts.
    #[serde(default)]
    pub setup: Vec<String>,
    /// Files to write into the temp directory before the agent starts.
    /// Each entry is `{ path, content }`.
    #[serde(default)]
    pub setup_files: Vec<SetupFile>,
    /// How many times to run this task. Pass rate is reported as `k/runs`.
    /// Default 1. Set higher for tasks where model non-determinism is expected.
    #[serde(default = "default_runs")]
    pub runs: usize,
    /// Graders applied after the agent finishes.
    #[serde(rename = "grader", default)]
    pub graders: Vec<Grader>,
}

/// A file to create in the task working directory before the agent runs.
#[derive(Debug, Deserialize)]
pub struct SetupFile {
    pub path: String,
    pub content: String,
}

/// One grader applied to a completed task.
#[derive(Debug, Deserialize)]
pub struct Grader {
    /// Human-readable label printed in the report. Derived from `kind` if absent.
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: GraderKind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraderKind {
    /// The file exists in the task directory after the agent finishes.
    FileExists { path: String },
    /// The file does NOT exist after the agent finishes.
    FileAbsent { path: String },
    /// The file exists and contains `pattern` as a substring.
    FileContains { path: String, pattern: String },
    /// A shell command exits with code 0 (run in the task directory).
    ShellSucceeds { cmd: String },
    /// The trajectory contains at least one call to the named tool.
    ToolCalled { name: String },
    /// The trajectory contains no call to the named tool.
    ToolNotCalled { name: String },
    /// The total number of tool calls across all spans is ≤ `count`.
    MaxToolCalls { count: usize },
    /// The agent's final response contains `pattern` as a case-insensitive substring.
    ResponseContains { pattern: String },
    /// The total wall-clock time for the task is ≤ `ms` milliseconds.
    MaxDurationMs { ms: u64 },
}

impl GraderKind {
    pub fn default_label(&self) -> String {
        match self {
            Self::FileExists { path } => format!("file_exists({path})"),
            Self::FileAbsent { path } => format!("file_absent({path})"),
            Self::FileContains { path, pattern } => {
                format!("file_contains({path}, {pattern:?})")
            }
            Self::ShellSucceeds { cmd } => format!("shell_succeeds({cmd:?})"),
            Self::ToolCalled { name } => format!("tool_called({name})"),
            Self::ToolNotCalled { name } => format!("tool_not_called({name})"),
            Self::MaxToolCalls { count } => format!("max_tool_calls({count})"),
            Self::ResponseContains { pattern } => format!("response_contains({pattern:?})"),
            Self::MaxDurationMs { ms } => format!("max_duration_ms({ms})"),
        }
    }
}

/// Top-level TOML document: a list of tasks.
#[derive(Debug, Deserialize)]
pub struct TaskSuite {
    #[serde(rename = "task")]
    pub tasks: Vec<Task>,
}

impl TaskSuite {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let suite: Self = toml::from_str(&text)?;
        Ok(suite)
    }
}
