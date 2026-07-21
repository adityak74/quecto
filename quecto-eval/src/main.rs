use clap::Parser;
mod cli;
mod contracts;
mod manifest;
mod runner;
mod snapshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    match args.command {
        cli::Command::Eval { suite } => {
            println!("Running suite: {suite}");
            let db_path = std::path::Path::new("evals/results/telemetry.db");
            runner::init_db(db_path)?;
            println!("Database initialized.");
        }
        cli::Command::Compat {
            manifest,
            tasks_dir,
            db,
            agent_binary,
        } => {
            runner::run_suite(&manifest, &tasks_dir, &db, &agent_binary)?;
            println!("Compatibility experiment complete. Results in {}", db.display());
        }
    }
    Ok(())
}
