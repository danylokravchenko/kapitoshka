use crate::eval::runner::{AttemptResult, TaskResult};

/// Print a human-readable report to stdout.
pub fn print_report(results: &[TaskResult], model: &str, provider: &str) {
    let width = 64;
    let bar = "═".repeat(width);

    println!();
    println!("  Eval run  model: {model}  provider: {provider}");
    println!("  {bar}");
    println!();

    for r in results {
        print_task(r);
    }

    println!();
    println!("  {bar}");

    let tasks_passed = results.iter().filter(|r| r.all_passed()).count();
    let tasks_total = results.len();
    let avg_ms = if results.is_empty() {
        0
    } else {
        results.iter().map(|r| r.avg_duration_ms()).sum::<u64>() / results.len() as u64
    };
    let pct = (tasks_passed * 100).checked_div(tasks_total).unwrap_or(0);

    println!("  {tasks_passed}/{tasks_total} tasks passed  ({pct}%)    avg {avg_ms}ms");
    println!();
}

fn print_task(r: &TaskResult) {
    let multi = r.runs > 1;
    let passes = r.passes();

    // Check for an infrastructure error (agent couldn't start).
    if let Some(first) = r.attempts.first()
        && let Err(ref e) = first.agent_result
    {
        println!("  ✗ {:<32}  error  {}ms", r.task_id, first.duration_ms);
        println!("      agent error: {e}");
        return;
    }

    if multi {
        let icon = if passes == r.runs { "✓" } else { "✗" };
        let status = if passes == r.runs {
            "pass".to_string()
        } else {
            format!("{passes}/{} runs passed", r.runs)
        };
        println!(
            "  {icon} {:<32}  {status}  avg {}ms",
            r.task_id,
            r.avg_duration_ms()
        );
        // Show grader breakdown for any failing attempt.
        for (i, attempt) in r.attempts.iter().enumerate() {
            if !attempt.passed() {
                println!("    run {}:", i + 1);
                print_attempt_failures(attempt);
            }
        }
    } else {
        // Single run — classic display.
        let attempt = match r.attempts.first() {
            Some(a) => a,
            None => return,
        };
        let passed = attempt.graders.iter().filter(|g| g.passed).count();
        let total = attempt.graders.len();
        let icon = if passed == total { "✓" } else { "✗" };
        let status = if passed == total { "pass" } else { "fail" };
        println!(
            "  {icon} {:<32}  {status}  {passed}/{total}  {}ms",
            r.task_id, attempt.duration_ms
        );
        print_attempt_failures(attempt);
    }
}

fn print_attempt_failures(attempt: &AttemptResult) {
    if let Err(ref e) = attempt.agent_result {
        println!("      agent error: {e}");
        return;
    }
    for g in &attempt.graders {
        if !g.passed {
            let detail = g.detail.as_deref().unwrap_or("failed");
            println!("      ✗ {}: {detail}", g.label);
        }
    }
}

/// Write results as a JSON report file.
pub fn write_report(
    results: &[TaskResult],
    model: &str,
    provider: &str,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let report = serde_json::json!({
        "model": model,
        "provider": provider,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "summary": {
            "tasks_total": results.len(),
            "tasks_passed": results.iter().filter(|r| r.all_passed()).count(),
            "avg_duration_ms": if results.is_empty() { 0 } else {
                results.iter().map(|r| r.avg_duration_ms()).sum::<u64>() / results.len() as u64
            },
        },
        "tasks": results.iter().map(task_to_json).collect::<Vec<_>>(),
    });

    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn task_to_json(r: &TaskResult) -> serde_json::Value {
    serde_json::json!({
        "id": r.task_id,
        "runs": r.runs,
        "passes": r.passes(),
        "all_passed": r.all_passed(),
        "avg_duration_ms": r.avg_duration_ms(),
        "attempts": r.attempts.iter().map(attempt_to_json).collect::<Vec<_>>(),
    })
}

fn attempt_to_json(a: &AttemptResult) -> serde_json::Value {
    serde_json::json!({
        "passed": a.passed(),
        "duration_ms": a.duration_ms,
        "agent_error": a.agent_result.as_ref().err().map(|e| e.to_string()),
        "graders": a.graders.iter().map(|g| serde_json::json!({
            "label": g.label,
            "passed": g.passed,
            "detail": g.detail,
        })).collect::<Vec<_>>(),
    })
}
