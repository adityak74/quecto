# quecto — Harness Design

> **The smallest harness of all time.**

## Overview

quecto is a Rust library + CLI that sends a prompt to any OpenAI-compatible LLM endpoint and returns the raw text response.

One function. Two files. That's the harness.

## Core Library

### Public API

Two functions:

```rust
pub fn quecto(prompt: &str) -> Result<String, Error>
pub fn quecto_to(prompt: &str, base_url: &str, api_key: &str, model: &str) -> Result<String, Error>
```

- `quecto()` — reads `QUECTO_BASE_URL` and `QUECTO_API_KEY` from environment
- `quecto_to()` — accepts base URL and API key directly (for local models, vLLM, Ollama, etc.)

### Configuration

Two environment variables:

| Variable | Default | Purpose |
|---|---|---|
| `QUECTO_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `QUECTO_API_KEY` | *(required)* | API key for authentication |

### Error handling

Single `Error` enum:

```rust
pub enum Error {
    Http(reqwest::Error),
}
```

No custom error types. No error chain. One variant.

### Dependencies

| Crate | Feature | Purpose |
|---|---|---|
| `reqwest` | `json` | HTTP client |
| `serde` | `derive` | JSON serialization |
| `serde_json` | *(none)* | JSON parsing |

No framework. No CLI library. No tracing/logging. No error chain crate.

## CLI Binary

```bash
quecto "your prompt here"
```

- Reads arguments from `std::env::args()`
- Passes them to the library
- Prints result to stdout
- Prints error to stderr and exits with code 1

No help flag. No config file. No subcommands.

## Crate Structure

```
quecto/
├── Cargo.toml
├── src/
│   ├── lib.rs    # pub fn quecto(), pub fn quecto_to(), Error type
│   └── main.rs   # CLI entry point
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
reqwest POST /chat/completions
    │
    ▼
Parse response.json.choices[0].message.content
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
| Body | `{"model": "gpt-4o", "messages": [{"role": "user", "content": "<prompt>"}]}` |

The model is hardcoded to `gpt-4o` as the default. `quecto_to()` accepts the model as a fourth optional parameter.

## Error Behavior

- Network failure → print error to stderr, exit 1
- Invalid response → print error to stderr, exit 1
- API error (4xx/5xx) → print error to stderr, exit 1

## Non-Goals

- Streaming responses
- Multiple model support (beyond the hardcoded default)
- Tool/function calling
- Image/audio generation
- Context management / conversation history
- Configuration files
- Authentication helpers
- Logging / tracing
