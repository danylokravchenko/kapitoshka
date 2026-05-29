mod agent;
mod context;
mod permission;
mod session;
mod tools;
mod trace;
mod ui;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "kapitoshka", about = "A coding agent powered by rig")]
struct Cli {
    /// Working directory for the agent
    #[arg(short, long, default_value = ".")]
    dir: String,

    /// Model to use
    #[arg(short, long, default_value = "Qwen3-0.6B")]
    model: String,

    /// Provider backend: "openai" (default, works with any OpenAI-compatible server) or "anthropic"
    #[arg(short, long, default_value = "openai")]
    provider: String,

    /// Show the model's internal reasoning/thinking if the model supports it
    #[arg(long)]
    thinking: bool,

    /// Context window size in tokens used to display fill percentage (e.g. 131072)
    #[arg(long, default_value = "0")]
    context_size: u64,

    /// Resume a previous session from a saved state file (.json sidecar next to the session log)
    #[arg(long)]
    resume: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let _trace_guards = trace::init();

    // Short hex ID derived from the current timestamp for log correlation.
    let session_id = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );

    tracing::info!(
        session = %session_id,
        dir = %cli.dir,
        model = %cli.model,
        thinking = cli.thinking,
        "starting agent"
    );

    agent::run_interactive(
        &cli.dir,
        &cli.model,
        &cli.provider,
        cli.thinking,
        cli.context_size,
        &session_id,
        cli.resume.as_deref(),
    )
    .await
}
