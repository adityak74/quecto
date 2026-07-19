use clap::Parser;
mod cli;
mod runner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    println!("Running suite: {}", args.suite);
    
    let db_path = std::path::Path::new("evals/results/telemetry.db");
    
    // Use the newly implemented run_suite
    runner::run_suite(&args.suite, db_path)?;
    
    println!("Suite run completed.");
    Ok(())
}
