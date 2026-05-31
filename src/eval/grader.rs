use crate::eval::task::{Grader, GraderKind};
use crate::trajectory::{Span, TurnRecord};

/// Outcome of a single grader evaluation.
#[derive(Debug)]
pub struct GraderResult {
    pub label: String,
    pub passed: bool,
    pub detail: Option<String>,
}

/// Evaluate all graders for a completed task. `response` is the agent's final
/// text reply; `trajectory` is the parsed `.jsonl` records.
pub fn evaluate_all(
    graders: &[Grader],
    task_dir: &std::path::Path,
    trajectory: &[TurnRecord],
    response: &str,
    duration_ms: u64,
) -> Vec<GraderResult> {
    graders
        .iter()
        .map(|g| evaluate(g, task_dir, trajectory, response, duration_ms))
        .collect()
}

fn evaluate(
    grader: &Grader,
    task_dir: &std::path::Path,
    trajectory: &[TurnRecord],
    response: &str,
    duration_ms: u64,
) -> GraderResult {
    let label = grader
        .label
        .clone()
        .unwrap_or_else(|| grader.kind.default_label());

    let (passed, detail) = match &grader.kind {
        GraderKind::FileExists { path } => {
            let full = task_dir.join(path);
            let exists = full.exists();
            (
                exists,
                if exists {
                    None
                } else {
                    Some(format!("{path} not found"))
                },
            )
        }
        GraderKind::FileAbsent { path } => {
            let full = task_dir.join(path);
            let exists = full.exists();
            (
                !exists,
                if exists {
                    Some(format!("{path} exists but should not"))
                } else {
                    None
                },
            )
        }
        GraderKind::FileContains { path, pattern } => {
            let full = task_dir.join(path);
            match std::fs::read_to_string(&full) {
                Ok(content) => {
                    let found = content.contains(pattern.as_str());
                    (
                        found,
                        if found {
                            None
                        } else {
                            Some(format!("{path:?} does not contain {pattern:?}"))
                        },
                    )
                }
                Err(e) => (false, Some(format!("could not read {path}: {e}"))),
            }
        }
        GraderKind::ShellSucceeds { cmd } => {
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(task_dir)
                .status();
            match status {
                Ok(s) if s.success() => (true, None),
                Ok(s) => (false, Some(format!("exited with {s}"))),
                Err(e) => (false, Some(format!("failed to run: {e}"))),
            }
        }
        GraderKind::ToolCalled { name } => {
            let called = tool_was_called(trajectory, name);
            (
                called,
                if called {
                    None
                } else {
                    Some(format!("{name} was not called"))
                },
            )
        }
        GraderKind::ToolNotCalled { name } => {
            let called = tool_was_called(trajectory, name);
            (
                !called,
                if called {
                    Some(format!("{name} was called but should not have been"))
                } else {
                    None
                },
            )
        }
        GraderKind::MaxToolCalls { count } => {
            let actual = count_tool_calls(trajectory);
            let ok = actual <= *count;
            (
                ok,
                if ok {
                    None
                } else {
                    Some(format!("{actual} tool calls > limit {count}"))
                },
            )
        }
        GraderKind::ResponseContains { pattern } => {
            let found = response.to_lowercase().contains(&pattern.to_lowercase());
            (
                found,
                if found {
                    None
                } else {
                    Some(format!("response does not contain {pattern:?}"))
                },
            )
        }
        GraderKind::MaxDurationMs { ms } => {
            let ok = duration_ms <= *ms;
            (
                ok,
                if ok {
                    None
                } else {
                    Some(format!("{duration_ms}ms > limit {ms}ms"))
                },
            )
        }
    };

    GraderResult {
        label,
        passed,
        detail,
    }
}

fn tool_was_called(trajectory: &[TurnRecord], name: &str) -> bool {
    trajectory.iter().any(|record| {
        record
            .spans
            .iter()
            .any(|span| matches!(span, Span::ToolCall { name: n, .. } if n == name))
    })
}

fn count_tool_calls(trajectory: &[TurnRecord]) -> usize {
    trajectory
        .iter()
        .flat_map(|r| r.spans.iter())
        .filter(|s| matches!(s, Span::ToolCall { .. }))
        .count()
}
