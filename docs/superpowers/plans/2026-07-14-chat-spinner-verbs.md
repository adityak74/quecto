# Chat REPL Spinner Verbs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a TTY-only, Claude-like loading spinner with built-in or `QUECTO_SPINNER_VERBS`-configured verbs to `quecto-agent chat` only.

**Architecture:** Extend the existing `Renderer` lifecycle with start/stop working hooks. `Agent::run_loop` brackets every model request with those hooks, while `LineRenderer` owns a small background spinner that writes temporary status to stdout and joins/clears it before normal output. Keep the core `quecto` REPL untouched and keep verb parsing pure/testable.

**Tech Stack:** Rust 2021, existing `quecto-agent` renderer, `std::thread`, `std::sync::atomic`, `crossterm` already in the agent crate, Cargo tests.

## Global Constraints

- Scope is only `quecto-agent chat`; do not alter the core `quecto` REPL.
- Do not add a dependency or async runtime.
- `QUECTO_SPINNER_VERBS` is a comma-separated replacement list; trim whitespace, ignore empty entries, and fall back to built-in defaults when no usable entries remain.
- Built-in verbs use the full supplied playful verb list stored in `DEFAULT_SPINNER_VERBS`.
- Spinner output is disabled for non-TTY output and is never recorded or sent to the model.

---

### Task 1: Add pure verb configuration and spinner renderer lifecycle

**Files:**
- Modify: `quecto-agent/src/render.rs`
- Modify: `quecto-agent/src/lib.rs` (export the pure parser only if tests need the public boundary; otherwise keep it module-private)
- Test: `quecto-agent/src/render.rs` unit tests

**Interfaces:**
- Produces `parse_spinner_verbs(raw: Option<&str>) -> Vec<String>` with the built-in fallback behavior.
- Produces `Renderer::working(&mut self)` and `Renderer::working_done(&mut self)` hooks with default no-op bodies so existing renderer implementations remain valid.
- Produces a `LineRenderer<std::io::Stdout>` constructor/configuration path used by chat to enable the spinner only when stdout is a TTY.

- [ ] **Step 1: Write failing parser tests**

Add focused tests alongside the existing renderer tests:

```rust
#[test]
fn spinner_verbs_use_compact_defaults_when_unconfigured() {
    assert_eq!(
        parse_spinner_verbs(None),
        vec!["Thinking", "Working", "Crafting", "Computing", "Pondering", "Wrangling"]
    );
}

#[test]
fn spinner_verbs_trim_and_ignore_empty_custom_entries() {
    assert_eq!(
        parse_spinner_verbs(Some(" Brewing, , Refactoring ,, ")),
        vec!["Brewing", "Refactoring"]
    );
}

#[test]
fn spinner_verbs_fall_back_when_custom_value_has_no_entries() {
    assert_eq!(parse_spinner_verbs(Some(" ,  , "))[0], "Thinking");
}
```

- [ ] **Step 2: Run the parser tests and verify they fail for the missing parser**

Run:

```bash
rtk cargo test -p quecto-agent render::tests::spinner_verbs
```

Expected: FAIL because `parse_spinner_verbs` does not exist yet.

- [ ] **Step 3: Implement the minimal pure parser**

In `render.rs`, define the default slice and implement parsing without reading process environment state:

```rust
const DEFAULT_SPINNER_VERBS: &[&str] =
    &["Thinking", "Working", "Crafting", "Computing", "Pondering", "Wrangling"];

fn parse_spinner_verbs(raw: Option<&str>) -> Vec<String> {
    let verbs: Vec<String> = raw
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    if verbs.is_empty() {
        DEFAULT_SPINNER_VERBS.iter().map(|v| (*v).to_string()).collect()
    } else {
        verbs
    }
}
```

Use `parse_spinner_verbs(std::env::var("QUECTO_SPINNER_VERBS").ok().as_deref())` at the chat-only construction site in Task 2 rather than making global env reads part of the parser.

- [ ] **Step 4: Write failing spinner lifecycle tests**

Add deterministic tests for two private helpers that do not need a real terminal or a sleeping thread:

```rust
#[test]
fn spinner_frame_contains_frame_and_verb() {
    assert_eq!(format_spinner_frame("⠋", "Brewing"), "\r⠋ Brewing…");
}

#[test]
fn spinner_clear_sequence_erases_the_temporary_line() {
    assert_eq!(SPINNER_CLEAR, "\r\x1b[2K");
}
```

Also add a renderer test that calls `working()` and `working_done()` on a spinner-disabled renderer and asserts its existing plain tool/notice/assistant output remains byte-for-byte unchanged. Keep the tests independent of `IsTerminal` and timing.

The renderer test should also verify that a renderer created without spinner configuration preserves the existing plain tool/notice/assistant output byte-for-byte.

- [ ] **Step 5: Run the lifecycle tests and verify the expected missing-type/method failure**

Run:

```bash
rtk cargo test -p quecto-agent render::tests
```

Expected: FAIL because the spinner helper and renderer lifecycle hooks are not implemented.

- [ ] **Step 6: Implement the minimal spinner and renderer hooks**

Add a private spinner state in `render.rs` using an `Arc<AtomicBool>` stop flag and a `JoinHandle<()>`. Define the deterministic formatting constants/helpers used by the tests:

```rust
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_CLEAR: &str = "\r\x1b[2K";

fn format_spinner_frame(frame: &str, verb: &str) -> String {
    format!("\r{frame} {verb}…")
}
```

The runtime spinner should:

```rust
trait Renderer: Send {
    fn working(&mut self);
    fn working_done(&mut self);
    fn tool(&mut self, name: &str, summary: &str);
    fn verify(&mut self, command: &str, passed: bool);
    fn notice(&mut self, text: &str);
    fn assistant(&mut self, text: &str);
}
```

`LineRenderer` should stop any active spinner before every normal output method, clear the temporary line with `SPINNER_CLEAR`, and ignore write errors as it does today. `working()` should be a no-op unless spinner support was explicitly enabled; otherwise it starts one thread that cycles `SPINNER_FRAMES` and the configured verb list at a modest fixed interval. `working_done()` sets the stop flag, joins the thread, and clears the line. Make the spinner state optional so existing stderr/stdout renderers and non-TTY paths remain unchanged. The spinner thread may own a separate `std::io::Stdout` handle because only the chat `LineRenderer<std::io::Stdout>` enables it; normal renderer writes happen after `working_done()` joins the thread.

- [ ] **Step 7: Run renderer tests and refactor only after green**

Run:

```bash
rtk cargo test -p quecto-agent render::tests
```

Expected: all renderer tests pass, including the pre-existing formatting/color tests and the new parser/lifecycle tests. Remove only duplication that is exposed by the green implementation.

- [ ] **Step 8: Commit the renderer unit**

```bash
rtk git add quecto-agent/src/render.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(chat): add configurable spinner verbs"
```

### Task 2: Bracket chat model calls and document the environment variable

**Files:**
- Modify: `quecto-agent/src/agent.rs:run_loop`
- Modify: `quecto-agent/src/main.rs:chat`
- Modify: `README.md` environment-variable table and chat configuration example
- Test: `quecto-agent/src/agent.rs` renderer test fixture

**Interfaces:**
- Consumes `Renderer::working` and `Renderer::working_done` from Task 1.
- Produces chat-only spinner activation with `QUECTO_SPINNER_VERBS`; one-shot/resume/default stderr rendering remains non-spinning unless explicitly configured by existing construction behavior.

- [ ] **Step 1: Write a failing agent renderer lifecycle test**

Extend the existing `CaptureRenderer` in `quecto-agent/src/agent.rs` with a vector of event names and add a test model that returns one assistant message. Assert the event sequence is `working`, `working_done`, then the normal completion path. The test should use the existing `Agent::new(...).with_renderer(...)` path and not make an HTTP request.

- [ ] **Step 2: Run the focused test and verify it fails because the agent does not call the hooks**

Run:

```bash
rtk cargo test -p quecto-agent agent::tests::model_call_brackets_renderer_working_state
```

Expected: FAIL with the captured event list missing `working` and `working_done`.

- [ ] **Step 3: Add the smallest model-call bracket**

In `Agent::run_loop`, call `self.renderer.working()` immediately before `self.model.complete(...)`, then call `self.renderer.working_done()` immediately after the result returns and before matching the result. This guarantees the spinner is stopped on both successful and failed model calls:

```rust
self.renderer.working();
let completed = self.model.complete(&self.messages, &schemas);
self.renderer.working_done();
let msg = match completed {
    Ok(msg) => msg,
    Err(e) => break Outcome::Error(e),
};
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
rtk cargo test -p quecto-agent agent::tests::model_call_brackets_renderer_working_state
```

Expected: PASS.

- [ ] **Step 5: Enable the spinner only for interactive chat stdout**

In `chat`, construct the agent's stdout `LineRenderer` with spinner enabled only when `std::io::stdout().is_terminal()` and pass that renderer into `Agent::with_renderer`. Keep the existing separate plain `LineRenderer` used by the chat loop for prompts, command notices, and assistant text; `working_done()` runs before `agent.run()` returns, so the temporary spinner line is cleared before that separate renderer writes. Read `QUECTO_SPINNER_VERBS` once at chat startup and pass the parsed list into the spinner configuration.

Do not modify the core crate's `src/main.rs`, `src/lib.rs`, or its REPL code. Do not enable the spinner in `run`, `resume`, or one-shot output.

- [ ] **Step 6: Document the environment variable**

Add `QUECTO_SPINNER_VERBS` to the `quecto-agent` environment table in `README.md`, documenting the comma-separated replacement behavior and the compact defaults. Include one shell example alongside the existing local/cloud setup examples.

- [ ] **Step 7: Run focused tests and the complete workspace suite**

Run:

```bash
rtk cargo test -p quecto-agent
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands exit 0; existing core REPL tests remain unchanged and pass.

- [ ] **Step 8: Commit the integration and docs**

```bash
rtk git add quecto-agent/src/agent.rs quecto-agent/src/main.rs README.md
rtk git commit -m "feat(chat): show spinner while model is working"
```

## Plan self-review

- Spec coverage: chat-only scope, compact defaults, env replacement parsing, TTY-only output, temporary clearing, no dependency, no recording/model contamination, tests, and documentation are covered by Tasks 1 and 2.
- Placeholder scan: no TODO/TBD or unspecified implementation steps remain.
- Type consistency: Task 1 defines the renderer hooks and parser consumed by Task 2; Task 2 uses the existing `LineRenderer`/`Agent` APIs and updates the known `CaptureRenderer` fixture.
- Scope check: one renderer unit plus one agent integration/documentation unit; the core crate is explicitly excluded.
