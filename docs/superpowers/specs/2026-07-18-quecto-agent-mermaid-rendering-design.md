# QuECTO Agent Mermaid Rendering Design

## Goal

Render Mermaid fenced code blocks inline in `quecto-agent` interactive TTY sessions using Unicode box-drawing output, while preserving existing markdown behavior and leaving non-interactive output unchanged.

## Scope

This change applies to assistant responses shown in `quecto-agent`.

In scope:
- Interactive TTY chat sessions.
- Interactive TTY one-shot completion output.
- Mermaid rendering only for fenced code blocks marked with the `mermaid` language tag.
- Unicode terminal rendering through a Rust-native Mermaid renderer.

Out of scope:
- Rendering Mermaid outside fenced code blocks.
- Changing stored messages or session persistence.
- Rendering diagrams in non-interactive or piped output.
- Adding image generation or SVG output to terminal chat.
- Reformatting tool, verifier, or notice lines.

## Constraints

- Mermaid rendering must only run in interactive TTY sessions.
- Non-TTY output must remain byte-for-byte plain markdown text.
- Unicode box-drawing output is the default terminal style.
- If a Mermaid block cannot be parsed or rendered, the original fenced code block must be preserved unchanged.
- The implementation should stay localized to the assistant rendering path.

## Approaches Considered

### 1. Preprocess Mermaid fences inside the renderer layer

Detect Mermaid fenced blocks before terminal markdown rendering, try to render them with a Rust-native Mermaid library, replace successful blocks inline, and leave failures unchanged.

Pros:
- Keeps Mermaid behavior in the same place as the current markdown rendering feature.
- Reuses the same output behavior for chat and one-shot TTY completions.
- Preserves the existing plain-text fallback path for non-TTY output.

Cons:
- Requires a preprocessing step before markdown rendering.

### 2. Handle Mermaid at call sites in `main.rs`

Render Mermaid blocks in chat and one-shot output call sites before invoking the renderer.

Pros:
- Small conceptual surface area.

Cons:
- Duplicates output logic across code paths.
- Easier for future output paths to miss Mermaid rendering.

### 3. Shell out to an external Mermaid renderer

Use an external binary such as `mmdc` or a terminal Mermaid CLI and capture its output.

Pros:
- Potentially broader syntax coverage depending on the tool.

Cons:
- Adds runtime dependencies outside the Rust crate.
- Complicates installation, portability, and failure handling.
- Less appropriate for inline terminal rendering than an in-process Rust library.

## Decision

Use approach 1 with `merman`.

Mermaid rendering should be integrated into the renderer layer as a TTY-only preprocessing step. `merman` should be used to attempt Unicode terminal rendering for fenced Mermaid blocks. If rendering succeeds, the rendered diagram replaces the block inline. If it fails, the original fenced block remains unchanged.

## Design

### Rendering pipeline

When assistant output is destined for an interactive TTY:
1. Scan the text for fenced `mermaid` code blocks.
2. For each Mermaid block:
   - try to parse and render it with `merman`
   - on success, replace the fenced block with rendered Unicode text
   - on failure, keep the original fenced block exactly as written
3. Pass the transformed text through the existing markdown renderer.

When output is not going to a TTY:
- Skip Mermaid preprocessing entirely.
- Emit the original assistant text unchanged.

### Library choice

Use `merman` because it is a Rust-native, headless Mermaid implementation with terminal-friendly text output. This avoids external CLI dependencies and fits the existing Rust rendering path.

### Integration points

- `quecto-agent/Cargo.toml`
  - add the `merman` dependency with terminal text rendering support
- `quecto-agent/src/render.rs`
  - add Mermaid fence detection and replacement helpers
  - keep `render_assistant_text(...)` as the single entry point for TTY assistant rendering
  - run Mermaid preprocessing before markdown rendering
- `quecto-agent/src/lib.rs`
  - export any helper that must be shared by one-shot completion output
- `quecto-agent/src/main.rs`
  - continue using the shared assistant rendering helper for one-shot TTY output

### Fallback behavior

Fallback must be silent and content-preserving:
- renderable Mermaid becomes inline Unicode output
- non-renderable Mermaid remains as the original fenced code block

No placeholder notice should replace failed diagrams because preserving the original source is more useful to the user than a generic error string.

### Testing

Add tests that cover:
- plain non-TTY output remains unchanged
- a simple Mermaid fence is replaced with rendered Unicode terminal output
- an invalid or unsupported Mermaid block remains unchanged
- normal markdown rendering still works after Mermaid preprocessing

Assertions should stay structural:
- confirm rendered output contains expected node labels and Unicode diagram characters
- confirm fallback retains the original fenced block text
- avoid assertions that depend on exact wrapping width

## Risks

- `merman` may not fully support every Mermaid dialect or syntax variant.
- Diagram layout output may evolve across library versions.
- Markdown formatting after Mermaid replacement may interact with spacing around diagrams.

## Risk Mitigation

- Preserve the original fenced block whenever rendering fails.
- Keep test assertions focused on stable structural properties, not exact layout geometry.
- Limit Mermaid rendering to interactive TTY output only so non-interactive consumers see unchanged source text.
