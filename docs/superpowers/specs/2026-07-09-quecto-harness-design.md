# quecto — Harness Design

> **The smallest harness of all time.**

## Overview

quecto is a Rust library + CLI that sends a prompt to any OpenAI-compatible LLM
endpoint — cloud (OpenAI) or local (Ollama, MLX/LM Studio, vLLM) — and returns the
response, buffered or streamed.

One wire format. Two files. Zero async. That's the harness.

In the terms of a full coding-agent harness, quecto is exactly the **model adapter** and
nothing else — the one component that talks to the model. The agent loop, tools, sandbox,
verification, session state, and rich TUI are all **companion crates built on top**, never
part of the core. See [Composability](#composability) and the companion reference,
`2026-07-09-full-harness-reference.md`.

**Provider scope:** OpenAI-compatible (`POST /chat/completions`) only. That single wire
format already covers OpenAI, Ollama, MLX/LM Studio, and vLLM. Native Anthropic/Gemini APIs
are out of scope — either use their OpenAI-compatible endpoints or wrap them in a separate
adapter crate later.

## Core Library

### Public API

Four functions: two primitives (buffered + streamed) and two conveniences layered over them.

```rust
// primitive (buffered): POST an arbitrary JSON body to an arbitrary URL with arbitrary
// headers; return the full response Value. Opinion-free: no path, no auth scheme, no
// assumption about model, messages, or response shape. The unit everything else builds on.
pub fn quecto_raw(
    url: &str,
    headers: &[(&str, &str)],
    body: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>>

// primitive (streamed): same, with "stream": true; calls on_delta for each content token
// as it arrives, and returns the fully accumulated text.
pub fn quecto_stream(
    url: &str,
    headers: &[(&str, &str)],
    body: serde_json::Value,
    on_delta: impl FnMut(&str),
) -> Result<String, Box<dyn std::error::Error>>

// convenience: build a single-user-message body, extract the text content.
pub fn quecto_to(
    prompt: &str,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Result<String, Box<dyn std::error::Error>>

// ergonomic: read config from the environment, delegate to quecto_to.
pub fn quecto(prompt: &str) -> Result<String, Box<dyn std::error::Error>>
```

- `quecto_raw()` — the opinion-free buffered primitive. You supply the **exact URL**, the
  **exact headers**, and an arbitrary JSON body; it returns the whole response as a `Value`.
  It imposes no endpoint path (hit `/chat/completions`, `/completions`, `/embeddings`, …), no
  auth scheme (`Authorization: Bearer`, `x-api-key`, org/version/proxy headers — your call),
  and no response shape. Because it neither shapes the request nor discards the response, a
  caller can include a `tools` array and read back `tool_calls` — the only hook an agent/MCP
  layer needs. The single opinion left is "the payload is JSON" (inherent to the target APIs).
- `quecto_stream()` — the streaming primitive, same signature shape. See [Streaming](#streaming).
- `quecto_to()` — convenience. **This is where the OpenAI-flavored opinions live**: it
  appends `/chat/completions` to `base_url`, turns `Some(key)` into an
  `Authorization: Bearer` header (`None` → no auth header, for local servers), builds
  `{"model": …, "messages": [{"role": "user", "content": prompt}]}`, calls `quecto_raw()`, and
  extracts `choices[0].message.content`. Deliberately user-message only — the system prompt
  lives in `quecto()`/the body, not in this signature. Primary path for local models (point
  `base_url` at `http://localhost:11434/v1`, pass `None` for the key).
- `quecto()` — the most ergonomic path. Reads `QUECTO_BASE_URL`, `QUECTO_API_KEY`,
  `QUECTO_MODEL`, and optional `QUECTO_SYSTEM`. With no system prompt it delegates to
  `quecto_to()`; with one set it builds a `[system, user]` body and the URL/headers itself and
  calls `quecto_raw()`.

**Opinion boundary:** the two primitives impose essentially nothing (you control URL,
headers, body). Every convenience above them — the `/chat/completions` path, the Bearer
scheme, the message shape, the `choices[0].message.content` extraction, the env defaults — is
optional sugar you bypass by calling `quecto_raw` directly.

### Streaming

`quecto_stream()` covers the "streaming model output" capability a coding-agent CLI expects,
**without reintroducing async**. It:

1. Ensures `"stream": true` in the body and POSTs as usual.
2. Reads the response body as a synchronous `BufRead`, line by line (this is all `ureq`
   needs — no runtime, no futures).
3. For each `data: {…}` line, parses it and passes `choices[0].delta.content` to the
   `on_delta` closure; stops at `data: [DONE]`.
4. Accumulates the deltas and returns the complete text `String`.

Scope note: streaming carries **text content only**. Tool-calling turns should use
`quecto_raw` (the agent needs the *complete* `tool_calls` before executing anything); only
the final user-facing answer benefits from streaming. This keeps `quecto_stream` tiny — it
never has to reassemble partial `tool_calls` deltas.

### System prompt

A system prompt is just a `{role:"system"}` message. Because `quecto_raw`/`quecto_stream`
accept an arbitrary body, **system-prompt support is already in the core** — no dedicated
API is needed. The agent layer always builds its own messages array (system prompt assembled
from `AGENTS.md`/instructions) and calls `quecto_raw`.

For standalone CLI use, `quecto()` and the binary read the optional `QUECTO_SYSTEM` env var:
when set, they prepend `{"role":"system","content": <QUECTO_SYSTEM>}` before the user turn;
when unset, the messages array is just `[user]`. `quecto_to()` is not widened — it stays the
single-user-message convenience.

### Configuration

Four environment variables (read only by `quecto()`; the primitives read none):

| Variable | Default | Purpose |
|---|---|---|
| `QUECTO_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `QUECTO_API_KEY` | *(optional)* | Bearer token; if unset, no auth header is sent |
| `QUECTO_MODEL` | `gpt-4o` | Model name sent in the request body |
| `QUECTO_SYSTEM` | *(optional)* | System prompt; if set, prepended as a `{role:system}` message |

`QUECTO_API_KEY` is optional by design — the local coding models this harness targets
(e.g. `qwen2.5-coder`, `qwen3.6:*-mlx`, `devstral`, `codestral`) run on servers that ignore
auth. The harness must reach them without a key.

`QUECTO_SYSTEM` is a convenience knob for standalone CLI use. A system prompt is just a
`{role:system}` message, which `quecto_raw`/`quecto_stream` already accept via the body — so
this env var adds no capability to the core, only ergonomics to the env-based path (see
[System prompt](#system-prompt)).

### Error handling

No custom error type. Every function returns `Result<_, Box<dyn std::error::Error>>`.

- `ureq` errors (transport failures *and* non-2xx HTTP status) propagate via `?`.
- `serde_json` parse errors propagate via `?`.
- quecto's own logic errors (e.g. a response with no `choices`) are constructed inline:
  `return Err("no choices in response".into());`

This is the tiniest correct option: zero type definitions, zero `From` impls, and every
error path composes with `?`. Consumers get a `Display` string; they can't `match` on
transport-vs-logic — an acceptable trade for "give me the string or a failure."

### Dependencies

| Crate | Feature | Purpose |
|---|---|---|
| `ureq` | `json` | Synchronous HTTP client (no async runtime) |
| `serde_json` | *(none)* | Build request bodies, parse responses |

Two direct dependencies, ~30 transitive crates, **no `tokio`, no `reqwest`, no `serde`
derive**. `ureq` is blocking, so `main` is a plain `fn main()`. `serde_json::Value` appears
in the public API — that is intentional; it is what makes `quecto_raw` composable.

No framework. No CLI library. No tracing/logging. No error-chain crate. No async runtime.

## CLI Binary

The binary has two modes, chosen by whether any arguments are present:

```bash
quecto "your prompt here"    # one-shot mode
quecto                       # interactive (REPL) mode
```

### One-shot mode (arguments present)

- Reads arguments from `std::env::args()`, skips `argv[0]`, and joins the rest with a
  single space — so `quecto write me a haiku` and `quecto "write me a haiku"` behave
  identically
- Reads `QUECTO_BASE_URL` / `QUECTO_API_KEY` / `QUECTO_MODEL` / `QUECTO_SYSTEM`, builds the
  body (prepending a system message when `QUECTO_SYSTEM` is set), and calls `quecto_stream`,
  printing each token to stdout as it arrives (live output). The buffered `quecto()` /
  `quecto_to()` remain the library entry points for callers who want a `String`.
- Prints a trailing newline when the stream ends
- Prints error to stderr and exits with code 1

### Interactive mode (no arguments)

With zero arguments, `quecto` enters a stateless REPL — see [Interactive Mode](#interactive-mode).
This also transparently handles piped input (`echo "hi" | quecto`), since both read lines
from stdin.

### `quecto --init` (optional env bootstrapper)

The one exception to "no subcommands." A `--init` flag runs a tiny interview for the four
env basics (`QUECTO_BASE_URL`, `QUECTO_API_KEY`, `QUECTO_MODEL`, `QUECTO_SYSTEM`) and writes a
`.env` (or prints `export` lines). It is a **flag, not a subcommand**, so it never collides
with a prompt like `quecto init`. Crucially, this only helps you *set* env — the core still
reads **only** env vars at runtime, so the "no config file" non-goal holds (nothing is read
back from a file). The same interview is exposed as a library helper that `quecto-agent`'s
full wizard reuses for its first section.

Otherwise: no help flag, no config file read at runtime, no other subcommands.

## Interactive Mode

A minimal read-eval-print loop. No new dependencies — only `std::io`. The entire "UI" is a
prompt indicator and Ctrl-D to quit.

The loop:

1. Print the prompt indicator `quecto› ` to **stderr** (keeps stdout clean, so
   `quecto > out.txt` captures only responses)
2. Read one line from stdin
   - EOF (Ctrl-D), or a line equal to `exit` or `quit` → break
   - Blank line → skip and re-prompt
3. Stream the reply — build the body from the line + env config (including a `QUECTO_SYSTEM`
   system message if set) and call `quecto_stream`, printing each token to **stdout** as it
   arrives. A fresh, independent call (**stateless**: no history is retained or sent between
   turns, preserving the conversation-history non-goal). Note the system prompt is *not*
   conversational state — it is re-sent identically each turn.
4. Print a trailing newline
5. Loop

### Error handling in the REPL

Any per-turn failure (network blip, bad response, unreachable server) prints to stderr and
the loop **continues** — a single flaky turn must not kill the session. There is no fatal
case: with the API key optional there is nothing to validate up front, so the REPL simply
loops until EOF/`exit`/`quit`.

### REPL non-goals

Command history (up-arrow), line editing, multi-line input, slash-commands, and `/clear`
are all out of scope — any of them would pull in a readline dependency (`rustyline` or
similar) and is a separate decision. Ctrl-D exits. That is the whole UI.

## Crate Structure

The repo root is a Cargo **workspace** whose primary member is the tiny `quecto` core crate.
Companion crates (agent loop, MCP) are added as sibling members later, each with their own
spec and their own (heavier) dependencies — they never alter the core.

```
quecto/                    # repo root = core crate + workspace root
├── Cargo.toml             # [package] quecto  +  [workspace] members = [".", …]
├── src/
│   ├── lib.rs             # quecto_raw(), quecto_stream(), quecto_to(), quecto()
│   └── main.rs            # CLI entry point: streaming one-shot + REPL
│
├── quecto-agent/          # FUTURE companion crate (own spec): install wizard + config
│                          #   file, agent loop, tools, sandbox, verification,
│                          #   session state, rich TUI
└── quecto-mcp/            # FUTURE companion crate (own spec): MCP client (tokio + rmcp)
```

The core is still **two source files**. The workspace layout just reserves the seams so
companions can grow without the core ever gaining `tokio`, tool execution, or state.

## Data Flow

Conveniences add opinions (path, Bearer, message shape) then call the opinion-free primitive.
**Buffered** (library convenience + agent tool-turns):

```
quecto(prompt) ─env─▶ quecto_to ─┐ append /chat/completions
                                 ├ Bearer header from key
                                 └ build messages body
                                          │
                                          ▼
                      quecto_raw(url, headers, body)  ◀── agent/MCP calls this directly
                                          │                with its own url/headers/tools
                              ureq POST <url> (non-2xx ⇒ Err)
                                          │
                                          ▼
                                 Response Value ◀── agent/MCP reads
                                          │          choices[].message.tool_calls
                                          ▼
              String ◀── choices[0].message.content
                         (missing choices ⇒ Err("no choices…"), never a panic)
```

**Streamed** (the CLI's default; final-answer UX):

```
body (+ "stream": true) ─▶ quecto_stream(url, headers, body, on_delta)
                                          │
                              ureq POST <url>
                                          │
                          read response as BufRead, line by line
                                          │
                    each `data: {…}` ─▶ on_delta(choices[0].delta.content)
                          `data: [DONE]` ─▶ stop
                                          ▼
                          String (all deltas concatenated)
```

## HTTP Request

The **primitive** sends exactly what it is given: `POST <url>` with the caller's `headers`
and JSON `body`. It adds only `Content-Type: application/json` (implied by sending JSON) and
the timeouts below. Nothing else.

The **convenience layer** (`quecto_to`/`quecto`) constructs the OpenAI-flavored request:

| Field | Value constructed by the convenience layer |
|---|---|
| Method | `POST` |
| URL | `<base_url>/chat/completions` |
| Header | `Authorization: Bearer <api_key>` *(only when `api_key` is `Some`)* |
| Header | `Content-Type: application/json` |
| Body | `{"model": "<model>", "messages": [ …optional system…, {"role": "user", "content": "<prompt>"}]}` |
| Timeout | connect + per-read timeout (e.g. 60s each), **not** an overall deadline |

The timeout is applied as connect/read timeouts rather than a single whole-response deadline
— otherwise a stream that legitimately runs longer than the deadline would be severed
mid-response. `ureq` returns `Err` on non-2xx status by default, so no explicit status check
is needed — HTTP errors surface through `?` like any other failure.

## Error Behavior

**One-shot mode:** any failure (network, invalid response, HTTP 4xx/5xx) → print to stderr,
exit 1.

**Interactive mode:** the same failures print to stderr but the loop continues — there is no
fatal case. See [Error handling in the REPL](#error-handling-in-the-repl).

## Composability

quecto's core is a text/JSON primitive. Anything stateful or agentic lives **outside** it,
depending on `quecto_raw`. The core never gains an async runtime, tool execution, or
conversation state.

### Mapping to a full coding-agent harness

A complete CLI coding agent has ~13 minimum components (see
`2026-07-09-full-harness-reference.md`). The compression verdict: **exactly one of them is
quecto.**

| Full-harness component | Home |
|---|---|
| Model adapter (talk to the model) | **quecto core** — `quecto_raw` / `quecto_stream` |
| Install wizard + config file (endpoint→model→system→tools→approvals) | `quecto-agent` (core contributes only `--init` for env basics) |
| CLI (rich: run/chat/resume/diff/undo, approvals, max-steps) | `quecto-agent` (quecto keeps only its tiny one-shot + REPL) |
| Instruction loader (AGENTS.md/CLAUDE.md precedence) | `quecto-agent` |
| Repository context engine (discovery, gitignore, ripgrep, budget) | `quecto-agent` |
| Tool registry + essential tools (read/search/patch/run/git/ask) | `quecto-agent` (core only *transports* `tools` in the body) |
| Agent loop (reason→tool→observe, limits, cancel) | `quecto-agent` |
| File-editing engine (patch, validate, rollback) | `quecto-agent` |
| Command sandbox (timeouts, approvals, redaction) | `quecto-agent` |
| Verification loop (format/lint/typecheck/test, gate) | `quecto-agent` |
| Session state (SQLite) | `quecto-agent` |
| Terminal renderer (● activity, slash commands) | `quecto-agent` |
| MCP integrations | `quecto-mcp` |

The reference's normalized model response — `text`, `tool_calls`, `usage`, `stop_reason` —
needs **no** typed struct in the core: all four are already present in the `Value` from
`quecto_raw` (`choices[0].message.content`, `.tool_calls`, `choices[0].finish_reason`,
`usage`). The agent loop reads them directly. So `quecto-agent` can be built with zero
further additions to quecto.

### Unopinionated by construction

quecto ships with **no baked-in behavior** — the user supplies every opinion. The primitives
(`quecto_raw`/`quecto_stream`) decide nothing: you pass the exact URL, headers, and body. The
conveniences add opinions but are entirely optional (bypass them by calling the primitive).
The way an unopinionated tool becomes usable is that it **asks** rather than assumes — which
is the job of the install wizard below.

### Install wizard & configuration

The full first-run wizard lives in **`quecto-agent`** (the installable product), because it
configures things that only exist there — tool enable/disable, approval policy, verification
commands — alongside the basics. It walks the user from endpoint → key → model → system
prompt → tools → approvals and persists answers to its own config file
(`~/.config/quecto/config.toml`). The tiny core contributes only its `--init` env
bootstrapper (reused as the wizard's first section).

Configuration precedence (in `quecto-agent`) is the standard, scriptable order:

```
CLI flag  >  env var  >  config.toml  >  built-in default
```

This keeps everything overridable and CI-friendly: the wizard writes the file, but env vars
and explicit flags always win, so power users and scripts can ignore the wizard entirely.
The **core** crate has no config file and no flags beyond `--init`; its runtime precedence is
simply env var > default.

### Tools (write / edit / create / read …)

The core ships **zero** tools and performs **zero** filesystem access. A tool is: a schema
in the request `tools` array, a `tool_calls` reply from the model, execution of the call
(with real side effects), and a follow-up turn feeding the result back — i.e. an agent loop
with state. That belongs one layer up. `quecto_raw` already exposes everything such a layer
needs (arbitrary body in, full response with `tool_calls` out).

Two ways a user adds tools on top, both richer than any hardcoded builtins:

- **Via MCP** — a `quecto-mcp` companion points at existing MCP servers; the official
  `server-filesystem` provides `read`/`write`/`edit`/`list` for free.
- **Hand-rolled native tools** — match on `tool_calls`, call `std::fs`, feed results back
  (~20 lines each, no dependency).

### MCP

MCP support is a **future companion crate** (`quecto-mcp`), not part of this spec or the core
crate. It would carry its own heavy dependencies (`tokio`, an MCP SDK such as `rmcp`,
JSON-RPC transports) and implement the agentic tool loop, building on `quecto_raw`. None of
that touches the tiny core. Whether that companion ships a batteries-included filesystem
tool set is a decision for *its* spec, later.

## Non-Goals

For the **core crate** (most of these belong to `quecto-agent` / `quecto-mcp`, never the core):

- Typed convenience params for model *behavior* tuning (temperature, max_tokens, top_p, …).
  The convenience functions (`quecto`/`quecto_to`) expose only the model name; anyone needing
  more sets those fields directly in the `Value` body passed to `quecto_raw`/`quecto_stream`.
- Tool/function calling *execution* (the core forwards `tools` and returns `tool_calls`, but
  never executes them)
- Agent loop, step/token limits, repeated-action detection
- File-editing / patch engine, command sandbox, approval policy
- Verification loop (format/lint/typecheck/test)
- Repository context engine, instruction loader, ripgrep search
- Session state / persistence (SQLite), checkpoints, undo
- Rich TUI / activity renderer / slash-commands
- MCP client (see [Composability](#composability))
- Native Anthropic/Gemini providers (OpenAI-compatible wire format only)
- Image/audio generation
- Context management / conversation history
- Config-file *reading* at runtime (env only; `--init` may *write* a `.env`, but nothing is
  read back from a config file — the persisted `config.toml` + wizard belong to `quecto-agent`)
- Auth *flows* (OAuth, token refresh, credential storage). The primitive sends whatever
  headers you give it — any scheme works — but the core performs no auth logic of its own
- Logging / tracing
- Async runtime

Note: **streaming is now in-core** via `quecto_stream` (synchronous SSE) — it is no longer a
non-goal. Everything else the reference lists as "minimum" is a companion concern.
