use quecto_agent::{cancel_token, Agent, ApprovalMode, HttpModel, Outcome};

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

    let repo_root = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let model = HttpModel::from_env();
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps,
        repo_root,
        cancel_token(),
        ApprovalMode::AutoApprove,
    )
    .register_builtins();

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
