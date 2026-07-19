use clap::Parser;
mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    println!("Running suite: {}", args.suite);
    Ok(())
}
