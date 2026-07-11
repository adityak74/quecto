# quecto-agent M5 — Instruction Loader, Seed Context, and Verification Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Feed the model real repository context (layered `AGENTS.md`/`CLAUDE.md` rules plus a seeded file tree and git state) and turn the agent's stop condition into a verification gate that runs pre-declared commands and loops test-and-fix until they pass.

**Architecture:** Three pure, independently testable units feed the agent. `instructions::load` walks repo-root→cwd collecting instruction files (nearer wins). `context::seed` builds a one-time repo snapshot (shallow tree + `git status`/`git diff`). `verify::Verifier` runs fixed commands through the existing sandbox and reports pass/fail. `main.rs` composes the first two into the system prompt; the agent loop consults the `Verifier` as a completion gate: when the model stops with edits present, failing checks are fed back as an observation and the loop continues.

**Tech Stack:** Rust 2021; existing `ignore` crate for the tree; `git` via `std::process`; the existing `Sandbox`. No new dependencies.

## Global Constraints

- Build on the completed M4 APIs exactly as present: `Context::new(repo_root, cancel)`, `Context::changes()`, the private `Sandbox`, `CommandOutput { status, .. }` and its `render()`, and `Agent::new(model, system, max_steps, repo_root, cancel, approval)`.
- Flavor configuration is **M7**: M5 verification commands are **not** flavor-configured. They come from the `QUECTO_VERIFY` environment variable (newline-separated), mirroring the existing `QUECTO_SYSTEM`/`QUECTO_MAX_STEPS` pattern in `main.rs`. No language auto-detection.
- Verification commands **bypass the approval prompt** (they are operator-declared) but still run inside the sandbox (timeout, process-group kill, output cap, redaction). A non-zero exit is a failure.
- The verification gate only runs when **edits exist** (`Context::changes()` is non-empty) and a verifier is configured. With no edits or no verifier, the model's stop completes the run unchanged.
- A failing gate feeds one observation back and continues the loop; progress is bounded by the existing `max_steps` (returns `Outcome::StepLimit` if never green).
- Instruction files searched, in order: `AGENTS.md`, `CLAUDE.md`, `.agent/instructions.md`. Root-first, cwd-last concatenation so nearer files appear last (take precedence when read top-to-bottom). Empty files are skipped. If none exist, contribute nothing.
- Seed context and instruction sections are byte-capped with the existing `cap_output` helper so they cannot blow the context budget.
- Preserve unrelated changes; stage and commit only the files named by each task. No new crates.
- Run repository shell commands through `rtk` per `AGENTS.md`.

---

## File Structure

- `quecto-agent/src/instructions.rs` — `load(repo_root, cwd) -> Option<String>`; directory-chain walk.
- `quecto-agent/src/context.rs` — `seed(repo_root) -> String`; shallow tree + git status/diff.
- `quecto-agent/src/verify.rs` — `Verifier`, `VerifyReport`, `VerifyResult`.
- `quecto-agent/src/tools/mod.rs` — add `Context::run_verify(command) -> Result<CommandOutput, ToolError>` (raw sandbox access for the verifier).
- `quecto-agent/src/agent.rs` — `verifier: Option<Verifier>` field, `with_verifier` builder, completion-gate logic.
- `quecto-agent/src/lib.rs` — declare/export the three new modules' public items.
- `quecto-agent/src/main.rs` — compose system prompt from base + instructions + seed; parse `--no-verify`; attach `Verifier::from_env`.
- `quecto-agent/tests/cli.rs` — verify `--no-verify` is stripped from the task; keep capture tests cwd-deterministic.

---

### Task 1: Raw sandbox access on `Context`

**Files:**
- Modify: `quecto-agent/src/tools/mod.rs`

**Interfaces:**
- Consumes: private `Context.sandbox: Sandbox`, `Sandbox::run(&str) -> Result<CommandOutput, ToolError>`.
- Produces: `Context::run_verify(&self, command: &str) -> Result<CommandOutput, ToolError>`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `tools/mod.rs`:

```rust
#[test]
fn run_verify_exposes_exit_status() {
    let dir = tempdir().unwrap();
    let cx = Context::new(dir.path().to_path_buf(), cancel_token());
    let ok = cx.run_verify("exit 0").unwrap();
    assert_eq!(ok.status, Some(0));
    let bad = cx.run_verify("exit 3").unwrap();
    assert_eq!(bad.status, Some(3));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p quecto-agent --lib run_verify`

Expected: FAIL — no method `run_verify` on `Context`.

- [ ] **Step 3: Add the method and import**

Change the sandbox import line in `tools/mod.rs` to also bring in `CommandOutput`:

```rust
use crate::sandbox::{CancelToken, CommandOutput, Sandbox};
```

Add this method to `impl Context` (next to `run_command`):

```rust
/// Run a pre-declared verification command through the sandbox, exposing the
/// raw exit status. Unlike `run_command`, this does not wrap the output for a
/// tool result; the verification gate reads `status` directly.
pub fn run_verify(&self, command: &str) -> Result<CommandOutput, ToolError> {
    self.sandbox.run(command)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `rtk cargo test -p quecto-agent --lib run_verify`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/tools/mod.rs
rtk git commit -m "feat(agent): expose raw sandbox run for verification"
```

---

### Task 2: Verifier and report

**Files:**
- Create: `quecto-agent/src/verify.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `Context::run_verify`, `CommandOutput { status, .. }`, `CommandOutput::render()`.
- Produces:
  - `VerifyResult { command: String, passed: bool, output: String }`
  - `VerifyReport { results: Vec<VerifyResult> }` with `all_passed(&self) -> bool` and `observation(&self) -> String`.
  - `Verifier` with `new(Vec<String>) -> Self`, `from_env() -> Option<Self>`, `is_empty(&self) -> bool`, `run(&self, cx: &Context) -> VerifyReport`.

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/verify.rs`:

```rust
use crate::tools::Context;

/// Outcome of running one verification command.
pub struct VerifyResult {
    pub command: String,
    pub passed: bool,
    pub output: String,
}

/// Aggregate of every verification command for one gate check.
pub struct VerifyReport {
    pub results: Vec<VerifyResult>,
}

impl VerifyReport {
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|r| r.passed)
    }

    /// A model-facing observation summarizing the failed checks.
    pub fn observation(&self) -> String {
        let mut out =
            String::from("Verification failed. Fix the reported problems and finish again.\n");
        for r in self.results.iter().filter(|r| !r.passed) {
            out.push_str(&format!("\n$ {}\n{}\n", r.command, r.output));
        }
        out
    }
}

/// Fixed (non-flavor) verification commands run as a completion gate. Commands
/// bypass the approval prompt but still execute inside the sandbox.
pub struct Verifier {
    commands: Vec<String>,
}

impl Verifier {
    pub fn new(commands: Vec<String>) -> Self {
        Verifier {
            commands: commands
                .into_iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
        }
    }

    /// Parse newline-separated commands from `QUECTO_VERIFY`. Returns `None`
    /// when unset or effectively empty.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("QUECTO_VERIFY").ok()?;
        let v = Verifier::new(raw.lines().map(|l| l.to_string()).collect());
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Run every command through the sandbox; a non-zero (or signal) exit fails.
    pub fn run(&self, cx: &Context) -> VerifyReport {
        let results = self
            .commands
            .iter()
            .map(|command| match cx.run_verify(command) {
                Ok(out) => VerifyResult {
                    command: command.clone(),
                    passed: out.status == Some(0),
                    output: out.render(),
                },
                Err(e) => VerifyResult {
                    command: command.clone(),
                    passed: false,
                    output: format!("error: {}", e.message),
                },
            })
            .collect();
        VerifyReport { results }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use tempfile::tempdir;

    fn cx() -> Context {
        Context::new(tempdir().unwrap().path().to_path_buf(), cancel_token())
    }

    #[test]
    fn new_trims_and_drops_blank_commands() {
        let v = Verifier::new(vec!["  ".into(), "echo hi".into(), "".into()]);
        assert!(!v.is_empty());
        let report = v.run(&cx());
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].command, "echo hi");
    }

    #[test]
    fn all_passed_true_when_every_command_exits_zero() {
        let report = Verifier::new(vec!["exit 0".into(), "true".into()]).run(&cx());
        assert!(report.all_passed());
    }

    #[test]
    fn failure_is_flagged_and_summarized() {
        let report = Verifier::new(vec!["exit 0".into(), "exit 1".into()]).run(&cx());
        assert!(!report.all_passed());
        let obs = report.observation();
        assert!(obs.contains("Verification failed"));
        assert!(obs.contains("$ exit 1"));
        assert!(!obs.contains("$ exit 0"));
    }

    #[test]
    fn empty_verifier_is_reported_empty() {
        assert!(Verifier::new(vec![]).is_empty());
        assert!(Verifier::new(vec!["   ".into()]).is_empty());
    }
}
```

- [ ] **Step 2: Declare the module and run to verify failure**

Add `mod verify;` to `lib.rs` (module list) so the test compiles.

Run: `rtk cargo test -p quecto-agent --lib verify`

Expected: FAIL initially only if the module is not declared; once declared the tests should compile. If they fail to compile, fix imports before proceeding. Target: all four tests PASS after the module is declared.

- [ ] **Step 3: Export public items from `lib.rs`**

Add:

```rust
pub use verify::{VerifyReport, VerifyResult, Verifier};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib verify`

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/verify.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add sandboxed verification runner"
```

---

### Task 3: Instruction loader

**Files:**
- Create: `quecto-agent/src/instructions.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces: `instructions::load(repo_root: &Path, cwd: &Path) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/instructions.rs`:

```rust
use std::path::{Path, PathBuf};

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".agent/instructions.md"];

/// Collect instruction files walking from `repo_root` down to `cwd`. Root-level
/// files come first and nearer (deeper) files come last, so when read
/// top-to-bottom the nearest instructions take precedence. Empty files are
/// skipped. Returns `None` when nothing is found.
pub fn load(repo_root: &Path, cwd: &Path) -> Option<String> {
    let mut sections = Vec::new();
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    for dir in dir_chain(&root, cwd) {
        for name in INSTRUCTION_FILES {
            let path = dir.join(name);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if text.trim().is_empty() {
                continue;
            }
            let label = path.strip_prefix(&root).unwrap_or(&path);
            sections.push(format!("## {}\n{}", label.display(), text.trim_end()));
        }
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Directories from `root` to `cwd` inclusive, root first. If `cwd` is not a
/// descendant of `root`, only `root` is returned.
fn dir_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let here = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let rel = match here.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => return vec![root.to_path_buf()],
    };
    let mut chain = vec![root.to_path_buf()];
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur = cur.join(comp);
        chain.push(cur.clone());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn none_when_no_instruction_files() {
        let dir = tempdir().unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }

    #[test]
    fn loads_root_agents_md() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
        let out = load(dir.path(), dir.path()).unwrap();
        assert!(out.contains("## AGENTS.md"));
        assert!(out.contains("root rules"));
    }

    #[test]
    fn nearer_file_appears_after_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
        let sub = dir.path().join("crate");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("AGENTS.md"), "crate rules").unwrap();
        let out = load(dir.path(), &sub).unwrap();
        let root_at = out.find("root rules").unwrap();
        let crate_at = out.find("crate rules").unwrap();
        assert!(root_at < crate_at, "nearer file must come last");
    }

    #[test]
    fn empty_files_are_skipped() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "   \n").unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }
}
```

- [ ] **Step 2: Declare the module and run to verify**

Add `mod instructions;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib instructions`

Expected: PASS (4 tests).

- [ ] **Step 3: Export `load`**

Add to `lib.rs`:

```rust
pub use instructions::load as load_instructions;
```

- [ ] **Step 4: Confirm the crate still builds**

Run: `rtk cargo test -p quecto-agent --lib instructions`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/instructions.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add layered instruction loader"
```

---

### Task 4: Seed context

**Files:**
- Create: `quecto-agent/src/context.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `ignore::WalkBuilder`, `crate::tools::cap_output`, `git` via `std::process`.
- Produces: `context::seed(repo_root: &Path) -> String`.

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/context.rs`:

```rust
use crate::tools::cap_output;
use std::path::Path;

/// Build the one-time seed context injected into the first system message:
/// the repo root, a shallow `.gitignore`-aware file tree, and `git status` /
/// `git diff` when the directory is a git repository.
pub fn seed(repo_root: &Path) -> String {
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut out = format!("# Repository context\nroot: {}\n", root.display());
    out.push_str("\n## Files (depth 2, .gitignore-aware)\n");
    out.push_str(&cap_output(&file_tree(&root), 8_000));
    if let Some(status) = git(&root, &["status", "--porcelain"]) {
        let status = status.trim_end();
        let status = if status.is_empty() { "clean" } else { status };
        out.push_str("\n\n## git status\n");
        out.push_str(&cap_output(status, 4_000));
    }
    if let Some(diff) = git(&root, &["diff"]) {
        let diff = diff.trim_end();
        if !diff.is_empty() {
            out.push_str("\n\n## git diff\n");
            out.push_str(&cap_output(diff, 16_000));
        }
    }
    out
}

fn file_tree(root: &Path) -> String {
    let mut entries = Vec::new();
    for dent in ignore::WalkBuilder::new(root)
        .require_git(false)
        .standard_filters(true)
        .max_depth(Some(2))
        .build()
        .flatten()
    {
        if dent.depth() == 0 {
            continue;
        }
        let shown = dent.path().strip_prefix(root).unwrap_or(dent.path());
        entries.push(shown.display().to_string());
        if entries.len() >= 300 {
            break;
        }
    }
    entries.sort();
    entries.join("\n")
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn seed_lists_files_and_marks_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        let out = seed(dir.path());
        assert!(out.contains("# Repository context"));
        assert!(out.contains("main.rs"));
    }

    #[test]
    fn seed_respects_gitignore() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(dir.path().join("kept.rs"), "x").unwrap();
        fs::write(dir.path().join("secret.txt"), "x").unwrap();
        let out = seed(dir.path());
        assert!(out.contains("kept.rs"));
        assert!(!out.contains("secret.txt"));
    }

    #[test]
    fn seed_omits_git_sections_without_a_repo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "x").unwrap();
        let out = seed(dir.path());
        assert!(!out.contains("## git status"));
        assert!(!out.contains("## git diff"));
    }
}
```

- [ ] **Step 2: Declare the module and run**

Add `mod context;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib context`

Expected: PASS (3 tests). Note: `.gitignore` is honored by the `ignore` crate even without an initialized repo because `standard_filters(true)` reads ignore files directly.

- [ ] **Step 3: Export `seed`**

Add to `lib.rs`:

```rust
pub use context::seed as seed_context;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib context`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/context.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add seed repository context"
```

---

### Task 5: Verification completion gate in the agent loop

**Files:**
- Modify: `quecto-agent/src/agent.rs`

**Interfaces:**
- Consumes: `Verifier`, `VerifyReport`, `Context::changes()`, `Context::run_verify`.
- Produces: `Agent.verifier: Option<Verifier>` and `Agent::with_verifier(self, Verifier) -> Self`.
- Behavior: when the model stops (`tool_calls` empty) with edits present and a verifier configured, run it; on failure push `report.observation()` as a user message and continue; on success (or no edits / no verifier) return `Outcome::Complete`.

- [ ] **Step 1: Add gate tests**

Add to the `tests` module in `agent.rs`. Reuse the existing `WriteFile` import pattern from `agent_write_file_flows_through_the_loop`:

```rust
#[test]
fn verify_gate_passes_returns_complete() {
    use crate::tools::fs::WriteFile;
    let dir = tempfile::tempdir().unwrap();
    let write = AssistantMessage {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({"path":"a.txt","content":"hi\n"}),
        }],
        finish_reason: "tool_calls".into(),
    };
    let model = Scripted::new(vec![write, text("done")]);
    let mut a = Agent::new(
        Box::new(model),
        "sys",
        10,
        dir.path().to_path_buf(),
        cancel_token(),
        ApprovalMode::AutoApprove,
    )
    .register(Box::new(WriteFile))
    .with_verifier(crate::verify::Verifier::new(vec!["exit 0".into()]));
    match a.run("edit") {
        Outcome::Complete(s) => assert_eq!(s, "done"),
        _ => panic!("expected Complete after passing verification"),
    }
}

#[test]
fn verify_gate_failure_loops_until_step_limit() {
    use crate::tools::fs::WriteFile;
    let dir = tempfile::tempdir().unwrap();
    let write = AssistantMessage {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({"path":"a.txt","content":"hi\n"}),
        }],
        finish_reason: "tool_calls".into(),
    };
    // After the edit the model keeps trying to stop; the failing gate re-prompts
    // each time until max_steps is hit.
    let model = Scripted::new(vec![write, text("done"), text("still"), text("more")]);
    let mut a = Agent::new(
        Box::new(model),
        "sys",
        3,
        dir.path().to_path_buf(),
        cancel_token(),
        ApprovalMode::AutoApprove,
    )
    .register(Box::new(WriteFile))
    .with_verifier(crate::verify::Verifier::new(vec!["exit 1".into()]));
    assert!(matches!(a.run("edit"), Outcome::StepLimit));
}

#[test]
fn verify_gate_skipped_without_edits() {
    let model = Scripted::new(vec![text("hi")]);
    let mut a = configured_agent(model, ApprovalMode::NonInteractive)
        .with_verifier(crate::verify::Verifier::new(vec!["exit 1".into()]));
    match a.run("nothing to change") {
        Outcome::Complete(s) => assert_eq!(s, "hi"),
        _ => panic!("no edits means the gate must not run"),
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib agent::tests::verify`

Expected: FAIL — `with_verifier` and the gate do not exist.

- [ ] **Step 3: Add the field and builder**

Add the import at the top of `agent.rs`:

```rust
use crate::verify::Verifier;
```

Add the field to `struct Agent` (after `approval`):

```rust
    verifier: Option<Verifier>,
```

Initialize it in `Agent::new` (in the struct literal, after `approval,`):

```rust
            verifier: None,
```

Add the builder method in `impl Agent` (next to `register_builtins`):

```rust
    /// Attach a completion-gate verifier. Its commands run (bypassing approval)
    /// whenever the model stops with edits present.
    pub fn with_verifier(mut self, verifier: Verifier) -> Self {
        self.verifier = Some(verifier);
        self
    }
```

- [ ] **Step 4: Insert the gate at the stop condition**

In `run`, replace the current stop block:

```rust
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
```

with:

```rust
            if msg.tool_calls.is_empty() {
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            eprintln!(
                                "● verify {}  {}",
                                r.command,
                                if r.passed { "passed" } else { "failed" }
                            );
                        }
                        if !report.all_passed() {
                            self.messages.push(Message::user(report.observation()));
                            step += 1;
                            continue;
                        }
                    }
                }
                return Outcome::Complete(msg.content);
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS (all agent tests, including the three new gate tests).

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): gate completion on verification"
```

---

### Task 6: CLI wiring — compose context, load instructions, `--no-verify`

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Consumes: `quecto_agent::{load_instructions, seed_context, Verifier}`.
- Behavior: `--no-verify` may appear anywhere and is stripped before task joining; without it, `Verifier::from_env()` (from `QUECTO_VERIFY`) is attached when present. The system prompt becomes base + `[repo rules]` + seed context.

- [ ] **Step 1: Add CLI tests**

Add to `quecto-agent/tests/cli.rs`. The capture tests run in a fresh temp dir so the seeded `git diff` cannot vary the request body:

```rust
#[test]
fn no_verify_flag_is_removed_from_the_user_task() {
    let dir = tempfile::tempdir().unwrap();
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["--no-verify", "do", "it"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    let body = request
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body.contains("do it"));
    assert!(!body.contains("--no-verify"));
}
```

Ensure the import line reads `use common::{mock, mock_capture};` (add `mock_capture` if the M4 tests already brought in `mock`). Confirm `tempfile` resolves in this integration test (it is a dev-dependency).

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli no_verify`

Expected: FAIL — `--no-verify` is currently forwarded into the task, so `body.contains("--no-verify")` is true (assertion fails) or the flag ends up in the joined task.

- [ ] **Step 3: Update `main.rs`**

After the existing `--yes` handling and before the empty-args check, add `--no-verify` stripping:

```rust
    let no_verify = args.iter().any(|arg| arg == "--no-verify");
    args.retain(|arg| arg != "--no-verify");
```

Replace the system-prompt construction:

```rust
    let system = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
```

with a composed prompt that layers repo rules and seed context:

```rust
    let base = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let mut system = base;
    if let Some(rules) = quecto_agent::load_instructions(&cwd, &cwd) {
        system.push_str("\n\n# Repository rules\n");
        system.push_str(&rules);
    }
    system.push_str("\n\n");
    system.push_str(&quecto_agent::seed_context(&cwd));
```

Then attach the verifier when enabled. Change the agent construction so it is `mut` and conditionally gains a verifier. Replace:

```rust
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps,
        repo_root,
        cancel,
        approval,
    )
    .register_builtins();
```

with:

```rust
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps,
        repo_root,
        cancel,
        approval,
    )
    .register_builtins();
    if !no_verify {
        if let Some(verifier) = quecto_agent::Verifier::from_env() {
            agent = agent.with_verifier(verifier);
        }
    }
```

Note: `repo_root` is already `std::env::current_dir()`; reuse `cwd` conceptually but keep the existing `repo_root` binding as-is for the agent. (They are the same directory; do not remove `repo_root`.)

- [ ] **Step 4: Run to verify the new test passes**

Run: `rtk cargo test -p quecto-agent --test cli`

Expected: PASS for all CLI tests, including `no_verify_flag_is_removed_from_the_user_task`.

- [ ] **Step 5: Full verification**

Run: `rtk cargo fmt --all -- --check`
Expected: PASS (run `rtk cargo fmt --all` then re-check if it fails).

Run: `rtk cargo test --workspace -- --test-threads=1`
Expected: PASS for both `quecto` and `quecto-agent`.

Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

Run: `rtk git diff --check`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs
rtk git commit -m "feat(agent): load instructions, seed context, and --no-verify"
```

---

## Final Acceptance Checklist

- [ ] `Context::run_verify` exposes the raw exit status; the verifier treats non-zero as failure.
- [ ] `QUECTO_VERIFY` (newline-separated) is the only source of M5 verification commands; flavor config stays deferred to M7.
- [ ] Verification commands bypass approval but run inside the sandbox.
- [ ] The gate runs only with edits present and a verifier configured; otherwise the model's stop completes the run.
- [ ] A failing gate feeds one observation back and continues, bounded by `max_steps`.
- [ ] Instruction files layer root→cwd with nearer files last; empty files skipped; none ⇒ no section.
- [ ] Seed context lists a shallow `.gitignore`-aware tree and omits git sections outside a repo.
- [ ] `--no-verify` is stripped from the task and disables the gate.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work

M6 owns SQLite session persistence, `resume`/`undo`/`diff` subcommands, chat mode, and the rich renderer. M7 owns flavor-configured verification (`[verify]` commands, `auto_verify`, required-check selection), flavor policies, tool allow-lists, and `ask_user`. Windows support and richer CLI parsing (`clap` subcommands) remain out of scope.
