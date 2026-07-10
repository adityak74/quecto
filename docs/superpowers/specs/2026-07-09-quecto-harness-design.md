# quecto — Harness Design

> **The smallest harness of all time.**

## Overview

quecto is a Rust library + CLI that sends a prompt to any OpenAI-compatible LLM endpoint and returns the raw text response.

Two functions. Two files. That's the harness.

## Core Library

### Public API

Two functions:

```rust
pub fn quecto(prompt: &str) -> Result<String, Error>
pub fn quecto_to(prompt: &str, base_url: &str, api_key: &str, model: &str) -> Result<String, Error>
```

- `quecto()` — reads `QUECTO_BASE_URL`, `QUECTO_API_KEY`, and `QUECTO_MODEL` from environment
- `quecto_to()` — accepts base URL, API key, and model directly (for local models, vLLM, Ollama, etc.)

All four parameters of `quecto_to()` are required. `quecto()` supplies the model from
`QUECTO_MODEL` (defaulting to `gpt-4o`) and delegates to `quecto_to()`.

### Configuration

Two environment variables:

| Variable | Default | Purpose |
|---|---|---|
| `QUECTO_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `QUECTO_API_KEY` | *(required)* | API key for authentication |
| `QUECTO_MODEL` | `gpt-4o` | Model name sent in the request body |

### Error handling

Single `Error` enum, two variants:

```rust
pub enum Error {
    Http(reqwest::Error),
    Quecto(String),
}
```

- `Http` — transport/decode failures from `reqwest` (via `From<reqwest::Error>`)
- `Quecto` — everything quecto detects itself: a missing/empty `QUECTO_API_KEY`,
  a non-2xx API response, or a response body with no `choices`

Two variants is the floor for correct behavior: env-var lookup returns `VarError` (not
a `reqwest::Error`), and an empty `choices` array must produce a clean error rather than
panicking on `choices[0]`. No error-chain crate; `Error` implements `std::error::Error`
and `Display` by hand.

### Dependencies

| Crate | Feature | Purpose |
|---|---|---|
| `reqwest` | `json` | HTTP client |
| `serde` | `derive` | JSON serialization |
| `serde_json` | *(none)* | JSON parsing |

No framework. No CLI library. No tracing/logging. No error chain crate.

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

This is the one behavioral difference from one-shot mode:

- A per-turn failure (network blip, bad response) → print to stderr and **keep looping**;
  a single flaky turn must not kill the session
- A missing/empty `QUECTO_API_KEY` → fatal on the first turn (exit 1); looping is pointless
  when no turn can ever succeed

### REPL non-goals

Command history (up-arrow), line editing, multi-line input, slash-commands, and `/clear`
are all out of scope — any of them would pull in a readline dependency (`rustyline` or
similar) and is a separate decision. Ctrl-D exits. That is the whole UI.

## Crate Structure

```
quecto/
├── Cargo.toml
├── src/
│   ├── lib.rs    # pub fn quecto(), pub fn quecto_to(), Error type
│   └── main.rs   # CLI entry point: one-shot + REPL
```

Two source files. That's the entire project.

## Data Flow

```
CLI arg (prompt)
    │
    ▼
quecto(prompt)
    │
    ▼
reqwest POST /chat/completions  (60s timeout, .error_for_status())
    │
    ▼
Parse body → choices.first() → message.content
    (empty choices ⇒ Error::Quecto, never a panic)
    │
    ▼
Return String
```

## HTTP Request

| Field | Value |
|---|---|
| Method | `POST` |
| URL | `$QUECTO_BASE_URL/chat/completions` |
| Header | `Authorization: Bearer $QUECTO_API_KEY` |
| Header | `Content-Type: application/json` |
| Body | `{"model": "<model>", "messages": [{"role": "user", "content": "<prompt>"}]}` |
| Timeout | 60s (set explicitly; reqwest has no default) |

The model comes from the caller: `quecto()` reads `QUECTO_MODEL` (default `gpt-4o`), and
`quecto_to()` takes it as a required fourth parameter. The request status is checked with
`.error_for_status()` so non-2xx responses become an `Error::Quecto` instead of being
decoded as a success body.

## Error Behavior

In **one-shot mode**:

- Network failure → print error to stderr, exit 1
- Invalid response → print error to stderr, exit 1
- API error (4xx/5xx) → print error to stderr, exit 1

In **interactive mode** the same failures print to stderr but the loop continues, except a
missing/empty `QUECTO_API_KEY`, which is fatal on the first turn. See
[Interactive Mode](#error-handling-in-the-repl).

## Non-Goals

- Streaming responses
- Model *behavior* tuning (temperature, max_tokens, top_p, etc. — model name is the only knob)
- Tool/function calling
- Image/audio generation
- Context management / conversation history
- Configuration files
- Authentication helpers
- Logging / tracing
