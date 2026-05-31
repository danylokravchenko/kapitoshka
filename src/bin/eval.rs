use anyhow::Result;
use clap::Parser;
use kapitoshka::eval;
use kapitoshka::settings::Settings;
use kapitoshka::trace;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "eval",
    about = "Run the kapitoshka eval harness against a live model"
)]
struct Cli {
    /// Path to the TOML task suite file.
    #[arg(short, long, default_value = "evals/tasks.toml")]
    suite: std::path::PathBuf,

    /// Provider backend: "openai" (default) or "anthropic".
    #[arg(short, long, default_value = "openai")]
    provider: String,

    /// Model name to evaluate. Reads from settings if not provided.
    #[arg(short, long)]
    model: Option<String>,

    /// Only run tasks whose ID contains this substring.
    #[arg(long)]
    filter: Option<String>,

    /// Write a JSON report to this file in addition to the terminal output.
    #[arg(long)]
    report: Option<std::path::PathBuf>,

    /// Show the model's internal reasoning if supported.
    #[arg(long)]
    thinking: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            tracing::error!(error = %e, "eval failed");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Returns `true` if all tasks passed.
async fn run() -> Result<bool> {
    let cli = Cli::parse();

    let _trace_guards = trace::init();

    let model = cli
        .model
        .or_else(|| Settings::load().model)
        .unwrap_or_default();

    if model.is_empty() {
        anyhow::bail!("no model configured — pass --model or run kapitoshka to set one");
    }

    let suite = eval::task::TaskSuite::load(&cli.suite)?;

    println!(
        "\n  Loading {} tasks from {}",
        suite.tasks.len(),
        cli.suite.display()
    );

    let results = eval::runner::run_suite(
        &suite.tasks,
        &model,
        &cli.provider,
        cli.thinking,
        cli.filter.as_deref(),
    )
    .await;

    eval::report::print_report(&results, &model, &cli.provider);

    if let Some(report_path) = &cli.report {
        eval::report::write_report(&results, &model, &cli.provider, report_path)?;
        println!("  Report written to {}", report_path.display());
    }

    let all_passed = results.iter().all(|r| r.all_passed());
    Ok(all_passed)
}
