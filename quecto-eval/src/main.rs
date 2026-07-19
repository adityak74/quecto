use clap::Parser;
mod cli;
mod runner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    println!("Running suite: {}", args.suite);
    
    let db_path = std::path::Path::new("evals/results/telemetry.db");
    runner::init_db(db_path)?;
    println!("Database initialized.");
    
    Ok(())
}
