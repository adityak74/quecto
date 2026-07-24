use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};
use quecto_agent::{
    cancel_token, chat_spinner_renderer, content_hash, default_user_capsules_dir,
    join_url, load_instructions, new_session_id, parse_command, parse_spinner_verbs,
    project_capsules_dir, project_raw, render_assistant_text, render_change_summary,
    resolve_scoped_configured, seed_context, Agent, ApprovalMode, CapsuleRegistry, CapsuleState,
    ChatCommand, ConfiguredFlavor, Flavor, HttpModel, LineRenderer, Message, Outcome, Policy,
    Preset, Provider, ReasoningCommand, ReasoningMode, Renderer, SqliteRecorder, Store,
    TrustStore, Verifier,
};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[cfg(feature = "otel")]
mod otel_init {
    pub struct OtelGuard {
        _rt: tokio::runtime::Runtime,
    }

    impl Drop for OtelGuard {
        fn drop(&mut self) {
            opentelemetry::global::shutdown_tracer_provider();
        }
    }

    pub fn init_otel() -> Option<OtelGuard> {
        // gRPC/HTTP OTLP exporter batch processor runs asynchronously.
        // Create a dedicated single-threaded runtime to orchestrate exports.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .ok()?;

        let _guard = rt.enter();

        let _ = opentelemetry::global::set_error_handler(|_| {});

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(opentelemetry_otlp::new_exporter().http())
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .ok()?;

        use tracing_subscriber::prelude::*;
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = tracing_subscriber::registry().with(telemetry);

        tracing::subscriber::set_global_default(subscriber).ok()?;

        Some(OtelGuard { _rt: rt })
    }
}

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

#[derive(Parser)]
#[command(version)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    /// Approve trusted prompts without asking.
    #[arg(long, global = true)]
    yes: bool,
    /// Skip configured verification commands.
    #[arg(long, global = true)]
    no_verify: bool,
    /// Select a named flavor profile.
    #[arg(long, global = true)]
    flavor: Option<String>,
    /// Override the model name.
    #[arg(long, global = true)]
    model: Option<String>,
    /// Override the OpenAI-compatible base URL.
    #[arg(long, global = true)]
    base_url: Option<String>,
    /// Select the provider wire format: "openai" (default) or "anthropic".
    #[arg(long, global = true)]
    provider: Option<String>,
    /// Override the max_tokens sent to Anthropic requests (ignored for openai).
    #[arg(long, global = true)]
    max_tokens: Option<u32>,
    /// Limit the number of agent steps.
    #[arg(long, global = true)]
    max_steps: Option<usize>,
    /// Select the approval preset.
    #[arg(long, global = true)]
    approval: Option<String>,
    /// Connect to an MCP server. Format: stdio:name:command[:arg1:arg2...]
    /// or streamable_http:name:url  or  sse:name:url (legacy).
    /// Can be specified multiple times. Requires --features mcp build.
    #[cfg(feature = "mcp")]
    #[arg(long = "mcp", global = true, value_name = "TRANSPORT:NAME:...")]
    mcp: Vec<String>,
    /// Attach image file(s) to the prompt. Can be specified multiple times.
    #[arg(long = "image", global = true, value_name = "PATH")]
    images: Vec<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    task: Vec<String>,
}

struct Overrides {
    flavor: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    provider: Option<String>,
    max_tokens: Option<u32>,
    max_steps: Option<usize>,
    approval: Option<String>,
    #[cfg(feature = "mcp")]
    mcp: Vec<String>,
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

    #[cfg(feature = "otel")]
    let _otel_guard = match &cli.command {
        Some(Command::Chat) | Some(Command::Resume { .. }) | None => otel_init::init_otel(),
        _ => None,
    };
    let overrides = Overrides {
        flavor: cli.flavor.clone(),
        model: cli.model.clone(),
        base_url: cli.base_url.clone(),
        provider: cli.provider.clone(),
        max_tokens: cli.max_tokens,
        max_steps: cli.max_steps,
        approval: cli.approval.clone(),
        #[cfg(feature = "mcp")]
        mcp: cli.mcp,
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
            if let Some(flag) = cli.task.first().filter(|arg| arg.starts_with("--")) {
                Cli::command()
                    .error(
                        ErrorKind::UnknownArgument,
                        format!("unexpected argument '{flag}' found"),
                    )
                    .exit();
            }
            run(cli.task.join(" "), &cli.images, cli.yes, cli.no_verify, &overrides);
        }
    }
}

const SCAFFOLD_TEMPLATE: &str = r#"name = "{name}"

# All keys are optional; omitted keys inherit from the layer below.
# api_key is NEVER read from a manifest — set QUECTO_API_KEY in the environment.
# model         = "qwen3.6:35b"
# base_url      = "http://localhost:11434/v1"
# provider      = "openai"  # openai | anthropic
# max_tokens    = 4096      # required by Anthropic; ignored for openai
# reasoning_mode  = "low"
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

fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
    .to_string()
}

fn is_image_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// Parse `@image <path>` or `@img <path>` references from text.
/// Returns (cleaned_text, vec of (image_data, mime_type)).
fn extract_image_refs(text: &str, cwd: &Path) -> (String, Vec<(Vec<u8>, String)>) {
    let re = regex::Regex::new(r"@(?:image|img)\s+(\S+)").unwrap();
    let mut images = Vec::new();
    let cleaned = re.replace_all(text, |caps: &regex::Captures| {
        let raw_path = &caps[1];
        let path = cwd.join(raw_path);
        match std::fs::read(&path) {
            Ok(data) => {
                let mime = mime_from_extension(&path);
                images.push((data, mime));
                format!("[Image {}]", images.len())
            }
            Err(e) => {
                eprintln!("quecto-agent: cannot read {}: {e}", path.display());
                caps[0].to_string()
            }
        }
    });
    (cleaned.to_string(), images)
}

enum Segment {
    Text(String),
    Paste(String),
    Image {
        data: Vec<u8>,
        mime_type: String,
        index: usize,
    },
}

fn segments_to_parts(segments: &[Segment], cwd: &Path) -> Vec<quecto_agent::ContentPart> {
    use quecto_agent::ContentPart;
    let mut parts: Vec<ContentPart> = Vec::new();
    let mut text_buf = String::new();
    for seg in segments {
        match seg {
            Segment::Text(t) => text_buf.push_str(t),
            Segment::Paste(s) => text_buf.push_str(s),
            Segment::Image { data, mime_type, .. } => {
                if !text_buf.is_empty() {
                    parts.push(ContentPart::Text(
                        std::mem::take(&mut text_buf),
                    ));
                }
                parts.push(ContentPart::Image {
                    data: data.clone(),
                    mime_type: mime_type.clone(),
                });
            }
        }
    }
    // Process @image refs in remaining text
    if !text_buf.is_empty() {
        let (cleaned, img_refs) = extract_image_refs(&text_buf, cwd);
        if !cleaned.trim().is_empty() {
            parts.push(ContentPart::Text(cleaned));
        }
        for (data, mime) in img_refs {
            parts.push(ContentPart::Image {
                data,
                mime_type: mime,
            });
        }
    }
    parts
}

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

fn resolve_flavor(overrides: &Overrides) -> (ConfiguredFlavor, ConfiguredFlavor) {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match resolve_scoped_configured(&home, &cwd, overrides.flavor.as_deref()) {
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
    user: &ConfiguredFlavor,
    project: &ConfiguredFlavor,
    flavor_name: Option<&str>,
    auto_approve: bool,
) -> ConfiguredFlavor {
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
            let rendered = render_assistant_text(answer, std::io::stdout().is_terminal());
            println!("{rendered}");
            "done"
        }
        Outcome::StepLimit => {
            eprintln!("quecto-agent: step limit reached");
            "step_limit"
        }
        Outcome::VerificationFailed { attempts } => {
            eprintln!("quecto-agent: verification still failing after {attempts} attempts");
            "verification_failed"
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
        Outcome::Blocked => {
            eprintln!(
                "quecto-agent: stopped — several actions were denied. Re-run with --yes to \
                 auto-approve edits and commands, or set an approval preset in a flavor."
            );
            "blocked"
        }
    };
    if let Some((store, id)) = store_status {
        let _ = store.set_status(id, status);
    }
    if !matches!(outcome, Outcome::Complete(_)) {
        std::process::exit(1);
    }
}

#[derive(serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(serde::Deserialize)]
struct OllamaModel {
    name: String,
}

fn resolve_host_and_model(overrides: &Overrides, merged: &Flavor) -> (String, String) {
    let base_url = pick(
        overrides.base_url.as_deref(),
        "QUECTO_BASE_URL",
        merged.base_url.as_deref(),
        "http://localhost:11434/v1",
    );
    let mut model_name = pick(
        overrides.model.as_deref(),
        "QUECTO_MODEL",
        merged.model.as_deref(),
        "",
    );

    if model_name.is_empty() && base_url.contains("localhost:11434") {
        let tags_url = base_url.replace("/v1", "/api/tags");
        if let Ok(res) = ureq::get(&tags_url).call() {
            if let Ok(json) = res.into_json::<OllamaTagsResponse>() {
                if !json.models.is_empty() {
                    eprintln!("No model specified. Available Ollama models:");
                    for (i, model) in json.models.iter().enumerate() {
                        eprintln!("  {}) {}", i + 1, model.name);
                    }
                    eprint!("Select a model (1-{}): ", json.models.len());
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    if std::io::stdin().read_line(&mut input).is_ok() {
                        let input = input.trim();
                        if let Ok(idx) = input.parse::<usize>() {
                            if idx > 0 && idx <= json.models.len() {
                                model_name = json.models[idx - 1].name.clone();
                                eprintln!("Selected model: {}", model_name);
                            }
                        }
                    }
                }
            }
        }
    }

    (base_url, model_name)
}

fn resolve_provider(overrides: &Overrides, merged: &Flavor) -> Result<Provider, quecto_agent::BoxErr> {
    if let Some(flag) = &overrides.provider {
        return flag.parse();
    }
    if let Ok(env) = std::env::var("QUECTO_PROVIDER") {
        if !env.is_empty() {
            return env.parse();
        }
    }
    Ok(merged.provider.unwrap_or_default())
}

fn resolve_max_tokens(overrides: &Overrides, merged: &Flavor) -> Option<u32> {
    overrides
        .max_tokens
        .or_else(|| {
            std::env::var("QUECTO_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or(merged.max_tokens)
}

fn run(task: String, images: &[PathBuf], auto_approve: bool, no_verify: bool, overrides: &Overrides) {
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
    let (base_url, model_name) = resolve_host_and_model(overrides, &merged);
    let provider = resolve_provider(overrides, &merged).unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
    let api_key = std::env::var("QUECTO_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let model = HttpModel {
        url: join_url(&base_url, provider.path_suffix()),
        api_key,
        model: model_name,
        provider,
        max_tokens: resolve_max_tokens(overrides, &merged),
    }
    .try_with_env_reasoning_mode(merged.reasoning_mode)
    .unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
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

    agent = attach_mcp_tools(agent, overrides, true);

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

    if images.is_empty() {
        let outcome = agent.run(&task);
        let status_target = recorder_store.as_ref().map(|s| (s, session_id.as_str()));
        finish(outcome, status_target);
    } else {
        use quecto_agent::ContentPart;
        let mut parts: Vec<ContentPart> = Vec::new();
        for path in images {
            let data = std::fs::read(path).unwrap_or_else(|e| {
                eprintln!("quecto-agent: cannot read image {}: {e}", path.display());
                std::process::exit(2);
            });
            let mime_type = mime_from_extension(path);
            parts.push(ContentPart::Image { data, mime_type });
        }
        parts.push(ContentPart::Text(task));
        let outcome = agent.run_multimodal(parts);
        let status_target = recorder_store.as_ref().map(|s| (s, session_id.as_str()));
        finish(outcome, status_target);
    }
}

const HELP: &str = "\
/commands            list available tools (same as /tools)
/exit, /quit, /q     leave chat
/help, /h, /?        show this help
/model               show the active model
/context             show transcript size
/diff                summarize this session's file changes
/status              show session id and status
/undo                revert the last recorded file change
/approve             auto-approve edits and commands this session
/deny                deny edits and commands this session
/clear               forget the conversation (keep system prompt)
/reasoning           show the active session reasoning mode
/reasoning <mode>    set reasoning mode for future turns in this session
/capsules            list available and loaded capsules
/load <name>         load a capsule
/unload <name>       unload a capsule
/<capsule_name> [text]  load a capsule (if needed) and optionally send a prompt through it

Images:
  @image <path>      attach an image file to your prompt
  @img <path>        alias for @image
  Ctrl+V             paste image from clipboard (requires --features clipboard)
  Drag & drop        drag an image file into the terminal
  --image <path>     attach image in one-shot mode (repeatable)";

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
    let user_capsules_dir = default_user_capsules_dir().unwrap_or_default();
    let project_capsules_dir_path = project_capsules_dir(&cwd);
    let capsule_registry = CapsuleRegistry::discover(&user_capsules_dir, &project_capsules_dir_path);
    let mut capsules = CapsuleState::new(capsule_registry, system.clone());
    let (base_url, model_name) = resolve_host_and_model(overrides, &merged);
    let provider = resolve_provider(overrides, &merged).unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
    let model = HttpModel {
        url: join_url(&base_url, provider.path_suffix()),
        api_key: std::env::var("QUECTO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        model: model_name.clone(),
        provider,
        max_tokens: resolve_max_tokens(overrides, &merged),
    }
    .try_with_env_reasoning_mode(merged.reasoning_mode)
    .unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
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
    let spinner_verbs = parse_spinner_verbs(std::env::var("QUECTO_SPINNER_VERBS").ok().as_deref());
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
    .with_renderer(if color {
        chat_spinner_renderer(spinner_verbs)
    } else {
        Box::new(LineRenderer::new(std::io::stdout(), color))
    });

    agent = attach_mcp_tools(agent, overrides, true);

    agent = attach_verifier(agent, no_verify, &gated);

    let store = open_store();
    if let Some(s) = &store {
        if let Err(e) = s.create_session_with_reasoning_mode(
            &session_id,
            "chat",
            &cwd.display().to_string(),
            "",
            merged.reasoning_mode,
        ) {
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
    if !stdin.is_terminal() {
        let mut lines = stdin.lock().lines();
        loop {
            print!("› ");
            let _ = std::io::stdout().flush();
            let Some(line) = lines.next() else { break };
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let exit = handle_chat_command(
                &line,
                &mut agent,
                &store,
                &session_id,
                &cwd,
                &model_name,
                &mut capsules,
                &mut out,
            );
            if exit {
                break;
            }
        }
        if let Some(s) = &store {
            let _ = s.set_status(&session_id, "done");
        }
        out.notice("bye");
        return;
    }

    let mut segments = vec![Segment::Text(String::new())];
    let mut image_counter: usize = 0;
    let mut redraw = true;

    crossterm::terminal::enable_raw_mode().unwrap();
    #[cfg(feature = "clipboard")]
    let mut clipboard = arboard::Clipboard::new().ok();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

    loop {
        if redraw {
            let mut prompt = String::from("\r› ");
            for seg in &segments {
                match seg {
                    Segment::Text(t) => prompt.push_str(t),
                    Segment::Paste(s) => {
                        prompt.push_str(&format!("[pasted +{} characters]", s.len()))
                    }
                    Segment::Image { index, .. } => {
                        prompt.push_str(&format!("[Image {}]", index))
                    }
                }
            }
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
            );
            print!("{}", prompt);
            let _ = std::io::stdout().flush();
            redraw = false;
        }

        if let Ok(event) = crossterm::event::read() {
            match event {
                crossterm::event::Event::Key(key)
                    if key.kind == crossterm::event::KeyEventKind::Press =>
                {
                    match key.code {
                        crossterm::event::KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            println!("\r");
                            break;
                        }
                        crossterm::event::KeyCode::Char('d')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            println!("\r");
                            break;
                        }
                        crossterm::event::KeyCode::Enter => {
                            println!("\r");
                            // Convert segments to content parts
                            use quecto_agent::ContentPart;
                            let parts = segments_to_parts(&segments, &cwd);

                            segments.clear();
                            segments.push(Segment::Text(String::new()));
                            image_counter = 0;

                            let _ = crossterm::execute!(
                                std::io::stdout(),
                                crossterm::event::DisableBracketedPaste
                            );
                            let _ = crossterm::terminal::disable_raw_mode();

                            // Determine if we have content to send
                            let has_content = parts.iter().any(|p| match p {
                                ContentPart::Text(t) => !t.trim().is_empty(),
                                ContentPart::Image { .. } => true,
                            });

                            if has_content {
                                // Check if it's a command (first text part)
                                let first_text = parts.iter().find_map(|p| match p {
                                    ContentPart::Text(t) => Some(t.as_str()),
                                    _ => None,
                                }).unwrap_or("");

                                if first_text.trim_start().starts_with('/') && !parts.iter().any(|p| matches!(p, ContentPart::Image { .. })) {
                                    // Pure text command — use existing command handler
                                    let exit = handle_chat_command(
                                        first_text, &mut agent, &store, &session_id,
                                        &cwd, &model_name, &mut capsules, &mut out,
                                    );
                                    if exit { break; }
                                } else {
                                    // Multimodal or text message
                                    match agent.run_multimodal(parts) {
                                        Outcome::Complete(answer) => out.assistant(&answer),
                                        Outcome::StepLimit => out.notice("(step limit reached)"),
                                        Outcome::VerificationFailed { attempts } => out.notice(
                                            &format!("(verification still failing after {attempts} attempts)"),
                                        ),
                                        Outcome::Cancelled => out.notice("(cancelled)"),
                                        Outcome::RepeatedAction => out.notice("(stopped: repeated action)"),
                                        Outcome::Blocked => out.notice(
                                            "(stopped: actions denied — use /approve to allow this session)",
                                        ),
                                        Outcome::Error(e) => out.notice(&format!("(error: {e})")),
                                    }
                                }
                            }

                            crossterm::terminal::enable_raw_mode().unwrap();
                            let _ = crossterm::execute!(
                                std::io::stdout(),
                                crossterm::event::EnableBracketedPaste
                            );
                            redraw = true;
                        }
                        crossterm::event::KeyCode::Backspace => {
                            let mut pop_segment = false;
                            if let Some(last) = segments.last_mut() {
                                match last {
                                    Segment::Text(t) => {
                                        if !t.is_empty() {
                                            t.pop();
                                        } else {
                                            pop_segment = true;
                                        }
                                    }
                                    Segment::Paste(_) => {
                                        pop_segment = true;
                                    }
                                    Segment::Image { .. } => {
                                        pop_segment = true;
                                    }
                                }
                            }
                            if pop_segment {
                                segments.pop();
                            }
                            if segments.is_empty() {
                                segments.push(Segment::Text(String::new()));
                            }
                            redraw = true;
                        }
                        #[cfg(feature = "clipboard")]
                        crossterm::event::KeyCode::Char('v')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                                || key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::SUPER) =>
                        {
                            let mut used_clipboard = false;
                            if let Some(ref mut cb) = clipboard {
                                if let Ok(img) = cb.get_image() {
                                    // Encode RGBA to PNG
                                    let mut png_buf = Vec::new();
                                    if let Ok(()) = {
                                        let encoder = image::codecs::png::PngEncoder::new(
                                            std::io::Cursor::new(&mut png_buf),
                                        );
                                        image::ImageEncoder::write_image(
                                            encoder,
                                            &img.bytes,
                                            img.width as u32,
                                            img.height as u32,
                                            image::ExtendedColorType::Rgba8,
                                        )
                                    } {
                                        image_counter += 1;
                                        segments.push(Segment::Image {
                                            data: png_buf,
                                            mime_type: "image/png".into(),
                                            index: image_counter,
                                        });
                                        segments.push(Segment::Text(String::new()));
                                        used_clipboard = true;
                                    }
                                }
                            }
                            if !used_clipboard {
                                // Fall through to normal 'v' character
                                if let Some(Segment::Text(t)) = segments.last_mut() {
                                    t.push('v');
                                } else {
                                    segments.push(Segment::Text("v".to_string()));
                                }
                            }
                            redraw = true;
                        }
                        crossterm::event::KeyCode::Char(c) => {
                            if let Some(Segment::Text(t)) = segments.last_mut() {
                                t.push(c);
                            } else {
                                segments.push(Segment::Text(c.to_string()));
                            }
                            redraw = true;
                        }
                        _ => {}
                    }
                }
                crossterm::event::Event::Paste(s) => {
                    let trimmed = s.trim().trim_matches('\'').trim_matches('"');
                    let path = std::path::Path::new(trimmed);
                    if path.is_file() && is_image_extension(path) {
                        if let Ok(data) = std::fs::read(path) {
                            image_counter += 1;
                            let mime_type = mime_from_extension(path);
                            segments.push(Segment::Image {
                                data,
                                mime_type,
                                index: image_counter,
                            });
                            segments.push(Segment::Text(String::new()));
                        } else {
                            segments.push(Segment::Paste(s));
                            segments.push(Segment::Text(String::new()));
                        }
                    } else {
                        segments.push(Segment::Paste(s));
                        segments.push(Segment::Text(String::new()));
                    }
                    redraw = true;
                }
                _ => {}
            }
        }
    }

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    let _ = crossterm::terminal::disable_raw_mode();

    if let Some(s) = &store {
        let _ = s.set_status(&session_id, "done");
    }
    out.notice("bye");
}
/// Dispatch one parsed chat-REPL command line against the running agent and
/// session. Shared by the non-TTY (piped stdin) and TTY (raw-mode) input
/// loops in `chat`, which otherwise duplicated this match verbatim. Returns
/// `true` if the REPL should exit.
fn handle_chat_command(
    line: &str,
    agent: &mut Agent,
    store: &Option<Store>,
    session_id: &str,
    cwd: &Path,
    model_name: &str,
    capsules: &mut CapsuleState,
    out: &mut dyn Renderer,
) -> bool {
    let mut exit = false;
    let capsule_names = capsules.registry().names();
    match parse_command(line, &capsule_names) {
        ChatCommand::Exit => exit = true,
        ChatCommand::Help => out.notice(HELP),
        ChatCommand::Model => {
            if model_name.is_empty() {
                out.notice("model: (not set)");
            } else {
                out.notice(&format!("model: {model_name}"));
            }
        }
        ChatCommand::Context => {
            let msg_n = agent.messages.len().saturating_sub(1);
            let char_count: usize = agent
                .messages
                .iter()
                .map(|m| {
                    m.text().len()
                        + m.tool_calls
                            .iter()
                            .map(|tc| tc.name.len() + tc.arguments.to_string().len())
                            .sum::<usize>()
                })
                .sum();
            out.notice(&format!(
                "session: {} ({} messages, ~{} chars)",
                session_id, msg_n, char_count
            ));
        }
        ChatCommand::Status => {
            let status = store
                .as_ref()
                .and_then(|s| s.session_status(session_id).ok().flatten())
                .unwrap_or_else(|| "unknown".to_string());
            let bg_count = agent.background_process_count();
            if bg_count > 0 {
                let plural = if bg_count == 1 {
                    "process"
                } else {
                    "processes"
                };
                out.notice(&format!(
                    "session {session_id} [{status}] ({} background {} running)",
                    bg_count, plural
                ));
            } else {
                out.notice(&format!("session {session_id} [{status}]"));
            }
        }
        ChatCommand::Diff => {
            if let Some(s) = &store {
                let changes = s.load_changes(session_id).unwrap_or_default();
                out.notice(render_change_summary(&changes).trim_end());
            } else {
                out.notice("no session store");
            }
        }
        ChatCommand::Undo => chat_undo(store, session_id, cwd, out),
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
            out.notice(&format!("session {} conversation cleared", session_id));
        }
        ChatCommand::Tools => {
            out.notice(&agent.tool_names().join("\n"));
        }
        ChatCommand::Reasoning(ReasoningCommand::Show) => {
            out.notice(&format!(
                "reasoning: {}",
                agent
                    .session_reasoning_mode()
                    .map(|mode| mode.effort_str())
                    .unwrap_or("off")
            ));
        }
        ChatCommand::Reasoning(ReasoningCommand::Set(raw)) => {
            let mode = if raw.eq_ignore_ascii_case("off") {
                None
            } else {
                match raw.parse::<ReasoningMode>() {
                    Ok(mode) => Some(mode),
                    Err(_) => {
                        out.notice(&format!("unknown reasoning mode: {raw}"));
                        return exit;
                    }
                }
            };
            if let Err(e) = agent.set_session_reasoning_mode(mode) {
                out.notice(&format!("error: {e}"));
                return exit;
            }
            if let Some(s) = store {
                if let Err(e) = s.set_session_reasoning_mode(session_id, mode) {
                    out.notice(&format!("error: {e}"));
                    return exit;
                }
            }
            match mode {
                Some(mode) => out.notice(&format!("reasoning set to {}", mode.effort_str())),
                None => out.notice("reasoning turned off"),
            }
        }
        ChatCommand::Capsules => out.notice(&capsules.list_display()),
        ChatCommand::LoadCapsule(name) => {
            if name.is_empty() {
                out.notice("usage: /load <capsule_name>");
            } else {
                match capsules.load(&name) {
                    Ok(true) => {
                        agent.messages[0] = Message::system(capsules.render_system_prompt());
                        out.notice(&format!("loaded {name}"));
                    }
                    Ok(false) => out.notice(&format!("{name} already loaded")),
                    Err(msg) => out.notice(&msg),
                }
            }
        }
        ChatCommand::UnloadCapsule(name) => {
            if name.is_empty() {
                out.notice("usage: /unload <capsule_name>");
            } else if capsules.unload(&name) {
                agent.messages[0] = Message::system(capsules.render_system_prompt());
                out.notice(&format!("unloaded {name}"));
            } else {
                out.notice(&format!("{name} is not loaded"));
            }
        }
        ChatCommand::InvokeCapsule { name, prompt } => {
            match capsules.load(&name) {
                Ok(true) => {
                    agent.messages[0] = Message::system(capsules.render_system_prompt());
                    out.notice(&format!("loaded {name}"));
                }
                Ok(false) => {}
                Err(msg) => {
                    out.notice(&msg);
                    return exit;
                }
            }
            if let Some(text) = prompt {
                run_and_render(agent, &text, out);
            }
        }
        ChatCommand::Unknown(name) => {
            out.notice(&format!("unknown command '/{name}' — try /help"));
        }
        ChatCommand::Say(text) => {
            if !text.is_empty() {
                run_and_render(agent, &text, out);
            }
        }
    }
    exit
}

fn run_and_render(agent: &mut Agent, text: &str, out: &mut dyn Renderer) {
    match agent.run(text) {
        Outcome::Complete(answer) => out.assistant(&answer),
        Outcome::StepLimit => out.notice("(step limit reached)"),
        Outcome::VerificationFailed { attempts } => out.notice(&format!(
            "(verification still failing after {attempts} attempts)"
        )),
        Outcome::Cancelled => out.notice("(cancelled)"),
        Outcome::RepeatedAction => out.notice("(stopped: repeated action)"),
        Outcome::Blocked => {
            out.notice("(stopped: actions denied — use /approve to allow this session)")
        }
        Outcome::Error(e) => out.notice(&format!("(error: {e})")),
    }
}

fn chat_undo(store: &Option<Store>, session_id: &str, cwd: &Path, out: &mut dyn Renderer) {
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
    let messages = match store.load_message_records(id) {
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
    let system = compose_system_with_persona(&cwd, persona(&cwd, &merged).as_deref());
    let (base_url, model_name) = resolve_host_and_model(overrides, &merged);
    let provider = resolve_provider(overrides, &merged).unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
    let model = HttpModel {
        url: join_url(&base_url, provider.path_suffix()),
        api_key: std::env::var("QUECTO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        model: model_name,
        provider,
        max_tokens: resolve_max_tokens(overrides, &merged),
    }
    .try_with_env_reasoning_mode(
        store
            .session_reasoning_mode(id)
            .ok()
            .flatten()
            .or(merged.reasoning_mode),
    )
    .unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
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
    let mut agent = Agent::new(Box::new(model), system, steps, cwd, cancel, approval)
        .register_builtins_filtered(merged.tools.enabled.as_deref())
        .with_policy(build_policy(overrides.approval.as_deref(), &gated))
        .with_message_records(messages);

    agent = attach_mcp_tools(agent, overrides, false);

    agent = attach_verifier(agent, no_verify, &gated);
    if let Ok(rec_store) = Store::open_default() {
        agent = agent.with_recorder(Box::new(SqliteRecorder::new(
            rec_store,
            id.to_string(),
            msg_seq,
            change_seq,
        )));
    }

    eprintln!("quecto-agent: resuming session {id}...");
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

#[cfg(not(feature = "mcp"))]
fn attach_mcp_tools(agent: Agent, _overrides: &Overrides, _add_prompt_additions: bool) -> Agent {
    agent
}

#[cfg(feature = "mcp")]
fn attach_mcp_tools(mut agent: Agent, overrides: &Overrides, add_prompt_additions: bool) -> Agent {
    use quecto_mcp::{McpConfig, McpRegistry};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    let file_cfg = McpConfig::from_file(Path::new(".quecto/mcp.toml")).unwrap_or_else(|e| {
        eprintln!("quecto-mcp: config warning: {e}");
        McpConfig::empty()
    });
    let env_cfg = McpConfig::from_env().unwrap_or_else(|e| {
        eprintln!("quecto-mcp: env warning: {e}");
        McpConfig::empty()
    });
    let cli_cfg = mcp_config_from_flags(&overrides.mcp);
    let merged = McpConfig::merged(file_cfg, env_cfg, cli_cfg);

    let mut registry = McpRegistry::new(merged);
    let mcp_tools = registry.discover();
    let prompt_additions = registry.system_prompt_additions();
    let registry_arc = Arc::new(Mutex::new(registry));

    for mcp_tool in mcp_tools {
        let adapter = quecto_agent::mcp_adapter::McpToolAdapter {
            tool: mcp_tool,
            registry: std::sync::Arc::clone(&registry_arc),
        };
        agent = agent.register(Box::new(adapter));
    }
    if add_prompt_additions {
        use quecto_agent::ContentPart;

        for addition in &prompt_additions {
            if let Some(msg) = agent.messages.first_mut() {
                if let Some(ContentPart::Text(text)) = msg.content.last_mut() {
                    text.push_str("\n\n");
                    text.push_str(addition);
                } else {
                    msg.content.push(ContentPart::Text(format!("\n\n{addition}")));
                }
            }
        }
    }
    agent
}

#[cfg(feature = "mcp")]
fn mcp_config_from_flags(flags: &[String]) -> quecto_mcp::McpConfig {
    use quecto_mcp::config::{ServerConfig, TransportKind, TrustLevel};
    use std::collections::HashMap;
    let mut servers = Vec::new();
    for flag in flags {
        let parts: Vec<&str> = flag.splitn(3, ':').collect();
        if parts.len() < 3 {
            eprintln!("quecto-mcp: ignoring malformed --mcp flag: {flag}");
            continue;
        }
        let (transport_str, name, rest) = (parts[0], parts[1], parts[2]);
        let transport = match transport_str {
            "stdio" => TransportKind::Stdio,
            "streamable_http" => TransportKind::StreamableHttp,
            "sse" => TransportKind::Sse,
            other => {
                eprintln!("quecto-mcp: unknown transport '{other}'");
                continue;
            }
        };
        let server = match transport {
            TransportKind::Stdio => {
                let mut p = rest.split(':');
                let command = p.next().unwrap_or("").to_string();
                let args: Vec<String> = p.map(str::to_string).collect();
                ServerConfig {
                    name: name.to_string(),
                    transport,
                    command: Some(command),
                    args,
                    env: HashMap::new(),
                    url: None,
                    headers: HashMap::new(),
                    trust: TrustLevel::Sandbox,
                    timeout_secs: None,
                }
            }
            _ => ServerConfig {
                name: name.to_string(),
                transport,
                command: None,
                args: vec![],
                env: HashMap::new(),
                url: Some(rest.to_string()),
                headers: HashMap::new(),
                trust: TrustLevel::Sandbox,
                timeout_secs: None,
            },
        };
        servers.push(server);
    }
    quecto_mcp::McpConfig { servers }
}

#[cfg(test)]
mod main_tests {
    use super::*;
    use quecto_agent::Renderer;
    use std::path::Path;

    #[derive(Default)]
    struct TestRenderer {
        notices: Vec<String>,
        assistant_replies: Vec<String>,
    }

    impl Renderer for TestRenderer {
        fn tool(&mut self, _name: &str, _summary: &str) {}
        fn verify(&mut self, _command: &str, _passed: bool) {}
        fn notice(&mut self, text: &str) {
            self.notices.push(text.to_string());
        }
        fn assistant(&mut self, text: &str) {
            self.assistant_replies.push(text.to_string());
        }
    }

    fn test_agent(mode: Option<ReasoningMode>) -> Agent {
        let model = HttpModel {
            url: "http://example.test/v1/chat/completions".into(),
            api_key: None,
            model: "test-model".into(),
            provider: Provider::OpenAiCompatible,
            max_tokens: None,
        }
        .with_default_reasoning_mode(mode);
        Agent::new(
            Box::new(model),
            "system".to_string(),
            4,
            std::env::current_dir().unwrap(),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
    }

    #[derive(Clone)]
    struct FakeModel {
        reply: String,
    }

    impl quecto_agent::Model for FakeModel {
        fn complete(
            &self,
            _messages: &[quecto_agent::Message],
            _tools: &[serde_json::Value],
        ) -> Result<quecto_agent::AssistantMessage, quecto_agent::BoxErr> {
            Ok(quecto_agent::AssistantMessage {
                content: self.reply.clone(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: None,
            })
        }

        fn clone_box(&self) -> Box<dyn quecto_agent::Model> {
            Box::new(self.clone())
        }
    }

    fn fake_agent(reply: &str) -> Agent {
        Agent::new(
            Box::new(FakeModel {
                reply: reply.to_string(),
            }),
            "system",
            4,
            std::env::current_dir().unwrap(),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
    }

    fn test_capsules() -> CapsuleState {
        CapsuleState::new(CapsuleRegistry::default(), "system".to_string())
    }

    fn write_capsule(root: &Path, name: &str, description: &str, body: &str) {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("CAPSULE.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    fn capsules_from(project_root: &Path) -> CapsuleState {
        let registry = CapsuleRegistry::discover(Path::new("/does-not-exist-user-dir"), project_root);
        CapsuleState::new(registry, "system".to_string())
    }

    #[test]
    fn load_unknown_capsule_reports_error() {
        let mut agent = test_agent(None);
        let mut capsules = test_capsules();
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(
            out.notices,
            vec!["no such capsule: demo (see /capsules)".to_string()]
        );
    }

    #[test]
    fn load_capsule_updates_agent_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "Follow the demo workflow.");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(out.notices, vec!["loaded demo".to_string()]);
        let prompt = agent.messages[0].text();
        assert!(prompt.contains("## Capsule: demo"));
        assert!(prompt.contains("Follow the demo workflow."));
    }

    #[test]
    fn unload_capsule_reverts_agent_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "Follow the demo workflow.");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        let exit = handle_chat_command(
            "/unload demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(agent.messages[0].text(), "system");
    }

    #[test]
    fn clear_does_not_unload_active_capsules() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "Follow the demo workflow.");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        let exit = handle_chat_command(
            "/clear", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert!(agent.messages[0].text().contains("## Capsule: demo"));
        assert_eq!(agent.messages.len(), 1);
    }

    #[test]
    fn capsules_command_marks_active_capsule() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "body");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        out.notices.clear();
        handle_chat_command(
            "/capsules", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert_eq!(out.notices, vec!["● demo — demo capsule".to_string()]);
    }

    #[test]
    fn bare_capsule_invocation_loads_without_running_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "body");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert!(capsules.is_active("demo"));
        assert_eq!(out.notices, vec!["loaded demo".to_string()]);
        assert_eq!(agent.messages.len(), 1);
    }

    #[test]
    fn unload_not_loaded_capsule_reports_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "body");
        let mut capsules = capsules_from(dir.path());
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/unload demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(out.notices, vec!["demo is not loaded".to_string()]);
    }

    #[test]
    fn capsule_session_lifecycle_load_invoke_unload_exit() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "Follow the demo workflow.");
        let mut capsules = capsules_from(dir.path());
        let mut agent = fake_agent("done!");
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/load demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        assert!(!exit);
        assert!(agent.messages[0].text().contains("## Capsule: demo"));

        let exit = handle_chat_command(
            "/demo please help", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        assert!(!exit);
        let prompt_after_invoke = agent.messages[0].text();
        assert_eq!(prompt_after_invoke.matches("## Capsule: demo").count(), 1);
        assert_eq!(agent.messages.len(), 3);
        assert_eq!(out.assistant_replies, vec!["done!".to_string()]);

        let exit = handle_chat_command(
            "/unload demo", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        assert!(!exit);
        assert_eq!(agent.messages[0].text(), "system");

        let exit = handle_chat_command(
            "/exit", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );
        assert!(exit);
    }

    struct EnvGuard(Vec<(String, Option<String>)>);

    impl EnvGuard {
        fn set(values: &[(&str, Option<&str>)]) -> Self {
            let previous = values
                .iter()
                .map(|(name, _)| ((*name).to_string(), std::env::var(name).ok()))
                .collect();
            for (name, value) in values {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            Self(previous)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn reasoning_query_reports_off_when_unset() {
        let mut agent = test_agent(None);
        let store = Some(Store::open_in_memory().unwrap());
        store
            .as_ref()
            .unwrap()
            .create_session_with_reasoning_mode("s1", "chat", "/repo", "test-model", None)
            .unwrap();
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/reasoning",
            &mut agent,
            &store,
            "s1",
            Path::new("/repo"),
            "test-model",
            &mut test_capsules(),
            &mut out,
        );

        assert!(!exit);
        assert_eq!(out.notices, vec!["reasoning: off".to_string()]);
    }

    #[test]
    fn reasoning_set_updates_agent_and_store() {
        let mut agent = test_agent(None);
        let store = Some(Store::open_in_memory().unwrap());
        store
            .as_ref()
            .unwrap()
            .create_session_with_reasoning_mode("s1", "chat", "/repo", "test-model", None)
            .unwrap();
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/reasoning high",
            &mut agent,
            &store,
            "s1",
            Path::new("/repo"),
            "test-model",
            &mut test_capsules(),
            &mut out,
        );

        assert!(!exit);
        assert_eq!(agent.session_reasoning_mode(), Some(ReasoningMode::High));
        assert_eq!(
            store
                .as_ref()
                .unwrap()
                .session_reasoning_mode("s1")
                .unwrap(),
            Some(ReasoningMode::High)
        );
        assert_eq!(out.notices, vec!["reasoning set to high".to_string()]);
    }

    #[test]
    fn chat_reasoning_updates_can_be_cleared_with_off() {
        let mut agent = test_agent(Some(ReasoningMode::Medium));
        let store = Some(Store::open_in_memory().unwrap());
        store
            .as_ref()
            .unwrap()
            .create_session_with_reasoning_mode(
                "s1",
                "chat",
                "/repo",
                "test-model",
                Some(ReasoningMode::Medium),
            )
            .unwrap();
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/reasoning off",
            &mut agent,
            &store,
            "s1",
            Path::new("/repo"),
            "test-model",
            &mut test_capsules(),
            &mut out,
        );

        assert_eq!(agent.session_reasoning_mode(), None);
        assert_eq!(
            store
                .as_ref()
                .unwrap()
                .session_reasoning_mode("s1")
                .unwrap(),
            None
        );
        assert_eq!(out.notices, vec!["reasoning turned off".to_string()]);
    }

    #[test]
    fn resume_prefers_persisted_session_reasoning_mode() {
        let _env = EnvGuard::set(&[
            ("QUECTO_REASONING_MODE", None),
            ("QUECTO_BASE_URL", Some("http://localhost:1234/v1")),
            ("QUECTO_MODEL", Some("reasoning-model")),
        ]);
        let store = Store::open_in_memory().unwrap();
        store
            .create_session_with_reasoning_mode(
                "s1",
                "chat",
                "/repo",
                "reasoning-model",
                Some(ReasoningMode::High),
            )
            .unwrap();

        let persisted = store.session_reasoning_mode("s1").unwrap();
        let model = HttpModel::from_env()
            .try_with_env_reasoning_mode(persisted)
            .unwrap();

        assert_eq!(model.session_reasoning_mode(), Some(ReasoningMode::High));
    }

    #[test]
    fn one_shot_run_does_not_depend_on_session_reasoning_state() {
        let _env = EnvGuard::set(&[
            ("QUECTO_REASONING_MODE", None),
            ("QUECTO_BASE_URL", Some("http://localhost:1234/v1")),
            ("QUECTO_MODEL", Some("reasoning-model")),
        ]);

        let model = HttpModel::from_env()
            .try_with_env_reasoning_mode(None)
            .unwrap();

        assert_eq!(model.session_reasoning_mode(), None);
    }

    #[test]
    #[cfg(feature = "otel")]
    fn test_otel_initialization() {
        // init_otel might return None if a global default subscriber has already been set,
        // but we want to check that if we call it, it doesn't panic.
        let _guard = super::otel_init::init_otel();
    }

    #[test]
    fn test_mime_from_extension() {
        use std::path::Path;
        assert_eq!(super::mime_from_extension(Path::new("test.png")), "image/png");
        assert_eq!(super::mime_from_extension(Path::new("test.jpg")), "image/jpeg");
        assert_eq!(super::mime_from_extension(Path::new("test.jpeg")), "image/jpeg");
        assert_eq!(super::mime_from_extension(Path::new("test.gif")), "image/gif");
        assert_eq!(super::mime_from_extension(Path::new("test.webp")), "image/webp");
        assert_eq!(super::mime_from_extension(Path::new("test.unknown")), "image/png");
    }

    #[test]
    fn test_is_image_extension() {
        use std::path::Path;
        assert!(super::is_image_extension(Path::new("test.png")));
        assert!(super::is_image_extension(Path::new("test.jpg")));
        assert!(super::is_image_extension(Path::new("test.jpeg")));
        assert!(super::is_image_extension(Path::new("test.gif")));
        assert!(super::is_image_extension(Path::new("test.webp")));
        assert!(!super::is_image_extension(Path::new("test.txt")));
        assert!(!super::is_image_extension(Path::new("test")));
    }

    #[test]
    fn test_extract_image_refs() {
        let dir = tempfile::tempdir().unwrap();
        let path1 = dir.path().join("img1.png");
        let path2 = dir.path().join("img2.jpg");
        std::fs::write(&path1, b"fake png").unwrap();
        std::fs::write(&path2, b"fake jpg").unwrap();

        let text = "Look at @image img1.png and @img img2.jpg or @image missing.png end";
        let (cleaned, images) = super::extract_image_refs(text, dir.path());

        assert_eq!(cleaned, "Look at [Image 1] and [Image 2] or @image missing.png end");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].0, b"fake png");
        assert_eq!(images[0].1, "image/png");
        assert_eq!(images[1].0, b"fake jpg");
        assert_eq!(images[1].1, "image/jpeg");
    }

    #[test]
    fn test_segments_to_parts() {
        use super::Segment;
        use quecto_agent::ContentPart;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ref.png");
        std::fs::write(&path, b"ref_data").unwrap();

        let segments = vec![
            Segment::Text("Hello ".into()),
            Segment::Paste("world. ".into()),
            Segment::Image {
                data: b"img_data".to_vec(),
                mime_type: "image/png".into(),
                index: 1,
            },
            Segment::Text("See @image ref.png".into()),
        ];

        let parts = super::segments_to_parts(&segments, dir.path());
        assert_eq!(parts.len(), 4);
        
        match &parts[0] {
            ContentPart::Text(t) => assert_eq!(t, "Hello world. "),
            _ => panic!("Expected text part"),
        }
        match &parts[1] {
            ContentPart::Image { data, mime_type } => {
                assert_eq!(data, b"img_data");
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("Expected image part"),
        }
        match &parts[2] {
            ContentPart::Text(t) => assert_eq!(t, "See [Image 1]"),
            _ => panic!("Expected text part"),
        }
        match &parts[3] {
            ContentPart::Image { data, mime_type } => {
                assert_eq!(data, b"ref_data");
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("Expected image part"),
        }
    }
}
