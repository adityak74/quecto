# QuECTO Agent Markdown Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render assistant markdown in interactive TTY sessions while preserving plain raw output for non-interactive stdout.

**Architecture:** Add a small markdown rendering helper in the existing renderer module and reuse it for both interactive chat output and one-shot completion output. Gate formatting on terminal-aware call sites so piped output remains unchanged and existing plain renderer tests keep their legacy expectations.

**Tech Stack:** Rust, `quecto-agent`, `crossterm`, lightweight terminal markdown rendering crate, existing renderer/unit test stack

## Global Constraints

- Only interactive TTY sessions should render markdown.
- Piped or redirected output must remain byte-for-byte plain text.
- Existing tests that rely on plain renderer behavior should keep passing.
- The implementation should stay localized to the rendering and output layer.
- The dependency footprint should stay small.

---

### Task 1: Add TTY-only assistant markdown rendering

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Modify: `quecto-agent/src/render.rs`
- Modify: `quecto-agent/src/lib.rs`
- Modify: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: `Renderer::assistant(&mut self, text: &str)`, `finish(outcome: Outcome, store_status: Option<(&Store, &str)>)`
- Produces: `render_assistant_text(text: &str, markdown: bool) -> String`, exported for reuse by the one-shot completion path

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn render_assistant_text_preserves_plain_output_when_markdown_disabled() {
    let input = "# Title\n\n- item\n\n```rust\nfn main() {}\n```";
    assert_eq!(render_assistant_text(input, false), input);
}

#[test]
fn render_assistant_text_formats_markdown_when_enabled() {
    let rendered = render_assistant_text("# Title\n\n- item", true);
    assert!(rendered.contains("Title"));
    assert!(rendered.contains("item"));
    assert_ne!(rendered, "# Title\n\n- item");
}
```

- [ ] **Step 2: Run the targeted test to verify it fails**

Run: `cargo test -p quecto-agent render_assistant_text_ -- --nocapture`

Expected: FAIL with unresolved function `render_assistant_text` or equivalent missing-symbol error.

- [ ] **Step 3: Add the markdown dependency**

Update `quecto-agent/Cargo.toml`:

```toml
[dependencies]
crossterm = "0.27"
termimad = "0.35"
quecto = { path = ".." }
```

Keep the rest of the dependency list unchanged.

- [ ] **Step 4: Implement the rendering helper in `quecto-agent/src/render.rs`**

Add a shared helper and use it from both renderer implementations:

```rust
pub fn render_assistant_text(text: &str, markdown: bool) -> String {
    if !markdown {
        return text.to_string();
    }

    let skin = termimad::MadSkin::default();
    skin.term_text(text)
}
```

Update `LineRenderer::assistant()` and `SpinnerRenderer::assistant()` to call:

```rust
let rendered = render_assistant_text(text, self.color);
let _ = writeln!(self.out, "{rendered}");
```

For the spinner renderer, keep the existing `stop_spinner()` call before writing output.

- [ ] **Step 5: Export the helper for reuse**

Update `quecto-agent/src/lib.rs`:

```rust
pub use render::{
    chat_spinner_renderer, parse_spinner_verbs, render_assistant_text, stderr_renderer,
    LineRenderer, Renderer,
};
```

- [ ] **Step 6: Reuse the helper in the one-shot completion path**

Update `finish()` in `quecto-agent/src/main.rs` so completed assistant output is rendered only when stdout is a TTY:

```rust
Outcome::Complete(answer) => {
    let rendered = render_assistant_text(&answer, std::io::stdout().is_terminal());
    println!("{rendered}");
    "done"
}
```

Do not change any non-complete outcome handling.

- [ ] **Step 7: Add stable unit tests in `quecto-agent/src/render.rs`**

Add tests that assert:

```rust
#[test]
fn render_assistant_text_preserves_plain_output_when_markdown_disabled() { /* ... */ }

#[test]
fn render_assistant_text_formats_markdown_when_enabled() { /* ... */ }

#[test]
fn plain_notice_and_assistant_are_raw_text() { /* existing test remains unchanged */ }
```

Keep the markdown-enabled assertions structural:
- check that heading/list content is present
- check that the rendered output differs from the raw markdown source
- avoid asserting exact wrapping width or ANSI sequences

- [ ] **Step 8: Run focused tests**

Run: `cargo test -p quecto-agent render_assistant_text_ plain_notice_and_assistant_are_raw_text -- --nocapture`

Expected: PASS

- [ ] **Step 9: Run the full crate test suite**

Run: `cargo test -p quecto-agent`

Expected: PASS

- [ ] **Step 10: Manual interactive verification**

Run in a terminal TTY:

```bash
cargo run -p quecto-agent -- chat
```

Then enter a prompt that elicits markdown, such as:

```text
Give me a markdown answer with a heading, a bullet list, and a fenced rust code block.
```

Expected:
- interactive chat displays formatted terminal markdown
- `/reasoning` and other chat commands still work
- no raw markdown punctuation is required for readability

Then verify non-interactive output remains plain:

```bash
printf 'Give me a markdown answer with a heading and list.\n/exit\n' | cargo run -p quecto-agent -- chat
```

Expected:
- output remains raw/plain markdown text
- no interactive terminal formatting assumptions

- [ ] **Step 11: Commit**

```bash
git add quecto-agent/Cargo.toml quecto-agent/src/render.rs quecto-agent/src/lib.rs quecto-agent/src/main.rs
git commit -m "feat(agent): render markdown in interactive tty output"
```

## Self-Review

- Spec coverage: This plan covers TTY-only formatting, shared renderer-layer integration, one-shot completion reuse, and regression protection for non-TTY output.
- Placeholder scan: No TODO/TBD markers remain.
- Type consistency: `render_assistant_text(text: &str, markdown: bool) -> String` is the single shared helper referenced by renderer code and `finish()`.
