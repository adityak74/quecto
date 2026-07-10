# quecto — Harness Design

> **The smallest harness of all time.**

## Overview

quecto is a Rust library + CLI that sends a prompt to any OpenAI-compatible LLM
endpoint — cloud (OpenAI) or local (Ollama, MLX/LM Studio, vLLM) — and returns the raw
text response.

One endpoint. Two files. Zero async. That's the harness.

Everything larger — tools, MCP, agent loops — is a **companion crate built on top**, never
part of the core (see [Composability](#composability)).

## Core Library

### Public API

Three functions, layered. Each is a thin wrapper over the one below it:

```rust
// primitive: one POST, full response Value in and out.
// The composable unit that tool/agent/MCP layers build on.
pub fn quecto_raw(
    body: serde_json::Value,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>>

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

- `quecto()` — reads `QUECTO_BASE_URL`, `QUECTO_API_KEY`, and `QUECTO_MODEL` from the
  environment, then calls `quecto_to()`.
- `quecto_to()` — builds `{"model": …, "messages": [{"role": "user", "content": prompt}]}`,
  calls `quecto_raw()`, and extracts `choices[0].message.content`. This is the primary path
  for local models (point `base_url` at `http://localhost:11434/v1`, pass `None` for the key).
- `quecto_raw()` — the primitive. Sends whatever JSON body you give it and returns the whole
  response as a `Value`. Because it neither shapes the request nor discards the response, a
  caller can include a `tools` array and read back `tool_calls` — this is the only hook an
  agent/MCP layer needs.

`api_key` is `Option<&str>`: `Some(key)` sends an `Authorization: Bearer` header; `None`
omits it entirely (required for no-auth local servers like Ollama).

### Configuration

Three environment variables (read only by `quecto()`):

| Variable | Default | Purpose |
|---|---|---|
| `QUECTO_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `QUECTO_API_KEY` | *(optional)* | Bearer token; if unset, no auth header is sent |
| `QUECTO_MODEL` | `gpt-4o` | Model name sent in the request body |

`QUECTO_API_KEY` is optional by design — the local coding models this harness targets
(e.g. `qwen2.5-coder`, `qwen3.6:*-mlx`, `devstral`, `codestral`) run on servers that ignore
auth. The harness must reach them without a key.

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
- Passes the joined prompt to `quecto()`
- Prints result to stdout
- Prints error to stderr and exits with code 1

### Interactive mode (no arguments)

With zero arguments, `quecto` enters a stateless REPL — see [Interactive Mode](#interactive-mode).
This also transparently handles piped input (`echo "hi" | quecto`), since both read lines
from stdin.

No help flag. No config file. No subcommands.

## Interactive Mode

A minimal read-eval-print loop. No new dependencies — only `std::io`. The entire "UI" is a
prompt indicator and Ctrl-D to quit.

The loop:

1. Print the prompt indicator `quecto› ` to **stderr** (keeps stdout clean, so
   `quecto > out.txt` captures only responses)
2. Read one line from stdin
   - EOF (Ctrl-D), or a line equal to `exit` or `quit` → break
   - Blank line → skip and re-prompt
3. Call `quecto(line)` — a fresh, independent call (**stateless**: no history is retained
   or sent between turns, preserving the conversation-history non-goal)
4. Print the response to **stdout**
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

```
quecto/
├── Cargo.toml
├── src/
│   ├── lib.rs    # quecto_raw(), quecto_to(), quecto()
│   └── main.rs   # CLI entry point: one-shot + REPL
```

Two source files. That's the entire project.

## Data Flow

```
CLI arg (prompt)                body: Value (+ optional tools)
    │                                   │
    ▼                                   │
quecto(prompt)  ── env ──▶ quecto_to ──▶│
    │                                   ▼
    │                       quecto_raw(body, base_url, key)
    │                                   │
    │                   ureq POST /chat/completions
    │                   (60s timeout; non-2xx ⇒ Err)
    │                                   │
    │                                   ▼
    │                          Response Value  ◀── agent/MCP layer reads
    │                                   │           choices[].message.tool_calls here
    ▼                                   ▼
Return String  ◀── choices[0].message.content
                   (missing choices ⇒ Err("no choices…"), never a panic)
```

## HTTP Request

| Field | Value |
|---|---|
| Method | `POST` |
| URL | `<base_url>/chat/completions` |
| Header | `Authorization: Bearer <api_key>` *(only when `api_key` is `Some`)* |
| Header | `Content-Type: application/json` |
| Body | The `Value` passed to `quecto_raw`. `quecto_to` builds `{"model": "<model>", "messages": [{"role": "user", "content": "<prompt>"}]}` |
| Timeout | 60s (set explicitly on the `ureq` agent) |

`ureq` returns `Err` on non-2xx status by default, so no explicit status check is needed —
HTTP errors surface through `?` like any other failure.

## Error Behavior

**One-shot mode:** any failure (network, invalid response, HTTP 4xx/5xx) → print to stderr,
exit 1.

**Interactive mode:** the same failures print to stderr but the loop continues — there is no
fatal case. See [Error handling in the REPL](#error-handling-in-the-repl).

## Composability

quecto's core is a text/JSON primitive. Anything stateful or agentic lives **outside** it,
depending on `quecto_raw`. The core never gains an async runtime, tool execution, or
conversation state.

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

For the **core crate** (candidates for a companion crate, never the core):

- Streaming responses
- Model *behavior* tuning (temperature, max_tokens, top_p, etc. — model name is the only knob)
- Tool/function calling *execution* (the core forwards `tools` and returns `tool_calls`, but
  never executes them)
- MCP client (see [Composability](#composability))
- Image/audio generation
- Context management / conversation history
- Configuration files
- Authentication helpers beyond an optional bearer token
- Logging / tracing
- Async runtime
