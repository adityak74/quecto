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
    /// Run harness-ablation arms over a task directory; writes episodes.jsonl.
    Ablate {
        #[arg(long, default_value = "evals/configs/arms.toml")]
        arms_file: PathBuf,
        #[arg(long, default_value = "evals/tasks")]
        tasks_dir: PathBuf,
        /// Output directory (its name becomes the experiment id).
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "target/release/quecto-agent")]
        agent_binary: PathBuf,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "http://localhost:11434/v1")]
        base_url: String,
        /// Reasoning level used by arms D and F.
        #[arg(long, default_value = "high")]
        reasoning_fixed: String,
        /// Comma-separated arm names (default: all).
        #[arg(long, value_delimiter = ',')]
        arms: Vec<String>,
        /// Comma-separated task ids (default: all).
        #[arg(long, value_delimiter = ',')]
        tasks: Vec<String>,
        #[arg(long, default_value_t = 3)]
        repeats: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
        #[arg(long, default_value_t = 30)]
        max_steps: u32,
    },
    /// Summarize an episodes.jsonl into report.md, results.csv and an SVG plot.
    Report {
        #[arg(long)]
        episodes: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}
