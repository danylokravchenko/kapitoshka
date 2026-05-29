mod agent;
mod tools;
mod trace;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "kapitoshka", about = "A coding agent powered by rig")]
struct Cli {
    /// The task to perform
    #[arg(short, long)]
    task: String,

    /// Working directory for the agent
    #[arg(short, long, default_value = ".")]
    dir: String,

    /// Model to use
    #[arg(short, long, default_value = "Qwen3-0.6B")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    trace::init();

    tracing::info!(task = %cli.task, dir = %cli.dir, model = %cli.model, "starting agent");

    let result = agent::run(&cli.task, &cli.dir, &cli.model).await?;

    println!("\n{result}");

    Ok(())
}
