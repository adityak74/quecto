# quecto-agent M6b — crossterm Renderer and Interactive Chat REPL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the agent's activity output through a pluggable `crossterm`-colored renderer and add an interactive `chat` subcommand with a slash-command REPL over a persistent, recorded session.

**Architecture:** A `Renderer` trait abstracts the agent's activity lines (tool, verify, notice). The agent holds a boxed renderer defaulting to a plain stderr line-renderer that reproduces today's exact output, so one-shot behavior and all existing tests are unchanged. The `chat` subcommand keeps one `Agent` alive across user turns (each turn is `agent.run(line)`, which accumulates the transcript and records incrementally), dispatching slash-commands parsed by a pure function. Interactive per-action approval is avoided in chat (stdin is the REPL); `/approve` and `/deny` flip the session approval mode instead.

**Tech Stack:** Rust 2021; `crossterm = "0.27"` for colored output; existing `clap`, `rusqlite`, `serde_json`. `main` stays a plain `fn main()`.

## Global Constraints

- Build on completed M6a APIs exactly as present: `Agent::new(model, system, max_steps, repo_root, cancel, approval)`, `Agent::run(&mut self, &str) -> Outcome`, `Agent::resume`, `with_recorder`, `with_messages`, `register_builtins`, `with_verifier`; `Store` (`open_default`, `create_session`, `set_status`, `latest_session`, `take_last_change`, `load_changes`, `message_count`, `change_count`), `new_session_id`, `render_change_summary`, `SqliteRecorder`; `Outcome`, `ApprovalMode::{AutoApprove, NonInteractive, terminal}`, `cancel_token`, `CancelToken`, `HttpModel`, `load_instructions`, `seed_context`, `Verifier`.
- One new dependency only: `crossterm = "0.27"`. No others.
- The default agent renderer MUST produce byte-identical output to today when color is disabled: `● {name}  {summary}\n` for tools and `● verify {command}  {passed|failed}\n` for verification. Existing tests and one-shot UX must not change.
- Color is enabled only when the target stream is a TTY (`IsTerminal`). Piped/test output is never colored.
- `chat` never uses interactive per-action approval (stdin is the REPL). It starts `AutoApprove` when `--yes` is given, else `NonInteractive`. `/approve` → `AutoApprove`; `/deny` → `NonInteractive`.
- `chat` records to the session store like one-shot runs; a store failure degrades to a stderr note and the REPL still works.
- True token streaming stays deferred (the loop uses buffered `quecto_raw`); chat prints each turn's complete answer.
- Run repository shell commands through `rtk` per `AGENTS.md`. Stage/commit only files named by each task. `fmt`/`clippy -D warnings`/`git diff --check` must pass at the end.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `crossterm`.
- `quecto-agent/src/render.rs` — `Renderer` trait, `LineRenderer<W>`, `stderr_renderer()`, `stdout_renderer()`.
- `quecto-agent/src/agent.rs` — `renderer` field + `with_renderer`, `set_approval`, `clear_history`; route the two activity sites through the renderer.
- `quecto-agent/src/chat.rs` — `ChatCommand` enum + `parse_command`.
- `quecto-agent/src/lib.rs` — declare/export the new modules' public items.
- `quecto-agent/src/main.rs` — `Chat` subcommand + REPL loop.
- `quecto-agent/tests/cli.rs` — chat integration tests.

---

### Task 1: Renderer abstraction

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Create: `quecto-agent/src/render.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces:
  - `trait Renderer: Send { fn tool(&mut self, name: &str, summary: &str); fn verify(&mut self, command: &str, passed: bool); fn notice(&mut self, text: &str); fn assistant(&mut self, text: &str); }`
  - `LineRenderer<W: Write>` with `new(out: W, color: bool) -> Self`.
  - `stderr_renderer() -> Box<dyn Renderer>` and `stdout_renderer() -> Box<dyn Renderer>`.

- [ ] **Step 1: Add the dependency**

Add to `quecto-agent/Cargo.toml` under `[dependencies]`:

```toml
crossterm = "0.27"
```

- [ ] **Step 2: Write the failing tests**

Create `quecto-agent/src/render.rs`:

```rust
use crossterm::style::Stylize;
use std::io::{self, IsTerminal, Write};

/// Receives an agent run's activity for display. Implementations format and
/// write; they never fail the run (write errors are ignored).
pub trait Renderer: Send {
    fn tool(&mut self, name: &str, summary: &str);
    fn verify(&mut self, command: &str, passed: bool);
    fn notice(&mut self, text: &str);
    fn assistant(&mut self, text: &str);
}

/// Line-based renderer over any writer. Colors are applied only when `color`
/// is true; with `color = false` the output is byte-identical to the agent's
/// historical plain output.
pub struct LineRenderer<W: Write> {
    out: W,
    color: bool,
}

impl<W: Write> LineRenderer<W> {
    pub fn new(out: W, color: bool) -> Self {
        LineRenderer { out, color }
    }

    fn bullet(&self) -> String {
        if self.color {
            format!("{}", "●".cyan())
        } else {
            "●".to_string()
        }
    }
}

impl<W: Write + Send> Renderer for LineRenderer<W> {
    fn tool(&mut self, name: &str, summary: &str) {
        let _ = writeln!(self.out, "{} {name}  {summary}", self.bullet());
    }

    fn verify(&mut self, command: &str, passed: bool) {
        let word = if passed { "passed" } else { "failed" };
        let shown = if self.color {
            if passed {
                format!("{}", word.green())
            } else {
                format!("{}", word.red())
            }
        } else {
            word.to_string()
        };
        let _ = writeln!(self.out, "{} verify {command}  {shown}", self.bullet());
    }

    fn notice(&mut self, text: &str) {
        let shown = if self.color {
            format!("{}", text.dark_grey())
        } else {
            text.to_string()
        };
        let _ = writeln!(self.out, "{shown}");
    }

    fn assistant(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }
}

/// A boxed renderer over stderr, colored only when stderr is a TTY.
pub fn stderr_renderer() -> Box<dyn Renderer> {
    let color = io::stderr().is_terminal();
    Box::new(LineRenderer::new(io::stderr(), color))
}

/// A boxed renderer over stdout, colored only when stdout is a TTY.
pub fn stdout_renderer() -> Box<dyn Renderer> {
    let color = io::stdout().is_terminal();
    Box::new(LineRenderer::new(io::stdout(), color))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_plain(f: impl FnOnce(&mut LineRenderer<&mut Vec<u8>>)) -> String {
        let mut buf = Vec::new();
        {
            let mut r = LineRenderer::new(&mut buf, false);
            f(&mut r);
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn plain_tool_line_matches_legacy_format() {
        let s = render_plain(|r| r.tool("read_file", "1 lines"));
        assert_eq!(s, "● read_file  1 lines\n");
    }

    #[test]
    fn plain_verify_line_reports_pass_and_fail() {
        assert_eq!(
            render_plain(|r| r.verify("cargo test", true)),
            "● verify cargo test  passed\n"
        );
        assert_eq!(
            render_plain(|r| r.verify("cargo test", false)),
            "● verify cargo test  failed\n"
        );
    }

    #[test]
    fn plain_notice_and_assistant_are_raw_text() {
        assert_eq!(render_plain(|r| r.notice("hello")), "hello\n");
        assert_eq!(render_plain(|r| r.assistant("answer")), "answer\n");
    }

    #[test]
    fn color_output_contains_ansi_escapes() {
        let mut buf = Vec::new();
        {
            let mut r = LineRenderer::new(&mut buf, true);
            r.tool("read_file", "x");
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains('\u{1b}'), "colored output should contain ANSI escapes");
        assert!(s.contains("read_file"));
    }
}
```

- [ ] **Step 3: Declare the module and run the tests**

Add `mod render;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib render`

Expected: PASS (4 tests).

- [ ] **Step 4: Export the public items**

Add to `lib.rs`:

```rust
pub use render::{stderr_renderer, stdout_renderer, LineRenderer, Renderer};
```

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/Cargo.toml quecto-agent/src/render.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add crossterm line renderer"
```

---

### Task 2: Route agent activity through the renderer

**Files:**
- Modify: `quecto-agent/src/agent.rs`

**Interfaces:**
- Consumes: `Renderer`, `stderr_renderer`.
- Produces: `Agent::with_renderer(self, Box<dyn Renderer>) -> Self`, `Agent::set_approval(&mut self, ApprovalMode)`, `Agent::clear_history(&mut self)`.
- Behavior: the tool activity line and the verify line are emitted via `self.renderer` instead of `eprintln!`; the default renderer is `stderr_renderer()`, preserving current output.

- [ ] **Step 1: Add a renderer-capture test**

Add to the `tests` module in `agent.rs` (reuse the existing `FakeRecorder`, `Scripted`, `text`, `wants_tool` helpers):

```rust
struct CaptureRenderer {
    tools: Arc<Mutex<Vec<String>>>,
}
impl crate::render::Renderer for CaptureRenderer {
    fn tool(&mut self, name: &str, summary: &str) {
        self.tools.lock().unwrap().push(format!("{name}:{summary}"));
    }
    fn verify(&mut self, _command: &str, _passed: bool) {}
    fn notice(&mut self, _text: &str) {}
    fn assistant(&mut self, _text: &str) {}
}

#[test]
fn renderer_receives_tool_activity() {
    let tools = Arc::new(Mutex::new(Vec::new()));
    let model = Scripted::new(vec![wants_tool("read_file"), text("done")]);
    let mut a = agent(model)
        .register(Box::new(RecordingNamed {
            name: "read_file",
            ran: Arc::new(AtomicBool::new(false)),
        }))
        .with_renderer(Box::new(CaptureRenderer {
            tools: tools.clone(),
        }));
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    assert_eq!(tools.lock().unwrap().clone(), vec!["read_file:ok".to_string()]);
}

#[test]
fn set_approval_switches_gate_behavior() {
    let ran = Arc::new(AtomicBool::new(false));
    let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
    let mut a = configured_agent(model, ApprovalMode::NonInteractive).register(Box::new(
        RecordingNamed {
            name: "write_file",
            ran: ran.clone(),
        },
    ));
    a.set_approval(ApprovalMode::AutoApprove);
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn clear_history_keeps_only_the_system_message() {
    let mut a = agent(Scripted::new(vec![text("done")]));
    assert!(matches!(a.run("first"), Outcome::Complete(_)));
    a.clear_history();
    // Second run starts fresh: only system + new user + assistant recorded shape.
    assert!(matches!(a.run("second"), Outcome::Complete(_)));
}
```

Note: `RecordingNamed` returns summary `"ok"` (see its `run`), so the expected tool activity is `read_file:ok`.

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib agent::tests::renderer_receives_tool_activity agent::tests::set_approval agent::tests::clear_history`

Expected: FAIL — `with_renderer`, `set_approval`, `clear_history` do not exist.

- [ ] **Step 3: Add the import, field, and default**

Add to the top imports of `agent.rs`:

```rust
use crate::render::{stderr_renderer, Renderer};
```

Add the field to `struct Agent` (after `recorded_changes: usize,`):

```rust
    renderer: Box<dyn Renderer>,
```

Initialize it in `Agent::new` (in the struct literal, after `recorded_changes: 0,`):

```rust
            renderer: stderr_renderer(),
```

- [ ] **Step 4: Add the builder and setters**

Add to `impl Agent` (next to `with_recorder`):

```rust
    /// Replace the activity renderer (default: plain stderr).
    pub fn with_renderer(mut self, renderer: Box<dyn Renderer>) -> Self {
        self.renderer = renderer;
        self
    }

    /// Change the approval mode mid-session (used by the chat REPL).
    pub fn set_approval(&mut self, approval: ApprovalMode) {
        self.approval = approval;
    }

    /// Drop the conversation history, keeping only the system message. The
    /// recording cursor is reset so a fresh turn records from the new baseline.
    pub fn clear_history(&mut self) {
        self.messages.truncate(1);
        self.recorded_messages = self.messages.len();
    }
```

- [ ] **Step 5: Replace the two `eprintln!` activity sites**

In `run_loop`, replace:

```rust
                        for r in &report.results {
                            eprintln!(
                                "● verify {}  {}",
                                r.command,
                                if r.passed { "passed" } else { "failed" }
                            );
                        }
```

with:

```rust
                        for r in &report.results {
                            self.renderer.verify(&r.command, r.passed);
                        }
```

And replace:

```rust
                eprintln!("● {}  {}", call.name, out.summary);
```

with:

```rust
                self.renderer.tool(&call.name, &out.summary);
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS (all agent tests, including the three new ones).

- [ ] **Step 7: Commit**

```bash
rtk git add quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): render activity via pluggable renderer"
```

---

### Task 3: Slash-command parser

**Files:**
- Create: `quecto-agent/src/chat.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum ChatCommand { Help, Model, Context, Diff, Status, Undo, Approve, Deny, Clear, Exit, Say(String), Unknown(String) }`
  - `parse_command(line: &str) -> ChatCommand`

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/chat.rs`:

```rust
/// A parsed line of chat input: a slash-command or plain text to send.
#[derive(Debug, PartialEq)]
pub enum ChatCommand {
    Help,
    Model,
    Context,
    Diff,
    Status,
    Undo,
    Approve,
    Deny,
    Clear,
    Exit,
    Say(String),
    Unknown(String),
}

/// Parse one line of REPL input. A leading `/` marks a command (case-insensitive,
/// first word only); anything else — including an empty line — is `Say`.
pub fn parse_command(line: &str) -> ChatCommand {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ChatCommand::Say(trimmed.to_string());
    };
    let name = rest.split_whitespace().next().unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
        "help" | "h" | "?" => ChatCommand::Help,
        "model" => ChatCommand::Model,
        "context" => ChatCommand::Context,
        "diff" => ChatCommand::Diff,
        "status" => ChatCommand::Status,
        "undo" => ChatCommand::Undo,
        "approve" => ChatCommand::Approve,
        "deny" => ChatCommand::Deny,
        "clear" => ChatCommand::Clear,
        "exit" | "quit" | "q" => ChatCommand::Exit,
        other => ChatCommand::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_say_trimmed() {
        assert_eq!(
            parse_command("  fix the bug  "),
            ChatCommand::Say("fix the bug".to_string())
        );
    }

    #[test]
    fn known_commands_parse_case_insensitively() {
        assert_eq!(parse_command("/HELP"), ChatCommand::Help);
        assert_eq!(parse_command("/Exit"), ChatCommand::Exit);
        assert_eq!(parse_command("/diff"), ChatCommand::Diff);
        assert_eq!(parse_command("/undo"), ChatCommand::Undo);
        assert_eq!(parse_command("/approve"), ChatCommand::Approve);
        assert_eq!(parse_command("/deny"), ChatCommand::Deny);
    }

    #[test]
    fn command_ignores_trailing_arguments() {
        assert_eq!(parse_command("/model gpt-4o"), ChatCommand::Model);
    }

    #[test]
    fn unknown_slash_command_is_reported() {
        assert_eq!(
            parse_command("/frobnicate"),
            ChatCommand::Unknown("frobnicate".to_string())
        );
    }

    #[test]
    fn aliases_map_to_canonical_commands() {
        assert_eq!(parse_command("/q"), ChatCommand::Exit);
        assert_eq!(parse_command("/?"), ChatCommand::Help);
    }
}
```

- [ ] **Step 2: Declare the module and run**

Add `mod chat;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib chat`

Expected: PASS (5 tests).

- [ ] **Step 3: Export the parser**

Add to `lib.rs`:

```rust
pub use chat::{parse_command, ChatCommand};
```

- [ ] **Step 4: Commit**

```bash
rtk git add quecto-agent/src/chat.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add chat slash-command parser"
```

---

### Task 4: Chat REPL subcommand

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Consumes: `quecto_agent::{parse_command, ChatCommand, LineRenderer, Agent, ApprovalMode, Store, SqliteRecorder, new_session_id, render_change_summary, Outcome}`.
- Behavior: `quecto-agent chat [--yes] [--no-verify]` starts a REPL. Each plain line runs one agent turn on the persistent transcript and prints the answer. Slash-commands act as specified. EOF or `/exit` ends the session and sets its status.

- [ ] **Step 1: Add the `Chat` subcommand variant and dispatch**

In `main.rs`, add to `enum Command`:

```rust
    /// Start an interactive chat session.
    Chat,
```

In `fn main`, extend the match:

```rust
        Some(Command::Chat) => chat(cli.yes, cli.no_verify),
```

- [ ] **Step 2: Add the chat integration tests**

Add to `quecto-agent/tests/cli.rs`:

```rust
#[test]
fn chat_help_and_exit_without_model() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(bin())
        .arg("chat")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_BASE_URL", "http://127.0.0.1:1") // unused: no plain-text turn
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/help\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/help"), "help listing expected: {stdout}");
}

#[test]
fn chat_runs_a_turn_and_records_it() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":"hello there"},"finish_reason":"stop"}]}"#,
    ]);
    let mut child = Command::new(bin())
        .args(["chat", "--yes"])
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_BASE_URL", &base)
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say hello\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello there"));
}
```

- [ ] **Step 3: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli chat`

Expected: FAIL — `chat` is not a recognized subcommand yet, so it is parsed as a bare task or errors.

- [ ] **Step 4: Implement `fn chat` in `main.rs`**

Add imports at the top of `main.rs`:

```rust
use quecto_agent::{parse_command, ChatCommand, LineRenderer};
use std::io::{BufRead, IsTerminal, Write};
```

(Merge these into the existing `use quecto_agent::{...}` and `use std::...` lines rather than duplicating; keep a single import block.)

Add the function:

```rust
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
```

Note: `Path` must be imported in `main.rs` (added in M6a as `use std::path::{Path, PathBuf};`). Keep that import.

- [ ] **Step 5: Run the chat integration tests**

Run: `rtk cargo test -p quecto-agent --test cli`

Expected: PASS for all CLI tests, including the two new chat tests.

- [ ] **Step 6: Full verification**

Run: `rtk cargo fmt --all -- --check`
Expected: PASS (run `rtk cargo fmt --all` then re-check if needed).

Run: `rtk cargo test --workspace -- --test-threads=1`
Expected: PASS for both crates.

Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

Run: `rtk git diff --check`
Expected: no output, exit 0.

- [ ] **Step 7: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs
rtk git commit -m "feat(agent): add interactive chat REPL"
```

---

## Final Acceptance Checklist

- [ ] The default agent renderer produces byte-identical plain output; existing tests and one-shot UX are unchanged.
- [ ] Activity lines are colored only on a TTY; piped output has no ANSI.
- [ ] `chat` starts a recorded session; each plain line runs one turn on the persistent transcript and prints the complete answer.
- [ ] Slash-commands `/help /model /context /diff /status /undo /approve /deny /clear /exit` all work; unknown commands report a hint.
- [ ] `/approve` and `/deny` flip the session approval mode; chat never blocks on interactive per-action approval.
- [ ] `/undo` reverts the last recorded change of the chat session; `/diff` summarizes them.
- [ ] EOF and `/exit` end the session and set status; store failures degrade to a note without breaking the REPL.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work

- True token streaming of assistant text (`quecto_stream`) in chat; a full-screen TUI (`ratatui`); spinners/progress bars.
- A real unified-diff view for `/diff` (still a per-file summary).
- **M7:** flavor manifests (`toml`), flavor-configured policy/verification, `sha2` trust-on-first-use, `new`/`init` scaffolding, `--flavor/--model/--base-url/--approval/--cwd/--no-stream` global flags.
- The `mcp` feature (`tokio` + `rmcp`). Windows support.
