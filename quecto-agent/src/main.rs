use clap::{Parser, Subcommand};
use quecto_agent::{
    cancel_token, content_hash, join_url, load_instructions, new_session_id, parse_command,
    project_raw, render_change_summary, resolve_scoped, seed_context, Agent, ApprovalMode,
    ChatCommand, Flavor, HttpModel, LineRenderer, Outcome, Policy, Preset, Renderer,
    SqliteRecorder, Store, TrustStore, Verifier,
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
    #[arg(long, global = true)]
    flavor: Option<String>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    base_url: Option<String>,
    #[arg(long, global = true)]
    max_steps: Option<usize>,
    #[arg(long, global = true)]
    approval: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    task: Vec<String>,
}

struct Overrides {
    flavor: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    max_steps: Option<usize>,
    approval: Option<String>,
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
    /// Scaffold a new flavor manifest at ./.quecto/flavors/<name>.toml.
    New { name: String },
}

fn main() {
    let cli = Cli::parse();
    let overrides = Overrides {
        flavor: cli.flavor.clone(),
        model: cli.model.clone(),
        base_url: cli.base_url.clone(),
        max_steps: cli.max_steps,
        approval: cli.approval.clone(),
    };
    match cli.command {
        Some(Command::Chat) => chat(cli.yes, cli.no_verify, &overrides),
        Some(Command::Resume { id }) => resume(&id, cli.yes, cli.no_verify, &overrides),
        Some(Command::Undo) => undo(),
        Some(Command::Diff) => diff(),
        Some(Command::New { name }) => scaffold(&name),
        None => {
            if cli.task.is_empty() {
                eprintln!("usage: quecto-agent [--yes] [--no-verify] \"<task>\"");
                std::process::exit(2);
            }
            run(cli.task.join(" "), cli.yes, cli.no_verify, &overrides);
        }
    }
}

const SCAFFOLD_TEMPLATE: &str = r#"name = "{name}"

# All keys are optional; omitted keys inherit from the layer below.
# api_key is NEVER read from a manifest — set QUECTO_API_KEY in the environment.
# model         = "qwen3.6:35b"
# base_url      = "http://localhost:11434/v1"
# max_steps     = 30
# auto_verify   = true
# system_prompt = "You are a terse senior reviewer."

[tools]
# Allow-list over all built-in tools. Omit to enable all.
# enabled = ["read_file", "search_text", "list_files", "git_diff"]

[approval]
# preset = "read-only"   # read-only | editor | full
# run_command = "ask"    # allow | ask | deny

[verify]
# Commands run as a completion gate (project flavors require trust-on-first-use).
# test = "cargo test"
# lint = "cargo clippy -- -D warnings"
"#;

fn scaffold(name: &str) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let dir = cwd.join(".quecto").join("flavors");
    let path = dir.join(format!("{name}.toml"));
    if path.exists() {
        eprintln!("quecto-agent: {} already exists", path.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("quecto-agent: {e}");
        std::process::exit(1);
    }
    let body = SCAFFOLD_TEMPLATE.replace("{name}", name);
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("quecto-agent: {e}");
        std::process::exit(1);
    }
    println!("created {}", path.display());
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

fn compose_system_with_persona(cwd: &Path, persona: Option<&str>) -> String {
    let mut system = String::new();
    if let Some(p) = persona {
        if !p.trim().is_empty() {
            system.push_str("# Persona\n");
            system.push_str(p.trim());
            system.push_str("\n\n");
        }
    }
    system.push_str(&compose_system(cwd));
    system
}

fn resolve_flavor(overrides: &Overrides) -> (Flavor, Flavor) {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match resolve_scoped(&home, &cwd, overrides.flavor.as_deref()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("quecto-agent: flavor error: {e}");
            std::process::exit(1);
        }
    }
}

/// Return the flavor whose command-bearing/loosening fields may be applied:
/// `user ⊕ project` when the project flavor is trusted (or needs no privilege),
/// otherwise `user` alone. Prompts on a TTY; non-interactive denies; `--yes`
/// trusts and records.
fn gated_flavor(
    user: &Flavor,
    project: &Flavor,
    flavor_name: Option<&str>,
    auto_approve: bool,
) -> Flavor {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let Some(raw) = project_raw(&home, &cwd, flavor_name) else {
        return user.clone();
    };
    if !project.wants_privilege() {
        // Only safe project fields exist; nothing to gate for policy/verify.
        return user.clone();
    }
    let hash = content_hash(&raw);
    let mut store = TrustStore::open();
    if store.is_trusted(&hash) {
        return user.clone().merge(project.clone());
    }
    let trusted = auto_approve || prompt_trust(project);
    if trusted {
        store.trust(&hash);
    } else {
        eprintln!(
            "quecto-agent: project flavor not trusted; its verify/approval settings are ignored"
        );
    }
    if trusted {
        user.clone().merge(project.clone())
    } else {
        user.clone()
    }
}

/// Ask the human to approve a project flavor. Denies unless stdin is a TTY and
/// the answer is y/yes.
fn prompt_trust(project: &Flavor) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprintln!("⚠  ./.quecto/flavor.toml is new/changed and wants to:");
    for line in project.privilege_summary() {
        eprintln!("     • {line}");
    }
    eprint!("   Allow this project flavor? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn pick(flag: Option<&str>, env: &str, flavor: Option<&str>, default: &str) -> String {
    flag.map(str::to_string)
        .or_else(|| std::env::var(env).ok().filter(|s| !s.is_empty()))
        .or_else(|| flavor.map(str::to_string))
        .unwrap_or_else(|| default.to_string())
}

fn build_policy(flag: Option<&str>, user: &Flavor) -> Policy {
    let preset_name = flag
        .map(str::to_string)
        .or_else(|| user.approval.preset.clone());
    let mut policy = match preset_name.as_deref().and_then(Preset::parse) {
        Some(p) => Policy::from_preset(p),
        None => Policy::default(),
    };
    for (op, decision) in &user.approval.overrides {
        policy = policy.with_override(op, decision);
    }
    policy
}

fn persona(cwd: &Path, flavor: &Flavor) -> Option<String> {
    flavor.system_prompt.clone().or_else(|| {
        flavor
            .system_prompt_file
            .as_deref()
            .and_then(|path| std::fs::read_to_string(cwd.join(path)).ok())
    })
}

fn attach_verifier(mut agent: Agent, no_verify: bool, user_flavor: &Flavor) -> Agent {
    if !no_verify {
        let commands = user_flavor.verify_commands();
        if !commands.is_empty() {
            agent = agent.with_verifier(Verifier::new(commands));
        } else if let Some(verifier) = Verifier::from_env() {
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

fn run(task: String, auto_approve: bool, no_verify: bool, overrides: &Overrides) {
    let cancel = install_cancel();
    let approval = ApprovalMode::terminal(auto_approve);
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let (user_flavor, project_flavor) = resolve_flavor(overrides);
    let gated = gated_flavor(
        &user_flavor,
        &project_flavor,
        overrides.flavor.as_deref(),
        auto_approve,
    );
    let merged = user_flavor.clone().merge(project_flavor);
    let system = compose_system_with_persona(&cwd, persona(&cwd, &merged).as_deref());
    let base_url = pick(
        overrides.base_url.as_deref(),
        "QUECTO_BASE_URL",
        merged.base_url.as_deref(),
        "http://localhost:11434/v1",
    );
    let model_name = pick(
        overrides.model.as_deref(),
        "QUECTO_MODEL",
        merged.model.as_deref(),
        "",
    );
    let api_key = std::env::var("QUECTO_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let model = HttpModel {
        url: join_url(&base_url, "chat/completions"),
        api_key,
        model: model_name,
    };
    let steps = overrides
        .max_steps
        .or_else(|| {
            std::env::var("QUECTO_MAX_STEPS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or(merged.max_steps)
        .unwrap_or(20);

    let session_id = new_session_id();
    let mut agent = Agent::new(
        Box::new(model),
        system,
        steps,
        cwd.clone(),
        cancel,
        approval,
    )
    .register_builtins_filtered(merged.tools.enabled.as_deref())
    .with_policy(build_policy(overrides.approval.as_deref(), &gated));
    agent = attach_verifier(agent, no_verify, &gated);

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

fn chat(auto_approve: bool, no_verify: bool, overrides: &Overrides) {
    let cancel = install_cancel();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let (user_flavor, project_flavor) = resolve_flavor(overrides);
    let gated = gated_flavor(
        &user_flavor,
        &project_flavor,
        overrides.flavor.as_deref(),
        auto_approve,
    );
    let merged = user_flavor.clone().merge(project_flavor);
    let system = compose_system_with_persona(&cwd, persona(&cwd, &merged).as_deref());
    let base_url = pick(
        overrides.base_url.as_deref(),
        "QUECTO_BASE_URL",
        merged.base_url.as_deref(),
        "http://localhost:11434/v1",
    );
    let model_name = pick(
        overrides.model.as_deref(),
        "QUECTO_MODEL",
        merged.model.as_deref(),
        "",
    );
    let model = HttpModel {
        url: join_url(&base_url, "chat/completions"),
        api_key: std::env::var("QUECTO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        model: model_name.clone(),
    };
    let steps = overrides
        .max_steps
        .or_else(|| {
            std::env::var("QUECTO_MAX_STEPS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or(merged.max_steps)
        .unwrap_or(20);

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
        steps,
        cwd.clone(),
        cancel,
        approval,
    )
    .register_builtins_filtered(merged.tools.enabled.as_deref())
    .with_policy(build_policy(overrides.approval.as_deref(), &gated))
    .with_renderer(Box::new(LineRenderer::new(std::io::stdout(), color)));
    agent = attach_verifier(agent, no_verify, &gated);

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

fn resume(id: &str, auto_approve: bool, no_verify: bool, overrides: &Overrides) {
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
    let (user_flavor, project_flavor) = resolve_flavor(overrides);
    let gated = gated_flavor(
        &user_flavor,
        &project_flavor,
        overrides.flavor.as_deref(),
        auto_approve,
    );
    let merged = user_flavor.clone().merge(project_flavor);
    let base_url = pick(
        overrides.base_url.as_deref(),
        "QUECTO_BASE_URL",
        merged.base_url.as_deref(),
        "http://localhost:11434/v1",
    );
    let model_name = pick(
        overrides.model.as_deref(),
        "QUECTO_MODEL",
        merged.model.as_deref(),
        "",
    );
    let model = HttpModel {
        url: join_url(&base_url, "chat/completions"),
        api_key: std::env::var("QUECTO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        model: model_name,
    };
    let steps = overrides
        .max_steps
        .or_else(|| {
            std::env::var("QUECTO_MAX_STEPS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or(merged.max_steps)
        .unwrap_or(20);

    let msg_seq = store.message_count(id).unwrap_or(0);
    let change_seq = store.change_count(id).unwrap_or(0);
    let mut agent = Agent::new(Box::new(model), String::new(), steps, cwd, cancel, approval)
        .register_builtins_filtered(merged.tools.enabled.as_deref())
        .with_policy(build_policy(overrides.approval.as_deref(), &gated))
        .with_messages(messages);
    agent = attach_verifier(agent, no_verify, &gated);
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
