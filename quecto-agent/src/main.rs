use clap::{Parser, Subcommand};
use quecto_agent::{
    cancel_token, load_instructions, new_session_id, parse_command, render_change_summary,
    seed_context, Agent, ApprovalMode, ChatCommand, HttpModel, LineRenderer, Outcome, Renderer,
    SqliteRecorder, Store, Verifier,
};
use std::io::{BufRead, IsTerminal, Write};
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
    /// Start an interactive chat session.
    Chat,
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
        Some(Command::Chat) => chat(cli.yes, cli.no_verify),
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

const HELP: &str = "\
commands:
  /help              show this help
  /model             show the active model
  /context           show transcript size
  /diff              summarize this session's file changes
  /status            show session id and status
  /undo              revert the last recorded file change
  /approve           auto-approve edits and commands this session
  /deny              deny edits and commands this session
  /clear             forget the conversation (keep system prompt)
  /exit              leave chat";

fn chat(auto_approve: bool, no_verify: bool) {
    let cancel = install_cancel();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let system = compose_system(&cwd);
    let model = HttpModel::from_env();
    let model_name = std::env::var("QUECTO_MODEL").unwrap_or_default();

    let color = std::io::stdout().is_terminal();
    let approval = if auto_approve {
        ApprovalMode::AutoApprove
    } else {
        ApprovalMode::NonInteractive
    };
    let session_id = new_session_id();
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps(),
        cwd.clone(),
        cancel,
        approval,
    )
    .register_builtins()
    .with_renderer(Box::new(LineRenderer::new(std::io::stdout(), color)));
    agent = attach_verifier(agent, no_verify);

    let store = open_store();
    if let Some(s) = &store {
        if let Err(e) = s.create_session(&session_id, "chat", &cwd.display().to_string(), "") {
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

    let mut out = LineRenderer::new(std::io::stdout(), color);
    out.notice("quecto-agent chat — /help for commands, /exit to quit");

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("› ");
        let _ = std::io::stdout().flush();
        let Some(line) = lines.next() else { break };
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        match parse_command(&line) {
            ChatCommand::Exit => break,
            ChatCommand::Help => out.notice(HELP),
            ChatCommand::Model => out.notice(&format!("model: {model_name}")),
            ChatCommand::Context => {
                out.notice(&format!("session: {session_id}"));
            }
            ChatCommand::Status => {
                let status = store
                    .as_ref()
                    .and_then(|s| s.latest_session().ok().flatten())
                    .map(|r| r.status)
                    .unwrap_or_else(|| "unknown".to_string());
                out.notice(&format!("session {session_id} [{status}]"));
            }
            ChatCommand::Diff => {
                if let Some(s) = &store {
                    let changes = s.load_changes(&session_id).unwrap_or_default();
                    out.notice(render_change_summary(&changes).trim_end());
                } else {
                    out.notice("no session store");
                }
            }
            ChatCommand::Undo => chat_undo(&store, &session_id, &cwd, &mut out),
            ChatCommand::Approve => {
                agent.set_approval(ApprovalMode::AutoApprove);
                out.notice("edits and commands will be auto-approved this session");
            }
            ChatCommand::Deny => {
                agent.set_approval(ApprovalMode::NonInteractive);
                out.notice("edits and commands will be denied this session");
            }
            ChatCommand::Clear => {
                agent.clear_history();
                out.notice("conversation cleared");
            }
            ChatCommand::Unknown(name) => {
                out.notice(&format!("unknown command '/{name}' — try /help"));
            }
            ChatCommand::Say(text) => {
                if text.is_empty() {
                    continue;
                }
                match agent.run(&text) {
                    Outcome::Complete(answer) => out.assistant(&answer),
                    Outcome::StepLimit => out.notice("(step limit reached)"),
                    Outcome::Cancelled => out.notice("(cancelled)"),
                    Outcome::RepeatedAction => out.notice("(stopped: repeated action)"),
                    Outcome::Error(e) => out.notice(&format!("(error: {e})")),
                }
            }
        }
    }

    if let Some(s) = &store {
        let _ = s.set_status(&session_id, "done");
    }
    out.notice("bye");
}

fn chat_undo(
    store: &Option<Store>,
    session_id: &str,
    cwd: &Path,
    out: &mut LineRenderer<std::io::Stdout>,
) {
    let Some(store) = store else {
        out.notice("no session store");
        return;
    };
    match store.take_last_change(session_id) {
        Ok(Some(change)) => {
            let path = cwd.join(&change.path);
            let result = match &change.before {
                Some(before) => std::fs::write(&path, before),
                None => std::fs::remove_file(&path),
            };
            match result {
                Ok(()) => out.notice(&format!("reverted {}", change.path)),
                Err(e) => out.notice(&format!("could not revert {}: {e}", change.path)),
            }
        }
        Ok(None) => out.notice("no changes to undo"),
        Err(e) => out.notice(&format!("error: {e}")),
    }
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
