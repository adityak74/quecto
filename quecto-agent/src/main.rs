use quecto_agent::{cancel_token, Agent, ApprovalMode, HttpModel, Outcome};

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let auto_approve = args.iter().any(|arg| arg == "--yes");
    args.retain(|arg| arg != "--yes");
    if args.is_empty() {
        eprintln!("usage: quecto-agent [--yes] \"<task>\"");
        std::process::exit(2);
    }

    let task = args.join(" ");
    let cancel = cancel_token();
    let signal_cancel = cancel.clone();
    if let Err(e) =
        ctrlc::set_handler(move || signal_cancel.store(true, std::sync::atomic::Ordering::SeqCst))
    {
        eprintln!("quecto-agent: failed to install Ctrl-C handler: {e}");
        std::process::exit(1);
    }
    let approval = ApprovalMode::terminal(auto_approve);
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
        cancel,
        approval,
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
        Outcome::Cancelled => {
            eprintln!("quecto-agent: cancelled");
            std::process::exit(1);
        }
        Outcome::RepeatedAction => {
            eprintln!("quecto-agent: repeated action detected");
            std::process::exit(1);
        }
    }
}
