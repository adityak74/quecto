# quecto Core Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the tiny `quecto` core crate — a Rust library (4 public functions + small reusable helpers) and a CLI binary (one-shot, streaming, REPL, `--init`) that sends a prompt to any OpenAI-compatible `/chat/completions` endpoint and returns the response, buffered or streamed, with zero async.

**Architecture:** Two source files. `src/lib.rs` holds the four primitives/conveniences (`quecto_raw`, `quecto_stream`, `quecto_to`, `quecto`) plus small `pub` helpers (`build_body`, `join_url`, `env_config`, `extract_content`, `init_exports`) that the binary and future companion crates reuse so request-assembly logic lives in exactly one place. `src/main.rs` is a thin CLI that reads env + args and calls the library. HTTP is synchronous via `ureq` (rustls, no runtime); JSON via `serde_json`. Pure helpers are unit-tested in `lib.rs`; HTTP and CLI behavior are integration-tested against a dependency-free hand-rolled TCP mock server.

**Tech Stack:** Rust (edition 2021), `ureq` 2.x (`json` feature), `serde_json` 1. No `tokio`, no `reqwest`, no `serde` derive, no CLI framework, no logging.

## Global Constraints

- Rust edition = **2021** (keeps `std::env::set_var` safe for tests; avoids the edition-2024 unsafe-set_var churn).
- Dependencies are exactly two: `ureq = { version = "2", features = ["json"] }` and `serde_json = "1"`. **Do not** set `default-features = false` on `ureq` (that would drop the rustls TLS backend and break every HTTPS call).
- Error type everywhere: `Box<dyn std::error::Error + Send + Sync>` (aliased `BoxErr`). No custom error enum, no `From` impls — every path composes with `?`.
- Exactly two source files under `src/`: `lib.rs` and `main.rs`. No third module.
- Public API primary surface is the four functions `quecto_raw` / `quecto_stream` / `quecto_to` / `quecto`; the additional `pub` helpers exist only to let the binary and companion crates reuse assembly logic (DRY) — keep them minimal.
- No async runtime anywhere. `main` is a plain `fn main()`. `ureq` is blocking.
- Provider scope: OpenAI-compatible `POST /chat/completions` only.
- `serde_json::Value` is intentionally part of the public API (it is what makes `quecto_raw` composable) — do not hide it behind a typed struct.

---

## File Structure

- `Cargo.toml` — root package `quecto` + workspace declaration (reserves future companion members). Created once in Task 1.
- `.gitignore` — ignore `/target`. Created once in Task 1.
- `src/lib.rs` — grows across Tasks 1–5 and 7:
  - Task 1: `BoxErr` alias, `build_body`, `join_url`
  - Task 2: `extract_content`, `parse_sse_delta`
  - Task 3: `agent()`, `quecto_raw`
  - Task 4: `env_config`, `quecto_to`, `quecto`
  - Task 5: `quecto_stream`, `handle_frame`
  - Task 7: `init_exports`
- `src/main.rs` — created in Task 6 (dispatch, `answer`, one-shot, REPL); extended in Task 7 (`--init`).
- `tests/common/mod.rs` — the dependency-free mock HTTP server helper (created in Task 3, reused by Tasks 4–7).
- `tests/http.rs` — integration tests for `quecto_raw`, `quecto_to`, `quecto` (Tasks 3–4).
- `tests/stream.rs` — integration tests for `quecto_stream` (Task 5).
- `tests/cli.rs` — subprocess integration tests for the binary via `CARGO_BIN_EXE_quecto` (Tasks 6–7).

Pure-function unit tests live in a `#[cfg(test)] mod tests` at the bottom of `src/lib.rs`.

---

### Task 1: Scaffold + pure body/url helpers

**Files:**
- Create: `Cargo.toml`
- Create: `.gitignore`
- Create: `src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;`
  - `pub fn build_body(system: Option<&str>, prompt: &str, model: &str) -> serde_json::Value`
  - `pub fn join_url(base: &str, path: &str) -> String`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "quecto"
version = "0.1.0"
edition = "2021"
description = "The smallest harness of all time."
license = "MIT OR Unlicense"

[dependencies]
ureq = { version = "2", features = ["json"] }   # do NOT set default-features = false
serde_json = "1"

[workspace]
members = ["."]
# Future companion members (own specs): "quecto-agent", "quecto-mcp"
```

- [ ] **Step 2: Create `.gitignore`**

```gitignore
/target
Cargo.lock
```

- [ ] **Step 3: Write the failing tests** (create `src/lib.rs` with the test module and the doc header, but no `build_body`/`join_url` yet)

```rust
//! quecto — the smallest harness of all time.
//! Core: quecto_raw / quecto_stream / quecto_to / quecto, plus small pub helpers
//! (build_body, join_url, env_config, extract_content, init_exports) reused by the
//! binary and future companion crates.

use serde_json::{json, Value};

/// Shared boxed error: every fallible fn returns this. Both ureq::Error and
/// serde_json::Error satisfy it, so `?` composes and errors cross into async tasks.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_user_only() {
        let b = build_body(None, "hi", "m");
        assert_eq!(b["model"], "m");
        assert_eq!(b["messages"].as_array().unwrap().len(), 1);
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"], "hi");
    }

    #[test]
    fn build_body_with_system() {
        let b = build_body(Some("sys"), "hi", "m");
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "sys");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn join_url_variants() {
        assert_eq!(join_url("http://x/v1", "chat/completions"), "http://x/v1/chat/completions");
        assert_eq!(join_url("http://x/v1/", "chat/completions"), "http://x/v1/chat/completions");
        assert_eq!(join_url("http://x/v1", "/chat/completions"), "http://x/v1/chat/completions");
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --lib`
Expected: FAIL — `cannot find function build_body` / `join_url` in this scope.

- [ ] **Step 5: Implement `build_body` and `join_url`** (add above the `#[cfg(test)]` module in `src/lib.rs`)

```rust
/// Build an OpenAI-style chat body: optional system message + one user message.
pub fn build_body(system: Option<&str>, prompt: &str, model: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(s) = system {
        messages.push(json!({"role": "system", "content": s}));
    }
    messages.push(json!({"role": "user", "content": prompt}));
    json!({"model": model, "messages": messages})
}

/// Join a base URL and a path with exactly one slash, tolerating trailing/leading
/// slashes on either side (so `…/v1` and `…/v1/` both work).
pub fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib`
Expected: PASS (3 tests). `cargo build` also succeeds.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml .gitignore src/lib.rs
git commit -m "feat: scaffold quecto core with build_body and join_url helpers"
```

---

### Task 2: Response-content + SSE-delta parsing helpers

**Files:**
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `BoxErr` (Task 1).
- Produces:
  - `pub fn extract_content(resp: &serde_json::Value) -> Result<String, BoxErr>` — `choices[0].message.content` as text; `Err("no choices in response")` when `choices` is missing/empty; `""` when content is absent/null.
  - `pub(crate) fn parse_sse_delta(data: &str) -> Option<serde_json::Value>` — `choices[0].delta` object, or `None` for `[DONE]`/bad JSON/no-delta.

- [ ] **Step 1: Write the failing tests** (add these to `mod tests` in `src/lib.rs`)

```rust
    #[test]
    fn extract_content_ok() {
        let r = json!({"choices":[{"message":{"content":"hello"}}]});
        assert_eq!(extract_content(&r).unwrap(), "hello");
    }

    #[test]
    fn extract_content_null_is_empty() {
        let r = json!({"choices":[{"message":{"tool_calls":[]}}]});
        assert_eq!(extract_content(&r).unwrap(), "");
    }

    #[test]
    fn extract_content_no_choices_errs() {
        let r = json!({"error":"x"});
        assert!(extract_content(&r).is_err());
    }

    #[test]
    fn parse_sse_delta_content() {
        let d = parse_sse_delta(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        assert_eq!(d["content"], "hi");
    }

    #[test]
    fn parse_sse_delta_done_none() {
        assert!(parse_sse_delta("[DONE]").is_none());
    }

    #[test]
    fn parse_sse_delta_bad_json_none() {
        assert!(parse_sse_delta("not json").is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib`
Expected: FAIL — `cannot find function extract_content` / `parse_sse_delta`.

- [ ] **Step 3: Implement both helpers** (add to `src/lib.rs`, above the test module)

```rust
/// Extract assistant text from a buffered chat response. Errors only when there
/// are no choices; a present-but-null/absent content yields "" (tool-call turns).
pub fn extract_content(resp: &Value) -> Result<String, BoxErr> {
    let choices = resp
        .get("choices")
        .and_then(|c| c.as_array())
        .filter(|a| !a.is_empty())
        .ok_or("no choices in response")?;
    Ok(choices[0]["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Parse one SSE `data:` payload into its `choices[0].delta` object.
/// Returns None for `[DONE]`, unparseable JSON, or a chunk without a delta.
pub(crate) fn parse_sse_delta(data: &str) -> Option<Value> {
    if data == "[DONE]" {
        return None;
    }
    let chunk: Value = serde_json::from_str(data).ok()?;
    chunk.get("choices")?.get(0)?.get("delta").cloned()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib`
Expected: PASS (9 tests total).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "feat: add extract_content and parse_sse_delta helpers"
```

---

### Task 3: `quecto_raw` buffered primitive + mock server

**Files:**
- Modify: `src/lib.rs`
- Create: `tests/common/mod.rs`
- Create: `tests/http.rs`

**Interfaces:**
- Consumes: `BoxErr` (Task 1).
- Produces:
  - `pub fn quecto_raw(url: &str, headers: &[(&str, &str)], body: serde_json::Value) -> Result<serde_json::Value, BoxErr>` — POST arbitrary JSON to arbitrary URL with arbitrary headers; returns the full parsed response; non-2xx → `Err`.
  - `fn agent() -> ureq::Agent` — private; connect+read timeouts (60s each), **not** an overall deadline.
  - Test helper `common::mock(status: u16, content_type: &str, body: &str) -> String` — one-shot TCP HTTP server, returns base URL `http://127.0.0.1:PORT`.

- [ ] **Step 1: Create the mock server helper** (`tests/common/mod.rs`)

```rust
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// One-shot mock HTTP server. Serves one connection with `status` + `body`
/// (using `content_type`), then the thread exits. Returns "http://127.0.0.1:PORT".
/// Suitable for the small request bodies these tests send.
pub fn mock(status: u16, content_type: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let reason = if status == 200 { "OK" } else { "ERROR" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Drain the request so the client's write side doesn't error.
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}
```

- [ ] **Step 2: Write the failing integration tests** (`tests/http.rs`)

```rust
mod common;
use common::mock;
use serde_json::json;

#[test]
fn raw_returns_full_value() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"hi"}}]}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let resp = quecto::quecto_raw(&url, &[], json!({"model":"m","messages":[]})).unwrap();
    assert_eq!(resp["choices"][0]["message"]["content"], "hi");
}

#[test]
fn raw_non_2xx_is_err() {
    let base = mock(400, "application/json", r#"{"error":"bad"}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let r = quecto::quecto_raw(&url, &[], json!({}));
    assert!(r.is_err());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test http`
Expected: FAIL — `cannot find function quecto_raw in crate quecto`.

- [ ] **Step 4: Implement `agent()` and `quecto_raw`** (add to `src/lib.rs`; add `use std::time::Duration;` at the top with the other `use`s)

```rust
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(60))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// Buffered primitive: POST an arbitrary JSON body to an arbitrary URL with
/// arbitrary headers; return the full parsed response. No path/auth/shape opinions.
/// `ureq` returns `Err` on non-2xx status, so no explicit status check is needed.
pub fn quecto_raw(url: &str, headers: &[(&str, &str)], body: Value) -> Result<Value, BoxErr> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body)?;
    let value: Value = resp.into_json()?;
    Ok(value)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test http && cargo test --lib`
Expected: PASS (2 integration + 9 unit).

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs tests/common/mod.rs tests/http.rs
git commit -m "feat: add quecto_raw buffered primitive with mock-server tests"
```

---

### Task 4: `quecto_to`, `quecto`, and `env_config`

**Files:**
- Modify: `src/lib.rs`
- Modify: `tests/http.rs`

**Interfaces:**
- Consumes: `build_body`, `join_url` (Task 1); `extract_content` (Task 2); `quecto_raw` (Task 3).
- Produces:
  - `pub fn env_config() -> (String, Option<String>, String, Option<String>)` — `(base_url, api_key, model, system)` with defaults `https://api.openai.com/v1` and `gpt-4o`.
  - `pub fn quecto_to(prompt: &str, base_url: &str, api_key: Option<&str>, model: &str) -> Result<String, BoxErr>`
  - `pub fn quecto(prompt: &str) -> Result<String, BoxErr>`

- [ ] **Step 1: Write the failing integration tests** (append to `tests/http.rs`)

```rust
use std::sync::Mutex;

// Serializes the env-mutating test(s); other tests take explicit args and need no lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn to_extracts_content() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"pong"}}]}"#);
    let out = quecto::quecto_to("ping", &base, None, "m").unwrap();
    assert_eq!(out, "pong");
}

#[test]
fn quecto_reads_env() {
    let _g = ENV_LOCK.lock().unwrap();
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"envd"}}]}"#);
    std::env::set_var("QUECTO_BASE_URL", &base);
    std::env::set_var("QUECTO_MODEL", "m");
    std::env::remove_var("QUECTO_API_KEY");
    std::env::remove_var("QUECTO_SYSTEM");
    let out = quecto::quecto("hi").unwrap();
    assert_eq!(out, "envd");
    std::env::remove_var("QUECTO_BASE_URL");
    std::env::remove_var("QUECTO_MODEL");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test http`
Expected: FAIL — `cannot find function quecto_to` / `quecto`.

- [ ] **Step 3: Implement `env_config`, `quecto_to`, `quecto`** (add to `src/lib.rs`)

```rust
/// Read the four env knobs, applying defaults for base_url and model.
pub fn env_config() -> (String, Option<String>, String, Option<String>) {
    let base = std::env::var("QUECTO_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let key = std::env::var("QUECTO_API_KEY").ok();
    let model = std::env::var("QUECTO_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
    let system = std::env::var("QUECTO_SYSTEM").ok();
    (base, key, model, system)
}

/// Convenience: build a single-user-message body, POST to <base_url>/chat/completions
/// with optional Bearer auth, return the assistant text ("" on a tool-only turn).
pub fn quecto_to(prompt: &str, base_url: &str, api_key: Option<&str>, model: &str) -> Result<String, BoxErr> {
    let url = join_url(base_url, "chat/completions");
    let body = build_body(None, prompt, model);
    let auth = api_key.map(|k| format!("Bearer {k}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = &auth {
        headers.push(("Authorization", a.as_str()));
    }
    let resp = quecto_raw(&url, &headers, body)?;
    extract_content(&resp)
}

/// Ergonomic: read env config (incl. optional QUECTO_SYSTEM), send, return text.
pub fn quecto(prompt: &str) -> Result<String, BoxErr> {
    let (base, key, model, system) = env_config();
    let url = join_url(&base, "chat/completions");
    let body = build_body(system.as_deref(), prompt, &model);
    let auth = key.map(|k| format!("Bearer {k}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = &auth {
        headers.push(("Authorization", a.as_str()));
    }
    let resp = quecto_raw(&url, &headers, body)?;
    extract_content(&resp)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test http`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs tests/http.rs
git commit -m "feat: add quecto_to, quecto, and env_config convenience layer"
```

---

### Task 5: `quecto_stream` streaming primitive with non-SSE fallback

**Files:**
- Modify: `src/lib.rs`
- Create: `tests/stream.rs`

**Interfaces:**
- Consumes: `agent()`, `join_url`, `extract_content`, `parse_sse_delta` (Tasks 1–4).
- Produces:
  - `pub fn quecto_stream(url: &str, headers: &[(&str, &str)], body: serde_json::Value, on_delta: impl FnMut(&serde_json::Value)) -> Result<String, BoxErr>` — forces `stream:true`, delivers each SSE chunk's `choices[0].delta` to `on_delta`, accumulates `delta.content`. On a non-SSE body (no `data:` frames), falls back to buffered: parses the whole body and issues one synthetic `{"content": …}` delta.
  - `fn handle_frame(payload: &str, acc: &mut String, on_delta: &mut impl FnMut(&serde_json::Value))` — private.

- [ ] **Step 1: Write the failing integration tests** (`tests/stream.rs`)

```rust
mod common;
use common::mock;
use serde_json::json;

#[test]
fn stream_accumulates_sse() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n";
    let base = mock(200, "text/event-stream", sse);
    let url = quecto::join_url(&base, "chat/completions");
    let mut seen = 0;
    let out = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |_d| seen += 1).unwrap();
    assert_eq!(out, "Hello");
    assert_eq!(seen, 2);
}

#[test]
fn stream_non_sse_fallback() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"whole"}}]}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let mut calls = 0;
    let out = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |d| {
        calls += 1;
        assert_eq!(d["content"], "whole");
    }).unwrap();
    assert_eq!(out, "whole");
    assert_eq!(calls, 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test stream`
Expected: FAIL — `cannot find function quecto_stream`.

- [ ] **Step 3: Implement `quecto_stream` and `handle_frame`** (add to `src/lib.rs`; `use std::io::BufRead;` is used locally inside the function)

```rust
/// Streaming primitive: force stream:true, POST, and deliver each SSE chunk's
/// `choices[0].delta` to `on_delta`; accumulate delta.content into the return String.
/// If the server ignores streaming (no `data:` frames), fall back to buffered: parse
/// the whole body and deliver one synthetic {"content": …} delta — never silent-empty.
pub fn quecto_stream(
    url: &str,
    headers: &[(&str, &str)],
    mut body: Value,
    mut on_delta: impl FnMut(&Value),
) -> Result<String, BoxErr> {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".to_string(), Value::Bool(true));
    }
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body)?;

    use std::io::BufRead;
    let reader = std::io::BufReader::new(resp.into_reader());
    let mut lines = reader.lines();
    let mut acc = String::new();

    // Find the first non-empty line to decide SSE vs buffered.
    let mut first = None;
    for line in lines.by_ref() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        first = Some(line);
        break;
    }
    let first = match first {
        Some(f) => f,
        None => return Ok(acc), // empty body
    };

    if let Some(payload) = first.strip_prefix("data:") {
        // SSE path: process the first frame, then the rest.
        handle_frame(payload.trim(), &mut acc, &mut on_delta);
        for line in lines {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(payload) = line.strip_prefix("data:") {
                let payload = payload.trim();
                if payload == "[DONE]" {
                    break;
                }
                handle_frame(payload, &mut acc, &mut on_delta);
            }
        }
    } else {
        // Non-SSE fallback: reassemble the whole body and parse as buffered.
        let mut whole = first;
        for line in lines {
            whole.push_str(&line?);
        }
        let resp: Value = serde_json::from_str(&whole)?;
        let content = extract_content(&resp)?;
        on_delta(&json!({"content": content}));
        acc.push_str(&content);
    }
    Ok(acc)
}

fn handle_frame(payload: &str, acc: &mut String, on_delta: &mut impl FnMut(&Value)) {
    if let Some(delta) = parse_sse_delta(payload) {
        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            acc.push_str(t);
        }
        on_delta(&delta);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test stream`
Expected: PASS (2 tests).

- [ ] **Step 5: Run the whole suite**

Run: `cargo test`
Expected: PASS (all lib + http + stream tests).

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs tests/stream.rs
git commit -m "feat: add quecto_stream with SSE parsing and non-SSE fallback"
```

---

### Task 6: CLI binary — dispatch, one-shot, and REPL

**Files:**
- Create: `src/main.rs`
- Create: `tests/cli.rs`

**Interfaces:**
- Consumes: `env_config`, `join_url`, `build_body`, `quecto_raw`, `quecto_stream`, `extract_content`, `BoxErr` (Tasks 1–5).
- Produces: the `quecto` binary. `quecto <prompt words>` → one-shot; `quecto` (no args) → REPL. Streaming vs buffered chosen by `QUECTO_STREAM` (default on; `0` = buffered). Errors → stderr; one-shot exits 1, REPL continues.

- [ ] **Step 1: Write the failing subprocess tests** (`tests/cli.rs`)

```rust
mod common;
use common::mock;
use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_quecto")
}

#[test]
fn oneshot_buffered_joins_args() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"hi there"}}]}"#);
    let out = Command::new(bin())
        .arg("say").arg("hi")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "0")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi there\n");
}

#[test]
fn oneshot_streaming_prints_deltas() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"str\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"eam\"}}]}\n\ndata: [DONE]\n\n";
    let base = mock(200, "text/event-stream", sse);
    let out = Command::new(bin())
        .arg("go")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "1")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "stream\n");
}

#[test]
fn repl_answers_one_line_then_eof() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"reply"}}]}"#);
    let mut child = Command::new(bin())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "0")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("reply"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test cli`
Expected: FAIL — link/compile error: no binary target `quecto` / `CARGO_BIN_EXE_quecto` unset (because `src/main.rs` does not exist yet).

- [ ] **Step 3: Create `src/main.rs`**

```rust
use quecto::BoxErr;
use std::io::{self, BufRead, Write};

fn stream_enabled() -> bool {
    std::env::var("QUECTO_STREAM").map(|v| v != "0").unwrap_or(true)
}

/// Answer one prompt with the given config, writing the model text to stdout.
fn answer(
    prompt: &str,
    base: &str,
    key: Option<&str>,
    model: &str,
    system: Option<&str>,
    stream: bool,
) -> Result<(), BoxErr> {
    let url = quecto::join_url(base, "chat/completions");
    let body = quecto::build_body(system, prompt, model);
    let auth = key.map(|k| format!("Bearer {k}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = &auth {
        headers.push(("Authorization", a.as_str()));
    }
    if stream {
        quecto::quecto_stream(&url, &headers, body, |delta| {
            if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
                print!("{t}");
                let _ = io::stdout().flush();
            }
        })?;
    } else {
        let resp = quecto::quecto_raw(&url, &headers, body)?;
        print!("{}", quecto::extract_content(&resp)?);
    }
    Ok(())
}

fn run_oneshot(prompt: &str) {
    let (base, key, model, system) = quecto::env_config();
    if let Err(e) = answer(prompt, &base, key.as_deref(), &model, system.as_deref(), stream_enabled()) {
        eprintln!("quecto: {e}");
        std::process::exit(1);
    }
    println!();
}

/// Stateless REPL: re-read env (incl. system prompt) each turn; no history retained.
fn run_repl() {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    loop {
        eprint!("quecto\u{203a} "); // "quecto› "
        let _ = io::stderr().flush();
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => break, // EOF / Ctrl-D
            Ok(_) => {}
            Err(_) => break,
        }
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" || prompt == "quit" {
            break;
        }
        let (base, key, model, system) = quecto::env_config();
        if let Err(e) = answer(prompt, &base, key.as_deref(), &model, system.as_deref(), stream_enabled()) {
            eprintln!("quecto: {e}"); // per-turn failure never kills the loop
        }
        println!();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        run_repl();
    } else {
        run_oneshot(&args.join(" "));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test cli`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "feat: add CLI binary with one-shot, streaming, and REPL modes"
```

---

### Task 7: `--init` env bootstrapper

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `tests/cli.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub fn init_exports(input: &mut impl std::io::BufRead, prompts: &mut impl std::io::Write) -> std::io::Result<Vec<(String, String)>>` — prompts on `prompts` (stderr), reads answers from `input`, returns the `(var, value)` pairs the user actually set (blanks skipped). Reused by the agent's wizard.
  - `quecto --init` prints `export QUECTO_*="value"` lines to stdout.

- [ ] **Step 1: Write the failing lib unit test** (add to `mod tests` in `src/lib.rs`)

```rust
    #[test]
    fn init_exports_skips_blanks() {
        use std::io::Cursor;
        // base set, key blank, model set, system blank
        let mut input = Cursor::new("http://localhost:11434/v1\n\nqwen\n\n");
        let mut prompts = Vec::new();
        let pairs = init_exports(&mut input, &mut prompts).unwrap();
        assert_eq!(pairs, vec![
            ("QUECTO_BASE_URL".to_string(), "http://localhost:11434/v1".to_string()),
            ("QUECTO_MODEL".to_string(), "qwen".to_string()),
        ]);
    }
```

- [ ] **Step 2: Write the failing CLI subprocess test** (append to `tests/cli.rs`)

```rust
#[test]
fn init_prints_exports() {
    let mut child = Command::new(bin())
        .arg("--init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn().unwrap();
    child.stdin.take().unwrap()
        .write_all(b"http://localhost:11434/v1\n\nqwen\n\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("export QUECTO_BASE_URL=\"http://localhost:11434/v1\""));
    assert!(s.contains("export QUECTO_MODEL=\"qwen\""));
    assert!(!s.contains("QUECTO_API_KEY"));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib init_exports_skips_blanks` then `cargo test --test cli init_prints_exports`
Expected: FAIL — `cannot find function init_exports`; the CLI test sees no `--init` handling (stdout lacks `export` lines).

- [ ] **Step 4: Implement `init_exports`** (add to `src/lib.rs`)

```rust
/// Interactive env bootstrap: prompt (on `prompts`) for each knob, read answers
/// from `input`, and return the (var, value) pairs the user actually set (blanks
/// skipped). The binary prints these as `export VAR="value"`; the agent's wizard
/// reuses it for its first section.
pub fn init_exports(
    input: &mut impl std::io::BufRead,
    prompts: &mut impl std::io::Write,
) -> std::io::Result<Vec<(String, String)>> {
    let fields = [
        ("QUECTO_BASE_URL", "Base URL [http://localhost:11434/v1]: "),
        ("QUECTO_API_KEY", "API key (blank for none): "),
        ("QUECTO_MODEL", "Model [gpt-4o]: "),
        ("QUECTO_SYSTEM", "System prompt (blank for none): "),
    ];
    let mut out = Vec::new();
    for (var, prompt) in fields {
        write!(prompts, "{prompt}")?;
        prompts.flush()?;
        let mut line = String::new();
        input.read_line(&mut line)?;
        let val = line.trim();
        if !val.is_empty() {
            out.push((var.to_string(), val.to_string()));
        }
    }
    Ok(out)
}
```

- [ ] **Step 5: Wire `--init` into `src/main.rs`** (add `run_init` and update `main`)

```rust
fn run_init() -> Result<(), BoxErr> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stderr = io::stderr();
    let mut prompts = stderr.lock();
    let pairs = quecto::init_exports(&mut input, &mut prompts)?;
    for (k, v) in pairs {
        println!("export {k}=\"{v}\"");
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("--init") {
        if let Err(e) = run_init() {
            eprintln!("quecto: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.is_empty() {
        run_repl();
    } else {
        run_oneshot(&args.join(" "));
    }
}
```

(Replace the existing `fn main` from Task 6 with this version.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS (all lib unit tests + http + stream + cli).

- [ ] **Step 7: Final full verification**

Run: `cargo build && cargo test && cargo clippy --all-targets`
Expected: build succeeds, all tests pass, no clippy errors.

- [ ] **Step 8: Commit**

```bash
git add src/lib.rs src/main.rs tests/cli.rs
git commit -m "feat: add --init env bootstrapper (flag + reusable library helper)"
```

---

## Self-Review

**Spec coverage** (core spec `2026-07-09-quecto-harness-design.md`):

- Public API — 4 functions: `quecto_raw` (T3), `quecto_stream` (T5), `quecto_to` (T4), `quecto` (T4). ✅
- Opinion boundary (primitives impose nothing; conveniences add path/Bearer/shape): `quecto_raw`/`quecto_stream` take exact url/headers/body; `quecto_to`/`quecto` add `/chat/completions`, Bearer, message shape. ✅
- Streaming: forces `stream:true`, full-delta callback, accumulates content, `[DONE]` stop, non-SSE fallback (T5); `QUECTO_STREAM` off-switch (T6). ✅
- System prompt: via `build_body(Some(system), …)` in `quecto`/CLI; `QUECTO_SYSTEM` in `env_config` (T4/T6); `quecto_to` deliberately not widened. ✅
- Configuration: all 4 env vars + `QUECTO_STREAM` (T4 `env_config`, T6). ✅
- Error handling: `BoxErr` everywhere; `"no choices in response"` inline error (T2); non-2xx via `?` (T3). ✅
- Dependencies: `ureq` 2 + `json`, `serde_json` 1, no defaults disabled (T1). ✅
- CLI one-shot (arg join, stream/buffered, trailing newline, exit 1 on error) T6; interactive REPL (stderr prompt, EOF/exit/quit, blank skip, stateless, per-turn error continues) T6; `--init` flag + reusable helper T7. ✅
- Crate structure: workspace root + core package, two source files (T1/T6). ✅
- Timeouts: connect+read, not overall deadline (`agent()`, T3). ✅

Note: `--init` prints exports to **stdout** with prompts on **stderr** (T7), matching the spec's `eval "$(quecto --init)"` design — not writing a `.env`, keeping the "no config-file reading" non-goal intact.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to Task N" — every code step contains complete code. ✅

**Type consistency:** `BoxErr` alias used uniformly; `extract_content` returns `Result<String, BoxErr>` and is called with `?` in `quecto_to`/`quecto`/`answer`; `build_body(system: Option<&str>, …)` signature identical at every call site; `join_url(base, "chat/completions")` consistent; `parse_sse_delta`/`handle_frame` signatures match their callers. ✅

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-10-quecto-core.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
