use clap::Parser;
mod cli;
mod runner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    println!("Running suite: {}", args.suite);
    
    let db_path = std::path::Path::new("evals/results/telemetry.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _conn = runner::init_db(db_path)?;
    println!("Database initialized.");
    
    // In full implementation, loop over directories and run graders.
    Ok(())
}
