# QuECTO Agent Mermaid Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render Mermaid fenced code blocks inline in interactive TTY assistant output using `merman` Unicode box-drawing output, while preserving existing markdown behavior and leaving non-TTY output unchanged.

**Architecture:** Extend the existing assistant rendering path in `quecto-agent/src/render.rs` with a Mermaid preprocessing stage that detects fenced `mermaid` blocks, tries to render them with `merman`, and leaves the original fence unchanged on failure. Reuse the same helper for chat and one-shot interactive output, and keep the plain non-TTY path byte-for-byte unchanged.

**Tech Stack:** Rust, `quecto-agent`, `termimad`, `merman`, existing renderer/unit test stack

## Global Constraints

- Mermaid rendering must only run in interactive TTY sessions.
- Non-TTY output must remain byte-for-byte plain markdown text.
- Unicode box-drawing output is the default terminal style.
- If a Mermaid block cannot be parsed or rendered, the original fenced code block must be preserved unchanged.
- The implementation should stay localized to the assistant rendering path.

---

### Task 1: Add TTY-only Mermaid fence rendering with `merman`

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Modify: `quecto-agent/src/render.rs`
- Modify: `quecto-agent/src/lib.rs`
- Modify: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: `render_assistant_text(text: &str, markdown: bool) -> String`
- Produces: Mermaid preprocessing inside the shared assistant rendering path before markdown rendering
- Produces: helper(s) that detect fenced Mermaid blocks and replace renderable blocks with Unicode terminal output

- [ ] **Step 1: Write the failing tests for Mermaid preprocessing**

Add tests in `quecto-agent/src/render.rs` for:

```rust
#[test]
fn render_assistant_text_preserves_plain_output_when_markdown_disabled() {
    let input = "```mermaid\ngraph TD\nA --> B\n```";
    assert_eq!(render_assistant_text(input, false), input);
}

#[test]
fn render_assistant_text_renders_simple_mermaid_when_enabled() {
    let rendered = render_assistant_text("```mermaid\ngraph TD\nA --> B\n```", true);
    assert!(rendered.contains("A"));
    assert!(rendered.contains("B"));
    assert!(rendered.contains('┌') || rendered.contains('│') || rendered.contains('─'));
    assert!(!rendered.contains("```mermaid"));
}

#[test]
fn render_assistant_text_preserves_invalid_mermaid_block_when_enabled() {
    let input = "```mermaid\nthis is not valid mermaid\n```";
    assert_eq!(render_assistant_text(input, true), input);
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test -p quecto-agent render_assistant_text_ -- --nocapture`

Expected: FAIL because Mermaid fences are not currently rendered and invalid Mermaid currently goes through `termimad` instead of content-preserving fallback behavior.

- [ ] **Step 3: Add the `merman` dependency**

Update `quecto-agent/Cargo.toml`:

```toml
[dependencies]
crossterm = "0.27"
termimad = "0.35"
merman = { version = "0.7", features = ["ascii"] }
quecto = { path = ".." }
```

Keep the rest of the dependency list unchanged.

- [ ] **Step 4: Implement Mermaid fence detection and rendering helpers**

In `quecto-agent/src/render.rs`, add helpers with responsibilities along these lines:

```rust
fn preprocess_mermaid_blocks(text: &str) -> String
fn try_render_mermaid_block(source: &str) -> Option<String>
```

Implementation requirements:
- detect fenced code blocks whose info string is exactly `mermaid`
- extract only the fence body for rendering
- use `merman` terminal text rendering APIs to produce Unicode output
- on successful render, replace the full fenced block with rendered diagram text
- on parse/render failure, preserve the original fenced block exactly

Use `merman` terminal text output APIs rather than shelling out to any external binary.

- [ ] **Step 5: Integrate Mermaid preprocessing into the shared assistant renderer**

Update `render_assistant_text(...)` in `quecto-agent/src/render.rs` so the TTY path becomes:

```rust
pub fn render_assistant_text(text: &str, markdown: bool) -> String {
    if !markdown {
        return text.to_string();
    }

    let preprocessed = preprocess_mermaid_blocks(text);
    let skin = termimad::MadSkin::default();
    skin.term_text(&preprocessed).to_string()
}
```

Do not change the non-TTY branch.

- [ ] **Step 6: Keep the shared output path wired through chat and one-shot completion**

Ensure these existing paths remain intact:
- `LineRenderer::assistant()` uses `render_assistant_text(text, self.color)`
- `SpinnerRenderer::assistant()` uses `render_assistant_text(text, self.color)`
- `finish()` in `quecto-agent/src/main.rs` uses `render_assistant_text(&answer, std::io::stdout().is_terminal())`

Only adjust `quecto-agent/src/lib.rs` exports if any new helper must be shared. Do not introduce duplicate Mermaid handling in `main.rs`.

- [ ] **Step 7: Add regression tests for mixed markdown and Mermaid content**

Add a test in `quecto-agent/src/render.rs` for mixed content:

```rust
#[test]
fn render_assistant_text_keeps_markdown_rendering_after_mermaid_preprocessing() {
    let rendered = render_assistant_text(
        "# Title\n\n```mermaid\ngraph TD\nA --> B\n```\n\n- item",
        true,
    );
    assert!(rendered.contains("Title"));
    assert!(rendered.contains("item"));
    assert!(rendered.contains("A"));
    assert!(rendered.contains("B"));
}
```

Keep assertions structural and avoid exact layout or width-sensitive expectations.

- [ ] **Step 8: Run focused tests**

Run: `cargo test -p quecto-agent render_assistant_text_ -- --nocapture`

Expected: PASS

- [ ] **Step 9: Run the full crate test suite**

Run: `cargo test -p quecto-agent`

Expected: PASS

- [ ] **Step 10: Manual interactive verification**

Run in a terminal TTY:

```bash
cargo run -p quecto-agent -- chat
```

Then enter a prompt that elicits Mermaid and markdown, such as:

```text
Give me a markdown answer with a heading, a mermaid flowchart code block from A to B, and a bullet list.
```

Expected:
- interactive chat shows the Mermaid block as Unicode diagram output inline
- surrounding markdown still renders readably
- `/reasoning` and other chat commands still work

Then verify fallback behavior with invalid Mermaid:

```text
Give me this exact fenced block:
```mermaid
this is not valid mermaid
```
```

Expected:
- the original fenced Mermaid block is preserved rather than replaced with an error string

Then verify non-interactive output remains plain:

```bash
printf 'Give me a mermaid flowchart from A to B.\n/exit\n' | cargo run -p quecto-agent -- chat
```

Expected:
- output remains raw/plain markdown text
- Mermaid is not rendered in non-TTY mode

- [ ] **Step 11: Commit**

```bash
git add quecto-agent/Cargo.toml quecto-agent/src/render.rs quecto-agent/src/lib.rs quecto-agent/src/main.rs
git commit -m "feat(agent): render mermaid in interactive tty output"
```

## Self-Review

- Spec coverage: This plan covers TTY-only Mermaid rendering, `merman` integration, fence-preserving fallback behavior, shared renderer-layer reuse, and regression coverage for markdown and non-TTY output.
- Placeholder scan: No TODO/TBD markers remain.
- Type consistency: `render_assistant_text(text: &str, markdown: bool) -> String` remains the single shared assistant rendering entry point.
