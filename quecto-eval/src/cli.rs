use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the existing grader-based eval suite.
    Eval {
        #[arg(short, long)]
        suite: String,
    },
    /// Run a manifest-driven paired behavioral-compatibility experiment.
    Compat {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        tasks_dir: PathBuf,
        #[arg(long, default_value = "evals/results/telemetry.db")]
        db: PathBuf,
        #[arg(long, default_value = "../target/release/quecto-agent")]
        agent_binary: PathBuf,
    },
}
