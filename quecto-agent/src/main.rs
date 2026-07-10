use quecto_agent::{Agent, HttpModel, Outcome};

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: quecto-agent \"<task>\"");
        std::process::exit(2);
    }

    let task = args.join(" ");
    let system = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
    let max_steps = std::env::var("QUECTO_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let model = HttpModel::from_env();
    let mut agent = Agent::new(Box::new(model), system, max_steps);

    match agent.run(&task) {
        Outcome::Complete(answer) => println!("{answer}"),
        Outcome::StepLimit => {
            eprintln!("quecto-agent: step limit reached");
            std::process::exit(1);
        }
        Outcome::Error(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}
