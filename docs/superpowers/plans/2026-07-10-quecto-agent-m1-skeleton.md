# quecto-agent M1 — Walking Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `quecto-agent` crate as a workspace member and get `quecto-agent "<task>"` to run one real model turn end-to-end through the `quecto` core — the walking skeleton the rest of the agent MVP builds on.

**Architecture:** A new `quecto-agent` library + binary in the existing Cargo workspace. `model.rs` defines the normalized message types (`Message`, `AssistantMessage`, `ToolCall`), a `parse_assistant` that reads the OpenAI-compatible buffered response (native tool-call protocol), a `messages_to_body` serializer, a `Model` trait (so the loop is testable without HTTP), and `HttpModel` (the real client, buffered `quecto_raw`). `agent.rs` holds the loop: append the task, call the model, record the reply, finish when it stops requesting tools. **No tools are registered in M1** — an unexpected tool call is reported back as an error observation and the loop continues (the loop shape M2 will fill in). `main.rs` is a minimal one-shot CLI.

**Tech Stack:** Rust (edition 2021), depends only on `quecto` (path) + `serde_json`. No async, no `clap`, no tools/sandbox/session/flavors (all later milestones).

## Global Constraints

- Rust edition = **2021** (matches the core).
- `quecto-agent` dependencies in M1 are **exactly** `quecto = { path = ".." }` and `serde_json = "1"`. No `tokio`, no `clap`, no `serde` derive, no other crates — those arrive in later milestones.
- Error type everywhere: `pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;` (define once in `lib.rs`, mirror the core).
- The agent loop talks to models **only** through the core via **buffered `quecto_raw`** — never `quecto_stream`, never streaming in the loop.
- **Native tool-call protocol only** in M1 (`parse_assistant` reads `choices[0].message.tool_calls`). The `text` protocol is deferred to M7.
- **No tools registered in M1.** Do not build the `Tool` trait, registry, sandbox, patch engine, policy, session, verify, flavors, or `render.rs` — every one of those is a later milestone. Build only what this plan lists.
- `Cargo.toml` uses Cargo's automatic target discovery (`src/lib.rs` → lib `quecto_agent`, `src/main.rs` → bin `quecto-agent`); do not add explicit `[lib]`/`[[bin]]` sections.
- The crate name is `quecto-agent`; its library crate name is therefore `quecto_agent` (hyphen → underscore) — use `quecto_agent::…` from the binary and integration tests.

---

## File Structure

- `Cargo.toml` (root) — add `quecto-agent` to `[workspace] members`. (Modified once, Task 1.)
- `quecto-agent/Cargo.toml` — the new crate manifest. (Created Task 1.)
- `quecto-agent/src/lib.rs` — `BoxErr` + module declarations + re-exports. Grows across Tasks 1–3.
- `quecto-agent/src/model.rs` — message types, `parse_assistant`, `messages_to_body`, `Model` trait (Task 1); `HttpModel` (Task 2). Pure-function tests inline.
- `quecto-agent/src/agent.rs` — `Agent`, `Outcome`, `run` + scripted-fake unit tests. (Task 3.)
- `quecto-agent/src/main.rs` — one-shot CLI. (Task 4.)
- `quecto-agent/tests/common/mod.rs` — the dependency-free mock HTTP server (hardened, drains the request before responding). (Created Task 2, reused Task 4.)
- `quecto-agent/tests/model.rs` — `HttpModel` integration test against the mock. (Task 2.)
- `quecto-agent/tests/cli.rs` — subprocess end-to-end test via `CARGO_BIN_EXE_quecto-agent`. (Task 4.)

---

### Task 1: Workspace wiring + model types, `parse_assistant`, `messages_to_body`

**Files:**
- Modify: `Cargo.toml` (root) — `[workspace] members`
- Create: `quecto-agent/Cargo.toml`
- Create: `quecto-agent/src/lib.rs`
- Create: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: the `quecto` core crate (path dependency).
- Produces:
  - `pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;`
  - `pub struct Message { pub role: String, pub content: String }` with constructors `system/user/assistant/tool`.
  - `pub struct ToolCall { pub id: String, pub name: String, pub arguments: serde_json::Value }`
  - `pub struct AssistantMessage { pub content: String, pub tool_calls: Vec<ToolCall>, pub finish_reason: String }`
  - `pub fn parse_assistant(resp: &serde_json::Value) -> Result<AssistantMessage, BoxErr>`
  - `pub fn messages_to_body(model: &str, messages: &[Message]) -> serde_json::Value`
  - `pub trait Model: Send + Sync { fn complete(&self, messages: &[Message]) -> Result<AssistantMessage, BoxErr>; }`

- [ ] **Step 1: Add the crate to the workspace** — edit root `Cargo.toml`

Change:
```toml
[workspace]
members = ["."]
# Future companion members (own specs): "quecto-agent", "quecto-mcp"
```
to:
```toml
[workspace]
members = [".", "quecto-agent"]
# Future companion members (own specs): "quecto-mcp"
```

- [ ] **Step 2: Create `quecto-agent/Cargo.toml`**

```toml
[package]
name = "quecto-agent"
version = "0.1.0"
edition = "2021"
description = "Coding agent built on the quecto core."
license = "MIT"

[dependencies]
quecto = { path = ".." }
serde_json = "1"
```

- [ ] **Step 3: Create `quecto-agent/src/lib.rs`** with the `BoxErr` alias, the `model` module, and re-exports (only what exists after this task)

```rust
//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod model;

pub use model::{messages_to_body, parse_assistant, AssistantMessage, Message, Model, ToolCall};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
```

- [ ] **Step 4: Write the failing tests** — create `quecto-agent/src/model.rs` with the types' fields referenced only by the test module (functions not yet implemented)

Create `quecto-agent/src/model.rs` containing exactly this test module plus the imports it needs (the `use` line and the type/function definitions come in Step 6 — for now the file will not compile, which is the RED state):

```rust
use crate::BoxErr;
use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_content() {
        let r = json!({"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.finish_reason, "stop");
        assert!(m.tool_calls.is_empty());
    }

    #[test]
    fn parses_native_tool_call_with_string_arguments() {
        let r = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}
        ]},"finish_reason":"tool_calls"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "");
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].id, "call_1");
        assert_eq!(m.tool_calls[0].name, "read_file");
        assert_eq!(m.tool_calls[0].arguments, json!({"path":"a.rs"}));
    }

    #[test]
    fn errors_on_missing_choices() {
        assert!(parse_assistant(&json!({"error":"x"})).is_err());
    }

    #[test]
    fn messages_to_body_shape() {
        let body = messages_to_body("m", &[Message::system("s"), Message::user("u")]);
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "u");
    }
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib`
Expected: FAIL — `cannot find function parse_assistant` / `messages_to_body` / `cannot find type Message`.

- [ ] **Step 6: Implement the types and functions** — add above the `#[cfg(test)]` module in `quecto-agent/src/model.rs`

```rust
/// A single chat message in the running transcript.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self { Message { role: "system".into(), content: c.into() } }
    pub fn user(c: impl Into<String>) -> Self { Message { role: "user".into(), content: c.into() } }
    pub fn assistant(c: impl Into<String>) -> Self { Message { role: "assistant".into(), content: c.into() } }
    pub fn tool(c: impl Into<String>) -> Self { Message { role: "tool".into(), content: c.into() } }
}

/// One requested tool call, normalized from the provider response.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// The assistant's turn: free text plus any tool calls it requested.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

/// Parse an OpenAI-compatible buffered chat response (native tool-call protocol)
/// into a normalized AssistantMessage. Content absent/null → ""; tool_calls absent → [].
pub fn parse_assistant(resp: &Value) -> Result<AssistantMessage, BoxErr> {
    let choice = resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or("no choices in response")?;
    let message = choice.get("message").ok_or("no message in choice")?;
    let content = message.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let finish_reason = choice.get("finish_reason").and_then(|f| f.as_str()).unwrap_or("").to_string();

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for call in calls {
            let id = call.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let func = call.get("function").ok_or("tool_call missing function")?;
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // Native protocol encodes arguments as a JSON string; tolerate an object too.
            let arguments = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            tool_calls.push(ToolCall { id, name, arguments });
        }
    }
    Ok(AssistantMessage { content, tool_calls, finish_reason })
}

/// Serialize the transcript into an OpenAI-compatible request body.
pub fn messages_to_body(model: &str, messages: &[Message]) -> Value {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.content}))
        .collect();
    json!({"model": model, "messages": msgs})
}

/// Abstraction over "take the transcript, return the assistant's next message."
/// The real impl calls the model over HTTP; tests inject a scripted fake.
pub trait Model: Send + Sync {
    fn complete(&self, messages: &[Message]) -> Result<AssistantMessage, BoxErr>;
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib`
Expected: PASS (4 tests). `cargo build -p quecto-agent` succeeds, warning-free.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml quecto-agent/Cargo.toml quecto-agent/src/lib.rs quecto-agent/src/model.rs
git commit -m "feat(agent): scaffold quecto-agent crate with model types and parse_assistant"
```

---

### Task 2: `HttpModel` (buffered `quecto_raw` client) + mock server

**Files:**
- Modify: `quecto-agent/src/model.rs`
- Modify: `quecto-agent/src/lib.rs`
- Create: `quecto-agent/tests/common/mod.rs`
- Create: `quecto-agent/tests/model.rs`

**Interfaces:**
- Consumes: `Model`, `Message`, `AssistantMessage`, `parse_assistant`, `messages_to_body` (Task 1); `quecto::quecto_raw`, `quecto::join_url`, `quecto::env_config` (core).
- Produces:
  - `pub struct HttpModel { pub url: String, pub api_key: Option<String>, pub model: String }`
  - `impl HttpModel { pub fn from_env() -> Self }` — builds `url`/`api_key`/`model` from `quecto::env_config()`.
  - `impl Model for HttpModel` — buffered `quecto_raw` → `parse_assistant`.
  - Test helper `common::mock(status: u16, content_type: &str, body: &str) -> String` returning the base URL.

- [ ] **Step 1: Create the mock server helper** — `quecto-agent/tests/common/mod.rs` (the hardened version: fully drains the request before responding, so it is reliable under the parallel test runner)

```rust
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// One-shot mock HTTP server. Serves one connection with `status` + `body`
/// (using `content_type`), then the thread exits. Returns "http://127.0.0.1:PORT".
/// Reads the client's full request (headers + Content-Length body) before
/// responding, so it stays reliable under parallel test execution.
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
            read_request(&mut stream);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    format!("http://{addr}")
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos + 4;
                }
            }
            Err(_) => return,
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let already = buf.len() - header_end;
    let mut remaining = content_length.saturating_sub(already);
    while remaining > 0 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => remaining = remaining.saturating_sub(n),
            Err(_) => break,
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
```

- [ ] **Step 2: Write the failing integration test** — `quecto-agent/tests/model.rs`

```rust
mod common;
use common::mock;
use quecto_agent::{HttpModel, Message, Model};

#[test]
fn http_model_completes_against_mock() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#);
    let m = HttpModel {
        url: format!("{base}/chat/completions"),
        api_key: None,
        model: "m".to_string(),
    };
    let msg = m.complete(&[Message::user("hey")]).unwrap();
    assert_eq!(msg.content, "hi");
    assert!(msg.tool_calls.is_empty());
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p quecto-agent --test model`
Expected: FAIL — `cannot find type HttpModel in crate quecto_agent`.

- [ ] **Step 4: Implement `HttpModel`** — add to `quecto-agent/src/model.rs`

```rust
/// The real model client: buffered `quecto_raw` against an OpenAI-compatible endpoint.
pub struct HttpModel {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl HttpModel {
    /// Build from the core's env config (QUECTO_BASE_URL / QUECTO_API_KEY / QUECTO_MODEL).
    pub fn from_env() -> Self {
        let (base, key, model, _system) = quecto::env_config();
        HttpModel {
            url: quecto::join_url(&base, "chat/completions"),
            api_key: key,
            model,
        }
    }
}

impl Model for HttpModel {
    fn complete(&self, messages: &[Message]) -> Result<AssistantMessage, BoxErr> {
        let body = messages_to_body(&self.model, messages);
        let auth = self.api_key.as_ref().map(|k| format!("Bearer {k}"));
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(a) = &auth {
            headers.push(("Authorization", a.as_str()));
        }
        let resp = quecto::quecto_raw(&self.url, &headers, body)?;
        parse_assistant(&resp)
    }
}
```

- [ ] **Step 5: Re-export `HttpModel`** — update the `pub use` in `quecto-agent/src/lib.rs`

```rust
pub use model::{messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS (4 lib + 1 model integration). Warning-free.

- [ ] **Step 7: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/lib.rs quecto-agent/tests/common/mod.rs quecto-agent/tests/model.rs
git commit -m "feat(agent): add HttpModel buffered client with mock-server test"
```

---

### Task 3: The agent loop (`Agent`, `Outcome`, `run`)

**Files:**
- Create: `quecto-agent/src/agent.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `Model`, `Message`, `AssistantMessage`, `ToolCall`, `BoxErr` (Tasks 1–2).
- Produces:
  - `pub enum Outcome { Complete(String), StepLimit, Error(BoxErr) }`
  - `pub struct Agent { … }`
  - `impl Agent { pub fn new(model: Box<dyn Model>, system: impl Into<String>, max_steps: usize) -> Self; pub fn run(&mut self, task: &str) -> Outcome }`

- [ ] **Step 1: Write the failing tests** — create `quecto-agent/src/agent.rs` with a scripted fake `Model` and three tests (implementation added in Step 3)

```rust
use crate::model::{Message, Model};
use crate::BoxErr;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantMessage, ToolCall};
    use serde_json::json;
    use std::sync::Mutex;

    /// A fake model that returns pre-scripted replies in order.
    struct Scripted {
        replies: Mutex<Vec<AssistantMessage>>,
    }
    impl Scripted {
        fn new(replies: Vec<AssistantMessage>) -> Self {
            Scripted { replies: Mutex::new(replies) }
        }
    }
    impl Model for Scripted {
        fn complete(&self, _messages: &[Message]) -> Result<AssistantMessage, BoxErr> {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                return Err("no more scripted replies".into());
            }
            Ok(r.remove(0))
        }
    }
    fn text(c: &str) -> AssistantMessage {
        AssistantMessage { content: c.to_string(), tool_calls: vec![], finish_reason: "stop".to_string() }
    }
    fn wants_tool(name: &str) -> AssistantMessage {
        AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall { id: "1".to_string(), name: name.to_string(), arguments: json!({}) }],
            finish_reason: "tool_calls".to_string(),
        }
    }

    #[test]
    fn completes_on_text_only_reply() {
        let m = Scripted::new(vec![text("hello")]);
        let mut a = Agent::new(Box::new(m), "sys", 10);
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn unknown_tool_is_reported_then_completes() {
        // No tools registered in M1: the tool call is answered with an error
        // observation, and the model's next (text) reply completes the run.
        let m = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        let mut a = Agent::new(Box::new(m), "sys", 10);
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after error observation"),
        }
    }

    #[test]
    fn step_limit_stops_a_spinning_model() {
        let m = Scripted::new(vec![wants_tool("x"), wants_tool("x"), wants_tool("x")]);
        let mut a = Agent::new(Box::new(m), "sys", 2);
        assert!(matches!(a.run("hi"), Outcome::StepLimit));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib agent`
Expected: FAIL — `cannot find type Agent` / `Outcome`.

- [ ] **Step 3: Implement the loop** — add above the `#[cfg(test)]` module in `quecto-agent/src/agent.rs`

```rust
/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    Error(BoxErr),
}

/// The agent loop. Milestone 1: reason → (no tools yet) → answer.
pub struct Agent {
    model: Box<dyn Model>,
    messages: Vec<Message>,
    max_steps: usize,
}

impl Agent {
    /// Create an agent with a model, a system prompt, and a step limit.
    pub fn new(model: Box<dyn Model>, system: impl Into<String>, max_steps: usize) -> Self {
        Agent {
            model,
            messages: vec![Message::system(system.into())],
            max_steps,
        }
    }

    /// Run one task to completion (or a limit/error). Appends the task as a user
    /// message and loops: call the model, record its reply, finish when it stops
    /// requesting tools. No tools are registered in M1, so any tool call is
    /// reported back as an error observation and the loop continues.
    pub fn run(&mut self, task: &str) -> Outcome {
        self.messages.push(Message::user(task));
        let mut step = 0;
        loop {
            if step >= self.max_steps {
                return Outcome::StepLimit;
            }
            let msg = match self.model.complete(&self.messages) {
                Ok(m) => m,
                Err(e) => return Outcome::Error(e),
            };
            self.messages.push(Message::assistant(msg.content.clone()));
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                self.messages.push(Message::tool(format!(
                    "error: tool '{}' is not available",
                    call.name
                )));
            }
            step += 1;
        }
    }
}
```

- [ ] **Step 4: Register the module and re-export** — update `quecto-agent/src/lib.rs`

Add the module declaration after `mod model;`:
```rust
mod agent;
```
Add its re-export (a separate `pub use` line is fine):
```rust
pub use agent::{Agent, Outcome};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS (all lib unit tests + the model integration test). Warning-free.

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/agent.rs quecto-agent/src/lib.rs
git commit -m "feat(agent): add the agent loop (Agent, Outcome, run) with scripted-model tests"
```

---

### Task 4: One-shot CLI (`main.rs`) + end-to-end subprocess test

**Files:**
- Create: `quecto-agent/src/main.rs`
- Create: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Consumes: `Agent`, `Outcome`, `HttpModel` (Tasks 1–3).
- Produces: the `quecto-agent` binary. `quecto-agent <task words>` → runs one task, prints the answer to stdout; `StepLimit`/`Error` → stderr + exit 1; no args → usage to stderr + exit 2.

- [ ] **Step 1: Write the failing subprocess test** — `quecto-agent/tests/cli.rs`

```rust
mod common;
use common::mock;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_quecto-agent")
}

#[test]
fn oneshot_prints_model_answer() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"42"},"finish_reason":"stop"}]}"#);
    let out = Command::new(bin())
        .arg("what").arg("is").arg("6x7")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");
}

#[test]
fn no_args_is_usage_error() {
    let out = Command::new(bin()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-agent --test cli`
Expected: FAIL — build error: no binary target `quecto-agent` / `CARGO_BIN_EXE_quecto-agent` unset (because `src/main.rs` does not exist yet).

- [ ] **Step 3: Create `quecto-agent/src/main.rs`**

```rust
use quecto_agent::{Agent, HttpModel, Outcome};

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: quecto-agent \"<task>\"");
        std::process::exit(2);
    }
    let task = args.join(" ");
    let system = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
    let max_steps = std::env::var("QUECTO_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let model = HttpModel::from_env();
    let mut agent = Agent::new(Box::new(model), system, max_steps);

    match agent.run(&task) {
        Outcome::Complete(answer) => println!("{answer}"),
        Outcome::StepLimit => {
            eprintln!("quecto-agent: step limit reached");
            std::process::exit(1);
        }
        Outcome::Error(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --test cli`
Expected: PASS (2 tests).

- [ ] **Step 5: Full verification**

Run: `cargo test -p quecto-agent && cargo clippy -p quecto-agent --all-targets`
Expected: all tests pass (4 lib + 1 model + 2 cli = 7), no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs
git commit -m "feat(agent): add one-shot CLI with end-to-end subprocess test"
```

---

## Self-Review

**Spec coverage** (against `2026-07-10-quecto-agent-architecture.md`, scoped to the M1 walking skeleton):

- Crate layout — `quecto-agent` as a workspace lib + bin (spec "library + default binary"): T1/T4. ✅
- `model.rs` — buffered turns + `parse_assistant` normalizing native protocol into `AssistantMessage { content, tool_calls, finish_reason }` (spec §"Buffered turns + tool-call transport"): T1/T2. ✅ (`text` protocol explicitly deferred to M7 per Global Constraints.)
- Agent loop — `Agent`, `Outcome`, `run`; buffered `quecto_raw`; `max_steps` guard; finish when `tool_calls` empty (spec §"The agent loop"): T3. ✅
- Talks to models **only** through the core via buffered `quecto_raw` (spec §"Relationship to the core"): `HttpModel` (T2). ✅
- CLI one-shot `quecto-agent "<task>"` (spec §"Renderer & CLI"): T4. ✅

**Deliberately deferred (later milestones, per the spec's own MVP layering and this plan's Global Constraints):** tools/registry/patch/sandbox/policy (M2–M4), verify gate + instruction loader + context seed (M5), SQLite session + resume/undo/chat + slash-commands (M6), flavors + `text` protocol + `new`/`init` + `clap` subcommands (M7), MCP (optional feature, later), cancellation via `ctrlc` (M4), the activity renderer `render.rs` (M2+). None are in M1; the walking skeleton intentionally has no tools.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to Task N"; every code step contains complete code. ✅

**Type consistency:** `BoxErr` used uniformly; `Model::complete(&self, &[Message]) -> Result<AssistantMessage, BoxErr>` identical at the trait, `HttpModel` impl, and the `Scripted` test fake; `parse_assistant`/`messages_to_body` signatures match their call sites; `Agent::new(Box<dyn Model>, impl Into<String>, usize)` matches every construction; `Outcome` variants (`Complete/StepLimit/Error`) matched consistently in the loop, tests, and `main`. ✅

**Note on scope decisions I made** (flag for review): (1) M1 uses **plain `std::env::args`**, not `clap` — the spec lists `clap`, but subcommands don't exist until M6, so pulling `clap` now would be premature; the plan adds it when subcommands arrive. (2) The system prompt is a **hardcoded default + `QUECTO_SYSTEM` override**; the layered flavor/instruction system is M5–M7. (3) `QUECTO_MAX_STEPS` is a small convenience env knob for the skeleton; the real `--max-steps` flag lands with `clap` in M6.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-10-quecto-agent-m1-skeleton.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
