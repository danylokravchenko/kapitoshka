use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;

use crate::eval::grader::{GraderResult, evaluate_all};
use crate::eval::task::Task;
use crate::trajectory::TurnRecord;

/// Result of one attempt at a task.
pub struct AttemptResult {
    pub duration_ms: u64,
    pub agent_result: Result<String>,
    pub graders: Vec<GraderResult>,
}

impl AttemptResult {
    pub fn passed(&self) -> bool {
        self.agent_result.is_ok() && self.graders.iter().all(|g| g.passed)
    }
}

/// Aggregated result across all attempts for a task.
pub struct TaskResult {
    pub task_id: String,
    /// Total runs attempted.
    pub runs: usize,
    /// Attempts in order. Length equals `runs`.
    pub attempts: Vec<AttemptResult>,
}

impl TaskResult {
    pub fn passes(&self) -> usize {
        self.attempts.iter().filter(|a| a.passed()).count()
    }

    pub fn avg_duration_ms(&self) -> u64 {
        if self.attempts.is_empty() {
            return 0;
        }
        self.attempts.iter().map(|a| a.duration_ms).sum::<u64>() / self.attempts.len() as u64
    }

    /// True only when ALL attempts passed.
    pub fn all_passed(&self) -> bool {
        self.passes() == self.runs
    }
}

/// Run a single attempt of a task and return its result.
async fn run_attempt(task: &Task, model: &str, provider: &str, thinking: bool) -> AttemptResult {
    let tmp = match tempfile::TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            return AttemptResult {
                duration_ms: 0,
                agent_result: Err(anyhow::anyhow!("failed to create temp dir: {e}")),
                graders: vec![],
            };
        }
    };

    // Write setup files.
    for f in &task.setup_files {
        let dest = tmp.path().join(&f.path);
        if let Some(parent) = dest.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return err_attempt(format!("setup_files mkdir: {e}"));
        }
        if let Err(e) = std::fs::write(&dest, &f.content) {
            return err_attempt(format!("setup_files write {}: {e}", f.path));
        }
    }

    // Run setup shell commands.
    for cmd in &task.setup {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(tmp.path())
            .status();
        match status {
            Ok(s) if !s.success() => {
                return err_attempt(format!("setup command failed ({s}): {cmd}"));
            }
            Err(e) => {
                return err_attempt(format!("setup command error: {e}"));
            }
            Ok(_) => {}
        }
    }

    // Prepare a stable trajectory path so we can read it back for grading.
    let traj_md: PathBuf = tmp.path().join("trajectory.md");
    let traj_jsonl: PathBuf = traj_md.with_extension("jsonl");

    let start = Instant::now();
    let agent_result = crate::agent::run_task(
        tmp.path().to_str().unwrap_or("."),
        model,
        provider,
        &task.prompt,
        thinking,
        Some(&traj_md),
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    // Only run graders when the agent completed — an infrastructure error
    // (missing API key, unreachable server) is not a grader failure.
    let graders = if agent_result.is_ok() {
        let trajectory = load_trajectory(&traj_jsonl);
        let response = agent_result.as_deref().unwrap_or("");
        evaluate_all(
            &task.graders,
            tmp.path(),
            &trajectory,
            response,
            duration_ms,
        )
    } else {
        vec![]
    };

    AttemptResult {
        duration_ms,
        agent_result,
        graders,
    }
}

/// Run a task `task.runs` times and aggregate into a `TaskResult`.
pub async fn run_task(task: &Task, model: &str, provider: &str, thinking: bool) -> TaskResult {
    let mut attempts = Vec::with_capacity(task.runs);
    for run in 0..task.runs {
        if task.runs > 1 {
            println!("  running  {}  (run {}/{}) …", task.id, run + 1, task.runs);
        } else {
            println!("  running  {} …", task.id);
        }
        let attempt = run_attempt(task, model, provider, thinking).await;
        // Stop early on infrastructure errors — retrying won't help.
        let is_infra_err = attempt.agent_result.is_err();
        attempts.push(attempt);
        if is_infra_err {
            break;
        }
    }
    TaskResult {
        task_id: task.id.clone(),
        runs: task.runs,
        attempts,
    }
}

/// Run the full suite sequentially and collect results.
pub async fn run_suite(
    tasks: &[Task],
    model: &str,
    provider: &str,
    thinking: bool,
    filter: Option<&str>,
) -> Vec<TaskResult> {
    let mut results = Vec::new();
    for task in tasks {
        if let Some(f) = filter
            && !task.id.contains(f)
        {
            continue;
        }
        let result = run_task(task, model, provider, thinking).await;
        results.push(result);
    }
    results
}

fn err_attempt(msg: String) -> AttemptResult {
    AttemptResult {
        duration_ms: 0,
        agent_result: Err(anyhow::anyhow!("{msg}")),
        graders: vec![],
    }
}

fn load_trajectory(path: &std::path::Path) -> Vec<TurnRecord> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
