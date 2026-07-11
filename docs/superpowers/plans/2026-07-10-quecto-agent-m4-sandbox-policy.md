# quecto-agent M4 — Unix Sandbox, Approval Policy, Cancellation, and Loop Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every model-requested mutation pass through a fixed approval policy, add bounded Unix `run_command` execution, and stop cancellation or no-progress loops safely.

**Architecture:** The agent loop is the single policy enforcement point: it classifies each `ToolCall`, resolves `Ask` through an injected approval provider or `--yes`, and only then dispatches through the registry. `run_command` delegates process lifecycle to a Unix sandbox with a canonical repo cwd, its own process group, timeout/cancel polling, bounded output, and best-effort secret redaction. A shared atomic cancellation token and a file-change-aware repeated-action tracker remain independent, testable units.

**Tech Stack:** Rust 2021; synchronous `std::process`; Unix `std::os::unix::process::CommandExt`; `ctrlc = "3"`; `libc = "0.2"`; existing `serde_json`, `regex`, `ignore`, and `tempfile`.

## Global Constraints

- Build on the completed M3 APIs exactly as present in the working tree: `Context::changes`, `WriteFile`, `ApplyPatch`, and their built-in registration must land before M4 implementation starts.
- Preserve unrelated user changes. Stage and commit only files named by each task.
- Run every repository shell command through `rtk`, per `AGENTS.md` → `/Users/adityakarnam/.codex/RTK.md`.
- M4 supports Unix only. Put `compile_error!("quecto-agent M4 requires a Unix target")` behind `#[cfg(not(unix))]` in `sandbox.rs`.
- The fixed 120-second timeout and fixed built-in policy are not configurable. Flavor configuration remains M7.
- Policy is enforced in `Agent`, never inside individual edit tools. All tool calls, including custom tools, must receive an explicit decision; unknown tool names are denied.
- `--yes` changes only `Ask` to execution. It never overrides `Deny` or the command denylist.
- Interactive approval accepts only case-insensitive `y` or `yes`; EOF, non-TTY, read error, and every other response deny.
- `run_command` always uses `/bin/sh -c`, always runs at the canonical repository root, and exposes no `cwd` argument.
- Timeout and cancellation kill the whole child process group and reap the direct child.
- Cap stdout and stderr independently at 32 KiB using the existing head/tail truncation helper. Never decode captured bytes with `unwrap`; use `String::from_utf8_lossy`.
- Secret redaction is best effort. Redact non-empty inherited environment values whose variable names contain `KEY`, `TOKEN`, `SECRET`, or `PASSWORD`; do not remove them from the child environment.
- Three consecutive identical `(tool name, canonical arguments, result)` observations stop only when `Context::changes().len()` did not advance. A changed call, changed result, or file mutation resets the streak.
- Verification/config/session/rich-renderer/`ask_user`/Windows support remain out of scope.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `ctrlc` and `libc`.
- `quecto-agent/src/policy.rs` — fixed classification, denylist, `Decision`, and denial reasons.
- `quecto-agent/src/approval.rs` — injectable `Approver`, TTY implementation, and `ApprovalMode`.
- `quecto-agent/src/sandbox.rs` — Unix process runner, cancellation token, process-group kill, capture, cap, and redaction.
- `quecto-agent/src/tools/shell.rs` — `RunCommand` schema and sandbox adapter.
- `quecto-agent/src/tools/mod.rs` — carry sandbox state in `Context`; register `RunCommand`.
- `quecto-agent/src/agent.rs` — central policy/approval gate, cancellation outcomes, repeated-action tracker.
- `quecto-agent/src/lib.rs` — module declarations and public exports.
- `quecto-agent/src/main.rs` — parse `--yes`, install Ctrl-C handler, construct configured agent.
- `quecto-agent/tests/cli.rs` — retain one-shot behavior and verify `--yes` is not included in the task.

---

### Task 1: Fixed policy and injectable approval resolution

**Files:**
- Create: `quecto-agent/src/policy.rs`
- Create: `quecto-agent/src/approval.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces: `Decision::{Allow, Ask, Deny(String)}`.
- Produces: `Policy::decide(&ToolCall) -> Decision`.
- Produces: `Approver::confirm(&self, call: &ToolCall) -> bool`.
- Produces: `ApprovalMode::{Interactive(Box<dyn Approver>), NonInteractive, AutoApprove}` and `ApprovalMode::allows(&self, &ToolCall) -> bool`.

- [ ] **Step 1: Write policy tests in `policy.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments }
    }

    #[test]
    fn reads_are_allowed_and_mutations_ask() {
        let p = Policy;
        assert!(matches!(p.decide(&call("read_file", json!({}))), Decision::Allow));
        assert!(matches!(p.decide(&call("write_file", json!({}))), Decision::Ask));
        assert!(matches!(p.decide(&call("apply_patch", json!({}))), Decision::Ask));
        assert!(matches!(p.decide(&call("run_command", json!({"command":"cargo test"}))), Decision::Ask));
    }

    #[test]
    fn unknown_and_dangerous_commands_are_denied() {
        let p = Policy;
        assert!(matches!(p.decide(&call("custom", json!({}))), Decision::Deny(_)));
        for command in ["sudo true", "rm -rf /", "mkfs.ext4 /dev/sda", "fdisk /dev/sda", "diskutil eraseDisk APFS X disk2", "git push origin main", "echo x > /tmp/x"] {
            assert!(matches!(p.decide(&call("run_command", json!({"command":command}))), Decision::Deny(_)), "{command}");
        }
    }
}
```

- [ ] **Step 2: Run the policy test and verify failure**

Run: `rtk cargo test -p quecto-agent --lib policy`

Expected: FAIL because `policy` is not declared and its types do not exist.

- [ ] **Step 3: Implement `policy.rs`**

```rust
use crate::model::ToolCall;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Ask,
    Deny(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Policy;

impl Policy {
    pub fn decide(&self, call: &ToolCall) -> Decision {
        match call.name.as_str() {
            "read_file" | "list_files" | "search_text" | "git_diff" | "git_status" => Decision::Allow,
            "write_file" | "apply_patch" => Decision::Ask,
            "run_command" => {
                let command = call.arguments.get("command").and_then(Value::as_str).unwrap_or("");
                deny_reason(command).map(Decision::Deny).unwrap_or(Decision::Ask)
            }
            _ => Decision::Deny(format!("tool '{}' is not permitted by the built-in policy", call.name)),
        }
    }
}

fn deny_reason(command: &str) -> Option<String> {
    let normalized = command.to_ascii_lowercase();
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let root_rm = words.first() == Some(&"rm")
        && words.iter().any(|w| *w == "/" || w.starts_with("/../"))
        && words.iter().any(|w| w.starts_with('-') && w.contains('r') && w.contains('f'));
    let forbidden = words.first() == Some(&"sudo")
        || root_rm
        || words.iter().any(|w| w.starts_with("mkfs"))
        || words.first() == Some(&"fdisk")
        || (normalized.contains("diskutil") && normalized.contains("erasedisk"))
        || (words.first() == Some(&"git") && words.get(1) == Some(&"push"))
        || ["> /", ">/", ">> /", ">>/"].iter().any(|p| normalized.contains(p));
    forbidden.then(|| "command matches the hard denylist".to_string())
}
```

- [ ] **Step 4: Write approval tests in `approval.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Stub { answer: bool, calls: AtomicUsize }
    impl Approver for Stub {
        fn confirm(&self, _call: &ToolCall) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer
        }
    }
    fn call() -> ToolCall { ToolCall { id: "1".into(), name: "write_file".into(), arguments: json!({}) } }

    #[test]
    fn modes_resolve_ask_safely() {
        assert!(!ApprovalMode::NonInteractive.allows(&call()));
        assert!(ApprovalMode::AutoApprove.allows(&call()));
        assert!(ApprovalMode::Interactive(Box::new(Stub { answer: true, calls: AtomicUsize::new(0) })).allows(&call()));
        assert!(!ApprovalMode::Interactive(Box::new(Stub { answer: false, calls: AtomicUsize::new(0) })).allows(&call()));
    }
}
```

- [ ] **Step 5: Implement `approval.rs`**

```rust
use crate::model::ToolCall;
use std::io::{self, IsTerminal, Write};

pub trait Approver: Send + Sync {
    fn confirm(&self, call: &ToolCall) -> bool;
}

pub enum ApprovalMode {
    Interactive(Box<dyn Approver>),
    NonInteractive,
    AutoApprove,
}

impl ApprovalMode {
    pub fn allows(&self, call: &ToolCall) -> bool {
        match self {
            Self::Interactive(a) => a.confirm(call),
            Self::NonInteractive => false,
            Self::AutoApprove => true,
        }
    }

    pub fn terminal(auto_approve: bool) -> Self {
        if auto_approve { Self::AutoApprove }
        else if io::stdin().is_terminal() { Self::Interactive(Box::new(TerminalApprover)) }
        else { Self::NonInteractive }
    }
}

pub struct TerminalApprover;
impl Approver for TerminalApprover {
    fn confirm(&self, call: &ToolCall) -> bool {
        eprint!("Approve {} {}? [y/N] ", call.name, call.arguments);
        if io::stderr().flush().is_err() { return false; }
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() { return false; }
        matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}
```

- [ ] **Step 6: Declare and export both modules in `lib.rs`**

Add `mod approval; mod policy;`, then add:

```rust
pub use approval::{ApprovalMode, Approver, TerminalApprover};
pub use policy::{Decision, Policy};
```

- [ ] **Step 7: Run and commit**

Run: `rtk cargo test -p quecto-agent --lib`

Expected: PASS.

```bash
rtk git add quecto-agent/src/policy.rs quecto-agent/src/approval.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add fixed approval policy"
```

---

### Task 2: Unix sandbox with bounded output, timeout, redaction, and cancellation

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Create: `quecto-agent/src/sandbox.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces: `CancelToken = Arc<AtomicBool>` and `cancel_token() -> CancelToken`.
- Produces: `Sandbox::new(repo_root, cancel)`, `Sandbox::run(command) -> Result<CommandOutput, ToolError>`.
- Produces: `CommandOutput { status, stdout, stderr, timed_out, cancelled }` and `render()`.

- [ ] **Step 1: Add dependencies**

```toml
ctrlc = "3"
libc = "0.2"
```

- [ ] **Step 2: Create sandbox tests before implementation**

Place these at the bottom of `sandbox.rs`; declare the module in `lib.rs` with `mod sandbox;` so the failing test compiles far enough to identify missing APIs.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};

    #[test]
    fn runs_at_repo_root_and_captures_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_secs(2))
            .run("pwd; printf err >&2; exit 7").unwrap();
        assert_eq!(out.status, Some(7));
        assert_eq!(out.stdout.trim(), dir.path().canonicalize().unwrap().display().to_string());
        assert_eq!(out.stderr, "err");
    }

    #[test]
    fn caps_and_redacts_output() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("QUECTO_TEST_SECRET_TOKEN", "m4-secret-value");
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_output_cap(64)
            .run("printf '%s' \"$QUECTO_TEST_SECRET_TOKEN\"; yes x | head -c 256").unwrap();
        std::env::remove_var("QUECTO_TEST_SECRET_TOKEN");
        assert!(!out.stdout.contains("m4-secret-value"));
        assert!(out.stdout.contains("[REDACTED]"));
        assert!(out.stdout.contains("truncated"));
    }

    #[test]
    fn timeout_kills_descendant_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("late.txt");
        let command = format!("(sleep 1; touch '{}') & wait", marker.display());
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_millis(100)).run(&command).unwrap();
        assert!(out.timed_out);
        thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists());
    }

    #[test]
    fn cancellation_kills_running_group() {
        let dir = tempfile::tempdir().unwrap();
        let token = cancel_token();
        let setter = token.clone();
        thread::spawn(move || { thread::sleep(Duration::from_millis(80)); setter.store(true, Ordering::SeqCst); });
        let out = Sandbox::new(dir.path().to_path_buf(), token)
            .with_timeout(Duration::from_secs(2)).run("sleep 10").unwrap();
        assert!(out.cancelled);
    }
}
```

- [ ] **Step 3: Run tests and verify failure**

Run: `rtk cargo test -p quecto-agent --lib sandbox -- --test-threads=1`

Expected: FAIL because sandbox types are missing.

- [ ] **Step 4: Implement `sandbox.rs`**

```rust
#[cfg(not(unix))]
compile_error!("quecto-agent M4 requires a Unix target");

use crate::tools::ToolError;
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub type CancelToken = Arc<AtomicBool>;
pub fn cancel_token() -> CancelToken { Arc::new(AtomicBool::new(false)) }

pub struct Sandbox { repo_root: PathBuf, cancel: CancelToken, timeout: Duration, output_cap: usize }
#[derive(Debug)]
pub struct CommandOutput { pub status: Option<i32>, pub stdout: String, pub stderr: String, pub timed_out: bool, pub cancelled: bool }

impl CommandOutput {
    pub fn render(&self) -> String {
        format!("exit_status: {}\ntimed_out: {}\ncancelled: {}\nstdout:\n{}\nstderr:\n{}",
            self.status.map(|n| n.to_string()).unwrap_or_else(|| "signal".into()), self.timed_out, self.cancelled, self.stdout, self.stderr)
    }
}

impl Sandbox {
    pub fn new(repo_root: PathBuf, cancel: CancelToken) -> Self {
        Self { repo_root: repo_root.canonicalize().unwrap_or(repo_root), cancel, timeout: Duration::from_secs(120), output_cap: 32 * 1024 }
    }
    #[cfg(test)] fn with_timeout(mut self, timeout: Duration) -> Self { self.timeout = timeout; self }
    #[cfg(test)] fn with_output_cap(mut self, cap: usize) -> Self { self.output_cap = cap; self }

    pub fn run(&self, command: &str) -> Result<CommandOutput, ToolError> {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(command).current_dir(&self.repo_root).stdout(Stdio::piped()).stderr(Stdio::piped());
        unsafe { cmd.pre_exec(|| { if libc::setpgid(0, 0) == -1 { return Err(std::io::Error::last_os_error()); } Ok(()) }); }
        let mut child = cmd.spawn().map_err(|e| ToolError::new(format!("spawn: {e}")))?;
        let pgid = child.id() as i32;
        let mut stdout = child.stdout.take().ok_or_else(|| ToolError::new("stdout pipe unavailable"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| ToolError::new("stderr pipe unavailable"))?;
        let out_reader = thread::spawn(move || { let mut b = Vec::new(); let _ = stdout.read_to_end(&mut b); b });
        let err_reader = thread::spawn(move || { let mut b = Vec::new(); let _ = stderr.read_to_end(&mut b); b });
        let started = Instant::now();
        let (status, timed_out, cancelled) = loop {
            if let Some(status) = child.try_wait().map_err(|e| ToolError::new(format!("wait: {e}")))? { break (status.code(), false, false); }
            let cancelled = self.cancel.load(Ordering::SeqCst);
            let timed_out = started.elapsed() >= self.timeout;
            if cancelled || timed_out {
                unsafe { libc::kill(-pgid, libc::SIGKILL); }
                let status = child.wait().map_err(|e| ToolError::new(format!("reap: {e}")))?;
                break (status.code(), timed_out, cancelled);
            }
            thread::sleep(Duration::from_millis(20));
        };
        let stdout = out_reader.join().map_err(|_| ToolError::new("stdout reader panicked"))?;
        let stderr = err_reader.join().map_err(|_| ToolError::new("stderr reader panicked"))?;
        Ok(CommandOutput {
            status,
            stdout: self.clean(&stdout),
            stderr: self.clean(&stderr),
            timed_out,
            cancelled,
        })
    }

    fn clean(&self, bytes: &[u8]) -> String {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        let mut seen = HashSet::new();
        for (name, value) in std::env::vars() {
            let upper = name.to_ascii_uppercase();
            if !value.is_empty() && ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|p| upper.contains(p)) && seen.insert(value.clone()) {
                text = text.replace(&value, "[REDACTED]");
            }
        }
        cap_output_head_tail(&text, self.output_cap)
    }
}

fn cap_output_head_tail(text: &str, cap: usize) -> String {
    if text.len() <= cap { return text.to_string(); }
    let half = cap / 2;
    let head_end = (0..=half).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let tail_start = text.len().saturating_sub(half);
    let tail_start = (tail_start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    format!("{}\n[… {} bytes truncated …]\n{}", &text[..head_end], tail_start.saturating_sub(head_end), &text[tail_start..])
}
```

- [ ] **Step 5: Export sandbox APIs and run tests**

Add to `lib.rs`:

```rust
pub use sandbox::{cancel_token, CancelToken, CommandOutput, Sandbox};
```

Run: `rtk cargo test -p quecto-agent --lib sandbox -- --test-threads=1`

Expected: PASS, including descendant-process cleanup.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/Cargo.toml quecto-agent/src/sandbox.rs quecto-agent/src/lib.rs Cargo.lock
rtk git commit -m "feat(agent): add bounded Unix command sandbox"
```

---

### Task 3: `run_command` tool and context wiring

**Files:**
- Create: `quecto-agent/src/tools/shell.rs`
- Modify: `quecto-agent/src/tools/mod.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- `Context::new(repo_root, cancel)` replaces the one-argument constructor.
- `Context::run_command(&self, command) -> ToolResult` delegates to its `Sandbox`.
- Produces: `pub struct RunCommand` implementing `Tool`.

- [ ] **Step 1: Write `RunCommand` tests in `tools/shell.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use serde_json::json;

    #[test]
    fn schema_requires_only_command() {
        let schema = RunCommand.schema();
        assert_eq!(schema["required"], json!(["command"]));
        assert!(schema["properties"].get("cwd").is_none());
    }

    #[test]
    fn runs_and_reports_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = RunCommand.run(&json!({"command":"printf hello"}), &mut cx).unwrap();
        assert!(out.content.contains("exit_status: 0"));
        assert!(out.content.contains("hello"));
    }

    #[test]
    fn missing_command_is_a_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(RunCommand.run(&json!({}), &mut cx).is_err());
    }
}
```

- [ ] **Step 2: Run and verify failure**

Run: `rtk cargo test -p quecto-agent --lib shell`

Expected: FAIL because `RunCommand` and the new `Context` constructor do not exist.

- [ ] **Step 3: Add sandbox state to `Context` in `tools/mod.rs`**

Add `use crate::sandbox::{CancelToken, Sandbox};`, add `sandbox: Sandbox` to `Context`, and replace its constructor with:

```rust
pub fn new(repo_root: PathBuf, cancel: CancelToken) -> Self {
    let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
    Context { sandbox: Sandbox::new(repo_root.clone(), cancel), repo_root, changes: Vec::new() }
}

pub fn run_command(&self, command: &str) -> ToolResult {
    let output = self.sandbox.run(command)?;
    let summary = if output.cancelled { "cancelled" } else if output.timed_out { "timed out" } else { "command finished" };
    Ok(ToolOutput::new(output.render(), summary))
}
```

Update every existing test and production call from `Context::new(path)` to `Context::new(path, cancel_token())`; import `crate::sandbox::cancel_token` in each affected test module.

- [ ] **Step 4: Implement `tools/shell.rs`**

```rust
use super::{Context, Tool, ToolError, ToolResult};
use serde_json::{json, Value};

pub struct RunCommand;
impl Tool for RunCommand {
    fn name(&self) -> &str { "run_command" }
    fn description(&self) -> &str { "Run a shell command at the repository root with timeout, cancellation, bounded output, and approval." }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string","description":"Command passed to /bin/sh -c"}},"required":["command"],"additionalProperties":false})
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let command = args.get("command").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::new("run_command requires a non-empty string 'command'"))?;
        cx.run_command(command)
    }
}
```

Declare `pub mod shell;`, add `Box::new(shell::RunCommand)` to `builtin_tools()`, and export `RunCommand` from `lib.rs`.

- [ ] **Step 5: Run all library tests and commit**

Run: `rtk cargo test -p quecto-agent --lib -- --test-threads=1`

Expected: PASS.

```bash
rtk git add quecto-agent/src/tools/shell.rs quecto-agent/src/tools/mod.rs quecto-agent/src/lib.rs quecto-agent/src/tools/fs.rs quecto-agent/src/tools/git.rs quecto-agent/src/tools/search.rs quecto-agent/src/tools/patch.rs quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): add run_command tool"
```

---

### Task 4: Central policy gate in the agent loop

**Files:**
- Modify: `quecto-agent/src/agent.rs`

**Interfaces:**
- `Agent::new(model, system, max_steps, repo_root, cancel, approval)` replaces the M3 constructor.
- `Policy::decide` always runs before `Registry::dispatch`.
- Denials append `denied: <reason>` as the tool result and do not run the tool.

- [ ] **Step 1: Add agent tests using the existing `Recording` tool**

```rust
#[test]
fn ask_tool_is_denied_without_interactivity() {
    let ran = Arc::new(AtomicBool::new(false));
    let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
    let mut a = configured_agent(model, ApprovalMode::NonInteractive)
        .register(Box::new(RecordingNamed { name: "write_file", ran: ran.clone() }));
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn auto_approve_runs_ask_tool_but_not_hard_denies() {
    let ran = Arc::new(AtomicBool::new(false));
    let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
    let mut a = configured_agent(model, ApprovalMode::AutoApprove)
        .register(Box::new(RecordingNamed { name: "write_file", ran: ran.clone() }));
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn unknown_custom_tool_is_denied_even_if_registered() {
    let ran = Arc::new(AtomicBool::new(false));
    let model = Scripted::new(vec![wants_tool("custom"), text("done")]);
    let mut a = configured_agent(model, ApprovalMode::AutoApprove)
        .register(Box::new(RecordingNamed { name: "custom", ran: ran.clone() }));
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    assert!(!ran.load(Ordering::SeqCst));
}
```

Define `RecordingNamed { name: &'static str, ran: Arc<AtomicBool> }` exactly like existing `Recording`, returning its `name` field. Define:

```rust
fn configured_agent(model: Scripted, approval: ApprovalMode) -> Agent {
    Agent::new(Box::new(model), "sys", 10, PathBuf::from("."), cancel_token(), approval)
}
```

- [ ] **Step 2: Run and verify failure**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: FAIL because the constructor and gate are not implemented.

- [ ] **Step 3: Add `policy`, `approval`, and `cancel` fields to `Agent`**

```rust
policy: Policy,
approval: ApprovalMode,
cancel: CancelToken,
```

Update the constructor to accept `cancel: CancelToken, approval: ApprovalMode`; pass `cancel.clone()` to `Context::new`, store the token, and initialize `Policy`.

- [ ] **Step 4: Replace direct dispatch with the central decision match**

```rust
let out = match self.policy.decide(call) {
    Decision::Allow => self.registry.dispatch(call, &mut self.cx),
    Decision::Ask if self.approval.allows(call) => self.registry.dispatch(call, &mut self.cx),
    Decision::Ask => ToolOutput::new("denied: approval required", "denied"),
    Decision::Deny(reason) => ToolOutput::new(format!("denied: {reason}"), "denied"),
};
```

Keep activity output and tool-result history behavior unchanged.

- [ ] **Step 5: Run and commit**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS.

```bash
rtk git add quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): enforce policy before tool dispatch"
```

---

### Task 5: Cancellation outcomes and repeated-action protection

**Files:**
- Modify: `quecto-agent/src/agent.rs`

**Interfaces:**
- Adds `Outcome::Cancelled` and `Outcome::RepeatedAction`.
- Adds private `RepeatGuard::observe(call, result, change_count) -> bool`.

- [ ] **Step 1: Add cancellation and repeat tests**

```rust
#[test]
fn pre_cancelled_agent_stops_before_model_call() {
    let token = cancel_token();
    token.store(true, Ordering::SeqCst);
    let mut a = Agent::new(Box::new(Scripted::new(vec![text("unused")])), "sys", 10, PathBuf::from("."), token, ApprovalMode::NonInteractive);
    assert!(matches!(a.run("hi"), Outcome::Cancelled));
}

#[test]
fn three_identical_no_change_observations_stop() {
    let replies = vec![wants_tool("read_file"), wants_tool("read_file"), wants_tool("read_file")];
    let mut a = configured_agent(Scripted::new(replies), ApprovalMode::NonInteractive)
        .register(Box::new(StaticNamed { name: "read_file", content: "same" }));
    assert!(matches!(a.run("hi"), Outcome::RepeatedAction));
}

#[test]
fn file_change_resets_repeat_streak() {
    let mut guard = RepeatGuard::default();
    let call = ToolCall { id: "1".into(), name: "read_file".into(), arguments: json!({"path":"a"}) };
    assert!(!guard.observe(&call, "same", 0));
    assert!(!guard.observe(&call, "same", 0));
    assert!(!guard.observe(&call, "same", 1));
    assert!(!guard.observe(&call, "same", 1));
    assert!(guard.observe(&call, "same", 1));
}
```

`StaticNamed` implements `Tool` and returns `ToolOutput::new(self.content, "same")` without mutation.

- [ ] **Step 2: Run and verify failure**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: FAIL because the outcomes and `RepeatGuard` are missing.

- [ ] **Step 3: Implement `RepeatGuard`**

```rust
#[derive(Default)]
struct RepeatGuard { fingerprint: Option<String>, changes: usize, streak: usize }
impl RepeatGuard {
    fn observe(&mut self, call: &crate::model::ToolCall, result: &str, changes: usize) -> bool {
        let fingerprint = format!("{}\n{}\n{}", call.name, canonical_json(&call.arguments), result);
        if self.fingerprint.as_deref() == Some(&fingerprint) && self.changes == changes { self.streak += 1; }
        else { self.fingerprint = Some(fingerprint); self.changes = changes; self.streak = 1; }
        self.streak >= 3
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect(); keys.sort();
            format!("{{{}}}", keys.into_iter().map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&map[k]))).collect::<Vec<_>>().join(","))
        }
        serde_json::Value::Array(items) => format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        _ => value.to_string(),
    }
}
```

- [ ] **Step 4: Integrate both guards**

At the start of `run`, create `let mut repeats = RepeatGuard::default();`. Before each model call and each tool call, return `Outcome::Cancelled` if the token is set. After dispatch and before appending the result, return `Outcome::Cancelled` if the sandbox set cancellation; otherwise call:

```rust
if repeats.observe(call, &out.content, self.cx.changes().len()) {
    self.messages.push(Message::tool_result(&call.id, out.content));
    return Outcome::RepeatedAction;
}
```

Add the two variants to `Outcome`.

- [ ] **Step 5: Run and commit**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS.

```bash
rtk git add quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): stop cancellation and repeated actions"
```

---

### Task 6: CLI `--yes`, Ctrl-C registration, and end-to-end verification

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- `--yes` may appear anywhere and is removed before task joining.
- Ctrl-C sets the same token passed to `Agent`.
- New outcomes print stable error messages and exit 1.

- [ ] **Step 1: Add CLI tests**

```rust
#[test]
fn yes_flag_is_removed_from_the_user_task() {
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin()).args(["--yes", "do", "it"])
        .env("QUECTO_BASE_URL", &base).env("QUECTO_MODEL", "m")
        .env_remove("QUECTO_API_KEY").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    let body = request.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert!(body.contains("do it"));
    assert!(!body.contains("--yes"));
}

#[test]
fn yes_without_task_is_usage_error() {
    let out = Command::new(bin()).arg("--yes").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
```

Add this sibling helper to `tests/common/mod.rs`; it deliberately duplicates the small server setup so existing tests keep their current API:

```rust
pub fn mock_capture(status: u16, content_type: &str, body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let reason = if status == 200 { "OK" } else { "ERROR" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let request = read_request_capture(&mut stream);
            let _ = tx.send(request);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), rx)
}

fn read_request_capture(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(header) = find_subslice(&buf, b"\r\n\r\n") {
            let end = header + 4;
            let headers = String::from_utf8_lossy(&buf[..end]);
            let length = headers.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
            }).unwrap_or(0);
            while buf.len() < end + length {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 { break; }
                buf.extend_from_slice(&chunk[..n]);
            }
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}
```

Change the test import to `use common::{mock, mock_capture};`.

- [ ] **Step 2: Run and verify failure**

Run: `rtk cargo test -p quecto-agent --test cli`

Expected: `yes_without_task_is_usage_error` FAILS before parsing is changed.

- [ ] **Step 3: Update `main.rs`**

```rust
let mut args: Vec<String> = std::env::args().skip(1).collect();
let auto_approve = args.iter().any(|arg| arg == "--yes");
args.retain(|arg| arg != "--yes");
if args.is_empty() {
    eprintln!("usage: quecto-agent [--yes] \"<task>\"");
    std::process::exit(2);
}
let task = args.join(" ");
let cancel = quecto_agent::cancel_token();
let signal_cancel = cancel.clone();
if let Err(e) = ctrlc::set_handler(move || signal_cancel.store(true, std::sync::atomic::Ordering::SeqCst)) {
    eprintln!("quecto-agent: failed to install Ctrl-C handler: {e}");
    std::process::exit(1);
}
let approval = quecto_agent::ApprovalMode::terminal(auto_approve);
```

Pass `cancel` and `approval` to `Agent::new`. Extend the outcome match:

```rust
Outcome::Cancelled => { eprintln!("quecto-agent: cancelled"); std::process::exit(1); }
Outcome::RepeatedAction => { eprintln!("quecto-agent: repeated action detected"); std::process::exit(1); }
```

- [ ] **Step 4: Run formatting and complete verification**

Run: `rtk cargo fmt --all -- --check`

Expected: PASS. If it fails, run `rtk cargo fmt --all`, then rerun the check.

Run: `rtk cargo test -p quecto-agent -- --test-threads=1`

Expected: PASS, including sandbox timeout/process-group tests.

Run: `rtk cargo test --workspace -- --test-threads=1`

Expected: PASS for both `quecto` and `quecto-agent`.

Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS with no warnings.

Run: `rtk git diff --check`

Expected: no output and exit 0.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs quecto-agent/tests/common/mod.rs
rtk git commit -m "feat(agent): expose approval and cancellation in CLI"
```

---

## Final Acceptance Checklist

- [ ] M3 editing work is committed before M4 commits; no M4 commit accidentally absorbs unrelated dirty files.
- [ ] All built-in tool calls are classified centrally before dispatch.
- [ ] Non-interactive mode denies edits and commands; `--yes` permits them.
- [ ] Hard-denied commands remain denied under `--yes`.
- [ ] `run_command` has no `cwd` parameter and executes at the canonical repository root.
- [ ] Timeout and cancellation kill descendant processes and reap the child.
- [ ] Captured stdout/stderr are independently bounded and secrets are redacted best effort.
- [ ] Ctrl-C maps to `Outcome::Cancelled` and exit code 1.
- [ ] Three identical no-change observations map to `Outcome::RepeatedAction`; a file change resets the streak.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work

M5 owns verification commands and instruction/context loading. M6 owns session persistence, undo/diff commands, chat, and rich rendering. M7 owns flavor-configured policies, trusted verification bypass, tool allow-lists, `ask_user`, and richer CLI parsing. Windows process-tree support requires a separate design.
