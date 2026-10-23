use clap::Parser;
use quecto_eval::runner;
mod cli;

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
        cli::Command::Ablate {
            arms_file, tasks_dir, out, agent_binary, model, base_url, reasoning_fixed,
            arms, tasks, repeats, seed, timeout_secs, max_steps,
        } => quecto_eval::ablate::run(&quecto_eval::ablate::Opts {
            arms_file, tasks_dir, out_dir: out, agent_binary, model, base_url,
            reasoning_fixed, arms, tasks, repeats, seed, timeout_secs, max_steps,
        })?,
        cli::Command::Report { episodes, out } => {
            quecto_eval::report::report(&episodes, &out)?;
            println!("Report written to {}", out.display());
        }
    }
    Ok(())
}
