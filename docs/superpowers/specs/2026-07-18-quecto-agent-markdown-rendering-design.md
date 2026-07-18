# QuECTO Agent Markdown Rendering Design

## Goal

Render assistant markdown output in `quecto-agent` only for interactive TTY sessions, while preserving exact plain-text output for non-interactive and piped use.

## Scope

This change applies to assistant responses shown in the terminal by `quecto-agent`.

In scope:
- Interactive `chat` sessions backed by a TTY.
- Interactive one-shot agent runs where final assistant output is printed to a TTY.
- Rendering common markdown constructs more readably in the terminal.

Out of scope:
- Changing stored message content.
- Changing non-interactive stdout behavior.
- Reformatting tool, notice, or verifier lines.
- Full-screen TUI work.

## Constraints

- Only interactive TTY sessions should render markdown.
- Piped or redirected output must remain byte-for-byte plain text.
- Existing tests that rely on plain renderer behavior should keep passing.
- The implementation should be localized to the rendering/output layer, not the model or session layers.
- The dependency footprint should stay small.

## Approaches Considered

### 1. TTY-only markdown rendering in the renderer layer

Add markdown-aware assistant rendering in `quecto-agent/src/render.rs`, enabled only when the output sink is a terminal.

Pros:
- Centralizes display behavior where assistant output is already emitted.
- Lets chat and one-shot interactive flows share the same behavior.
- Keeps non-TTY paths unchanged.

Cons:
- Requires a small renderer abstraction expansion or helper path for the one-shot `finish()` flow.

### 2. Markdown rendering only at call sites

Render markdown in `main.rs` where chat and one-shot output are printed.

Pros:
- Small surface area.

Cons:
- Duplicates behavior across multiple call sites.
- Easier for future output paths to bypass rendering accidentally.

## Decision

Use approach 1.

Assistant output rendering should be handled in the renderer layer and gated on terminal detection. The one-shot completion path in `main.rs` should route through the same markdown-aware helper used by the interactive renderer so all interactive assistant output is consistent.

## Design

### Rendering behavior

When stdout is a TTY:
- Render assistant markdown with terminal formatting.
- Preserve paragraph separation.
- Render fenced code blocks distinctly and keep code text unchanged.
- Render bullet and numbered lists readably.
- Render headings with visible emphasis.

When stdout is not a TTY:
- Emit the original assistant text unchanged.

### Library choice

Use a lightweight terminal markdown renderer rather than building ad hoc markdown parsing. The dependency should support terminal-friendly formatting from markdown strings and degrade cleanly to plain text when disabled.

### Integration points

- `quecto-agent/src/render.rs`
  - Add a helper for assistant text rendering that either formats markdown for terminals or returns the raw string for plain output.
  - Update interactive renderers' `assistant()` implementation to use that helper.
- `quecto-agent/src/main.rs`
  - Update `finish()` so one-shot completion output uses the same helper when stdout is a TTY.

### Testing

Add tests that cover:
- Plain renderer output remains unchanged when markdown rendering is disabled.
- Markdown rendering path transforms representative markdown for terminal display.
- Non-interactive output remains raw text.

## Risks

- Terminal markdown libraries may insert wrapping or ANSI output that makes assertions brittle.
- Some markdown constructs may render differently across terminal widths.

## Risk Mitigation

- Keep assertions focused on stable structural behavior rather than exact wrapping width.
- Restrict the feature to interactive TTY mode only.
- Preserve the existing plain-text path for non-TTY output and legacy tests.
