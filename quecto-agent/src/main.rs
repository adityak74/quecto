use clap::{Parser, Subcommand};
use quecto_agent::{
    cancel_token, load_instructions, new_session_id, render_change_summary, seed_context, Agent,
    ApprovalMode, HttpModel, Outcome, SqliteRecorder, Store, Verifier,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

#[derive(Parser)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[arg(long, global = true)]
    yes: bool,
    #[arg(long, global = true)]
    no_verify: bool,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    task: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Continue a previous session by id.
    Resume { id: String },
    /// Revert the most recent recorded file change.
    Undo,
    /// Print a summary of the latest session's file changes.
    Diff,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Resume { id }) => resume(&id, cli.yes, cli.no_verify),
        Some(Command::Undo) => undo(),
        Some(Command::Diff) => diff(),
        None => {
            if cli.task.is_empty() {
                eprintln!("usage: quecto-agent [--yes] [--no-verify] \"<task>\"");
                std::process::exit(2);
            }
            run(cli.task.join(" "), cli.yes, cli.no_verify);
        }
    }
}

fn open_store() -> Option<Store> {
    match Store::open_default() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("quecto-agent: session store unavailable: {e}");
            None
        }
    }
}

fn install_cancel() -> quecto_agent::CancelToken {
    let cancel = cancel_token();
    let signal = cancel.clone();
    if let Err(e) = ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst)) {
        eprintln!("quecto-agent: failed to install Ctrl-C handler: {e}");
        std::process::exit(1);
    }
    cancel
}

fn compose_system(cwd: &Path) -> String {
    let mut system = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
    if let Some(rules) = load_instructions(cwd, cwd) {
        system.push_str("\n\n# Repository rules\n");
        system.push_str(&rules);
    }
    system.push_str("\n\n");
    system.push_str(&seed_context(cwd));
    system
}

fn max_steps() -> usize {
    std::env::var("QUECTO_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

fn attach_verifier(mut agent: Agent, no_verify: bool) -> Agent {
    if !no_verify {
        if let Some(verifier) = Verifier::from_env() {
            agent = agent.with_verifier(verifier);
        }
    }
    agent
}

fn finish(outcome: Outcome, store_status: Option<(&Store, &str)>) {
    let status = match &outcome {
        Outcome::Complete(answer) => {
            println!("{answer}");
            "done"
        }
        Outcome::StepLimit => {
            eprintln!("quecto-agent: step limit reached");
            "step_limit"
        }
        Outcome::Error(e) => {
            eprintln!("quecto-agent: {e}");
            "error"
        }
        Outcome::Cancelled => {
            eprintln!("quecto-agent: cancelled");
            "cancelled"
        }
        Outcome::RepeatedAction => {
            eprintln!("quecto-agent: repeated action detected");
            "repeated_action"
        }
    };
    if let Some((store, id)) = store_status {
        let _ = store.set_status(id, status);
    }
    if !matches!(outcome, Outcome::Complete(_)) {
        std::process::exit(1);
    }
}

fn run(task: String, auto_approve: bool, no_verify: bool) {
    let cancel = install_cancel();
    let approval = ApprovalMode::terminal(auto_approve);
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let system = compose_system(&cwd);
    let model = HttpModel::from_env();

    let session_id = new_session_id();
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps(),
        cwd.clone(),
        cancel,
        approval,
    )
    .register_builtins();
    agent = attach_verifier(agent, no_verify);

    // Attach a recorder when the store is available; the run proceeds regardless.
    let recorder_store = open_store();
    if let Some(store) = &recorder_store {
        if let Err(e) = store.create_session(&session_id, &task, &cwd.display().to_string(), "") {
            eprintln!("quecto-agent: could not create session: {e}");
        } else if let Ok(rec_store) = Store::open_default() {
            agent = agent.with_recorder(Box::new(SqliteRecorder::new(
                rec_store,
                session_id.clone(),
                0,
                0,
            )));
        }
    }

    let outcome = agent.run(&task);
    let status_target = recorder_store.as_ref().map(|s| (s, session_id.as_str()));
    finish(outcome, status_target);
}

fn resume(id: &str, auto_approve: bool, no_verify: bool) {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    let messages = match store.load_messages(id) {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => {
            eprintln!("quecto-agent: no session '{id}'");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    };
    let cancel = install_cancel();
    let approval = ApprovalMode::terminal(auto_approve);
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let model = HttpModel::from_env();

    let msg_seq = store.message_count(id).unwrap_or(0);
    let change_seq = store.change_count(id).unwrap_or(0);
    let mut agent = Agent::new(
        Box::new(model),
        String::new(),
        max_steps(),
        cwd,
        cancel,
        approval,
    )
    .register_builtins()
    .with_messages(messages);
    agent = attach_verifier(agent, no_verify);
    if let Ok(rec_store) = Store::open_default() {
        agent = agent.with_recorder(Box::new(SqliteRecorder::new(
            rec_store,
            id.to_string(),
            msg_seq,
            change_seq,
        )));
    }

    let outcome = agent.resume();
    finish(outcome, Some((&store, id)));
}

fn undo() {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    let latest = match store.latest_session() {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("quecto-agent: no sessions to undo");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    };
    match store.take_last_change(&latest.id) {
        Ok(Some(change)) => {
            let path = PathBuf::from(&latest.repo).join(&change.path);
            let result = match &change.before {
                Some(before) => std::fs::write(&path, before),
                None => std::fs::remove_file(&path),
            };
            match result {
                Ok(()) => println!("reverted {}", change.path),
                Err(e) => {
                    eprintln!("quecto-agent: could not revert {}: {e}", change.path);
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => {
            eprintln!("quecto-agent: no changes to undo");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}

fn diff() {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    match store.latest_session() {
        Ok(Some(s)) => {
            let changes = store.load_changes(&s.id).unwrap_or_default();
            print!("{}", render_change_summary(&changes));
        }
        Ok(None) => {
            eprintln!("quecto-agent: no sessions");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}
