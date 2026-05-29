mod agent;
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

    /// Show the model's internal reasoning/thinking if the model supports it
    #[arg(long)]
    thinking: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let _trace_guard = trace::init();

    tracing::info!(dir = %cli.dir, model = %cli.model, thinking = cli.thinking, "starting agent");

    agent::run_interactive(&cli.dir, &cli.model, cli.thinking).await
}
