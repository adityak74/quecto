# quecto-agent M2 — Tool System + Read-Only Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `quecto-agent` a working tool loop with the five read-only tools (`read_file`, `list_files`, `search_text`, `git_diff`, `git_status`), so the agent can explore a repository: request a tool → it executes (path-safe, gitignore-aware) → the result feeds back → the model continues until it answers.

**Architecture:** Build on the M1 walking skeleton. First extend the transcript so tool calls round-trip (assistant messages carry `tool_calls`; tool results carry `tool_call_id`) and the `Model` call sends tool schemas. Then add the tool infrastructure in `src/tools/`: a `Tool` trait, `ToolOutput`/`ToolError`, a `Context` (repo root + a path-safety resolver that rejects escapes), and a `Registry` (register/schemas/dispatch). Add the five read-only tools. Finally wire the `Registry` + `Context` into the agent loop: send `registry.schemas()`, dispatch each `tool_call`, append the result as a `tool` message, print a one-line activity marker. **Editing, sandbox, verification, session, flavors, and the rich renderer are still deferred** (M3–M7).

**Tech Stack:** Rust (edition 2021). Adds `ignore` (gitignore-aware walking) and `regex` (search); `tempfile` as a dev-dependency for filesystem tests. Git tools shell out to the `git` binary (read-only). Still no async.

## Global Constraints

- Rust edition = **2021**. No async, no `tokio`.
- Error type: `BoxErr = Box<dyn std::error::Error + Send + Sync>` for fallible core paths; **tool failures use `ToolError` and are reported to the model, never `?`-propagated as fatal**.
- The loop still calls models **only** through the core via buffered `quecto_raw` (native tool-call protocol). No streaming in the loop.
- **Path safety is mandatory** for every filesystem tool: paths resolve against the repo root and must canonicalize to inside it; any `..`/symlink escape is rejected before I/O.
- New runtime dependencies are limited to **`ignore = "0.4"`** and **`regex = "1"`**; dev-dependency **`tempfile = "3"`**. No other crates. (`clap`, `rusqlite`, `crossterm`, sandbox/session crates are later milestones.)
- **Only the five read-only tools** in M2. Do NOT build `write_file`, `apply_patch`, `run_command`, the sandbox, approval policy, verification, session, flavors, or `ask_user` — each is a later milestone.
- **No flavor allow-list yet:** the registry exposes every registered tool; `[tools]` filtering arrives in M7.
- Activity output is a **single inline `● name  summary` line to stderr** per tool call — the full renderer/slash-commands/color are M6. Do not add `crossterm` or a `render.rs` module.

### Milestone simplifications (deviations from the spec, deliberate — flag at review, don't "fix")

- `search_text` uses **`ignore` (walk) + `regex` (match)** rather than the ripgrep `grep-searcher`/`grep-regex` libraries. Same outcome (in-process, gitignore-aware, no `rg` binary); simpler to implement now. Swappable later if perf demands.
- `git_diff`/`git_status` **shell out to `git`** directly (read-only, fixed args, not model-controlled). They run outside the M4 sandbox because the sandbox does not exist yet and their arguments are hard-coded.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `ignore`, `regex`; dev `tempfile`. (Tasks 3–4.)
- `quecto-agent/src/model.rs` — `Message` gains `tool_calls` + `tool_call_id`; new constructors; `messages_to_body` emits them; `Model::complete` gains a `tools` argument; `HttpModel` sends `tools`. (Task 1.)
- `quecto-agent/src/tools/mod.rs` — `Tool`, `ToolOutput`, `ToolError`, `ToolResult`, `Context`, `Registry`, `cap_output`. (Task 2.)
- `quecto-agent/src/tools/fs.rs` — `ReadFile`, `ListFiles`. (Task 3.)
- `quecto-agent/src/tools/search.rs` — `SearchText`. (Task 4.)
- `quecto-agent/src/tools/git.rs` — `GitDiff`, `GitStatus`, shared `run_git`. (Task 4.)
- `quecto-agent/src/agent.rs` — `Agent` gains `Registry` + `Context`; `new` takes `repo_root`; `register`/`register_builtins`; loop dispatches tools. (Tasks 1 & 5.)
- `quecto-agent/src/lib.rs` — module declarations + re-exports, grown per task.
- `quecto-agent/src/main.rs` — build the agent with the repo root and built-in tools. (Task 5.)

---

### Task 1: Transcript round-trips tool calls; model sends tool schemas

**Files:**
- Modify: `quecto-agent/src/model.rs`
- Modify: `quecto-agent/src/agent.rs` (call site + test fake signature only)
- Modify: `quecto-agent/tests/model.rs`

**Interfaces:**
- Consumes: M1 `ToolCall`, `AssistantMessage`, `parse_assistant`.
- Produces:
  - `Message { role: String, content: String, tool_calls: Vec<ToolCall>, tool_call_id: Option<String> }`
    with constructors `system/user/assistant/tool` (unchanged callable form) plus
    `assistant_with_calls(content, Vec<ToolCall>)` and `tool_result(id, content)`.
  - `messages_to_body` emits native `tool_calls` on assistant messages (arguments as a JSON **string**) and `tool_call_id` on tool messages.
  - `Model::complete(&self, messages: &[Message], tools: &[serde_json::Value]) -> Result<AssistantMessage, BoxErr>` — new `tools` parameter; `HttpModel` inserts a `"tools"` field when non-empty.

- [ ] **Step 1: Write the failing tests** — replace the `messages_to_body_shape` test in `quecto-agent/src/model.rs`'s `mod tests` and add two more

```rust
    #[test]
    fn messages_to_body_shape() {
        let body = messages_to_body("m", &[Message::system("s"), Message::user("u")]);
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "u");
        // plain messages carry no tool fields
        assert!(body["messages"][0].get("tool_calls").is_none());
        assert!(body["messages"][1].get("tool_call_id").is_none());
    }

    #[test]
    fn assistant_tool_call_serializes_native_shape() {
        let call = ToolCall { id: "c1".into(), name: "read_file".into(), arguments: json!({"path":"a.rs"}) };
        let body = messages_to_body("m", &[Message::assistant_with_calls("", vec![call])]);
        let tc = &body["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "c1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        // arguments is a JSON *string* per the native protocol
        assert_eq!(tc["function"]["arguments"], "{\"path\":\"a.rs\"}");
    }

    #[test]
    fn tool_result_serializes_with_id() {
        let body = messages_to_body("m", &[Message::tool_result("c1", "file contents")]);
        assert_eq!(body["messages"][0]["role"], "tool");
        assert_eq!(body["messages"][0]["tool_call_id"], "c1");
        assert_eq!(body["messages"][0]["content"], "file contents");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib`
Expected: FAIL — `assistant_with_calls` / `tool_result` not found; `tool_calls` field absent.

- [ ] **Step 3: Extend `Message` and its constructors** — replace the `Message` struct + `impl Message` block in `quecto-agent/src/model.rs`

```rust
/// A single chat message in the running transcript.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Tool calls requested by an assistant turn (empty otherwise).
    pub tool_calls: Vec<ToolCall>,
    /// The id of the assistant tool call this message answers (tool results only).
    pub tool_call_id: Option<String>,
}

impl Message {
    fn plain(role: &str, content: impl Into<String>) -> Self {
        Message { role: role.into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: None }
    }
    pub fn system(c: impl Into<String>) -> Self { Message::plain("system", c) }
    pub fn user(c: impl Into<String>) -> Self { Message::plain("user", c) }
    pub fn assistant(c: impl Into<String>) -> Self { Message::plain("assistant", c) }

    /// An assistant turn that requested tools.
    pub fn assistant_with_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message { role: "assistant".into(), content: content.into(), tool_calls, tool_call_id: None }
    }
    /// A tool result answering a specific assistant tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message { role: "tool".into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: Some(tool_call_id.into()) }
    }
}
```

- [ ] **Step 4: Emit tool fields in `messages_to_body`** — replace the `messages_to_body` function

```rust
/// Serialize the transcript into an OpenAI-compatible request body, including
/// native `tool_calls` on assistant turns and `tool_call_id` on tool results.
pub fn messages_to_body(model: &str, messages: &[Message]) -> Value {
    let msgs: Vec<Value> = messages.iter().map(message_to_json).collect();
    json!({"model": model, "messages": msgs})
}

fn message_to_json(m: &Message) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".into(), json!(m.role));
    obj.insert("content".into(), json!(m.content));
    if !m.tool_calls.is_empty() {
        let calls: Vec<Value> = m
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments.to_string() }
                })
            })
            .collect();
        obj.insert("tool_calls".into(), Value::Array(calls));
    }
    if let Some(id) = &m.tool_call_id {
        obj.insert("tool_call_id".into(), json!(id));
    }
    Value::Object(obj)
}
```

- [ ] **Step 5: Add the `tools` parameter to `Model` + `HttpModel`** — replace the `Model` trait and the `impl Model for HttpModel` block

```rust
/// Abstraction over "take the transcript (+ available tool schemas), return the
/// assistant's next message." The real impl calls the model over HTTP; tests inject a fake.
pub trait Model: Send + Sync {
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr>;
}

impl Model for HttpModel {
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
        let mut body = messages_to_body(&self.model, messages);
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
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

- [ ] **Step 6: Update the M1 call site + test fake in `quecto-agent/src/agent.rs`** (keep it compiling; real tool wiring is Task 5)

In `Agent::run`, change the model call to pass no tools yet:
```rust
            let msg = match self.model.complete(&self.messages, &[]) {
```
In the test module, change the `Scripted` impl signature and update the unknown-tool loop line (still uses `Message::tool` in M1 — replace with `tool_result`):
```rust
    impl Model for Scripted {
        fn complete(&self, _messages: &[Message], _tools: &[serde_json::Value]) -> Result<AssistantMessage, BoxErr> {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                return Err("no more scripted replies".into());
            }
            Ok(r.remove(0))
        }
    }
```
And in `Agent::run`'s unknown-tool branch, replace the `Message::tool(...)` push with:
```rust
            for call in &msg.tool_calls {
                self.messages.push(Message::tool_result(
                    &call.id,
                    format!("error: tool '{}' is not available", call.name),
                ));
            }
```
Also change the assistant push to preserve tool calls:
```rust
            self.messages.push(Message::assistant_with_calls(msg.content.clone(), msg.tool_calls.clone()));
```

- [ ] **Step 7: Update `quecto-agent/tests/model.rs`** — the integration test now passes no tools

```rust
    let msg = m.complete(&[Message::user("hey")], &[]).unwrap();
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS (all lib unit tests incl. the 2 new body tests; the model + cli integration tests). Warning-free.

- [ ] **Step 9: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/agent.rs quecto-agent/tests/model.rs
git commit -m "feat(agent): round-trip tool calls in the transcript; model sends tool schemas"
```

---

### Task 2: Tool trait, Context (path safety), Registry

**Files:**
- Create: `quecto-agent/src/tools/mod.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `ToolCall` (model).
- Produces:
  - `pub struct ToolOutput { pub content: String, pub summary: String }` + `ToolOutput::new`.
  - `pub struct ToolError { pub message: String }` + `ToolError::new`.
  - `pub type ToolResult = Result<ToolOutput, ToolError>;`
  - `pub trait Tool: Send + Sync { fn name(&self)->&str; fn description(&self)->&str; fn schema(&self)->Value; fn run(&self, args:&Value, cx:&mut Context)->ToolResult; }`
  - `pub struct Context { pub repo_root: PathBuf }` + `Context::new(PathBuf)` (canonicalizes) + `resolve_existing(&self, rel:&str)->Result<PathBuf, ToolError>`.
  - `pub struct Registry` + `new`/`register`/`is_empty`/`schemas()->Vec<Value>`/`dispatch(&ToolCall,&mut Context)->ToolOutput`.
  - `pub fn cap_output(s:&str, max:usize)->String`.

- [ ] **Step 1: Write the failing tests** — create `quecto-agent/src/tools/mod.rs` containing this test module and the `use` header (implementation added in Step 3)

```rust
use crate::model::ToolCall;
use serde_json::{json, Value};
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "echoes its text arg" }
        fn schema(&self) -> Value { json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}) }
        fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
            let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolOutput::new(t.to_string(), "echoed"))
        }
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments: args }
    }

    #[test]
    fn schemas_wrap_each_tool() {
        let mut r = Registry::new();
        r.register(Box::new(Echo));
        let s = r.schemas();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["type"], "function");
        assert_eq!(s[0]["function"]["name"], "echo");
        assert!(s[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn dispatch_routes_to_tool() {
        let mut r = Registry::new();
        r.register(Box::new(Echo));
        let mut cx = Context::new(PathBuf::from("."));
        let out = r.dispatch(&call("echo", json!({"text":"hi"})), &mut cx);
        assert_eq!(out.content, "hi");
    }

    #[test]
    fn dispatch_unknown_tool_is_error_output() {
        let r = Registry::new();
        let mut cx = Context::new(PathBuf::from("."));
        let out = r.dispatch(&call("nope", json!({})), &mut cx);
        assert!(out.content.contains("not available"));
    }

    #[test]
    fn resolve_rejects_escape() {
        let cx = Context::new(PathBuf::from("."));
        assert!(cx.resolve_existing("../../../etc/passwd").is_err());
    }

    #[test]
    fn cap_output_truncates() {
        let big = "x".repeat(100);
        let capped = cap_output(&big, 10);
        assert!(capped.len() < big.len());
        assert!(capped.contains("truncated"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib tools`
Expected: FAIL — `Registry`/`Context`/`Tool`/`ToolOutput`/`cap_output` not found. (Also update `lib.rs` per Step 4 so the module is compiled; do Step 4 first if the module isn't picked up.)

- [ ] **Step 3: Implement the tool infrastructure** — add above the `#[cfg(test)]` module in `quecto-agent/src/tools/mod.rs`

```rust
/// A successful tool result: `content` is fed back to the model; `summary` is the
/// short tail shown on the activity line.
pub struct ToolOutput {
    pub content: String,
    pub summary: String,
}
impl ToolOutput {
    pub fn new(content: impl Into<String>, summary: impl Into<String>) -> Self {
        ToolOutput { content: content.into(), summary: summary.into() }
    }
}

/// A tool failure. Reported back to the model as an observation — never fatal.
pub struct ToolError {
    pub message: String,
}
impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError { message: message.into() }
    }
}

pub type ToolResult = Result<ToolOutput, ToolError>;

/// A tool the model can call. `schema` is the JSON Schema for `run`'s `args`.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult;
}

/// Shared execution context. Milestone 2: the (canonical) repository root and the
/// path-safety resolver every filesystem tool must use.
pub struct Context {
    pub repo_root: PathBuf,
}
impl Context {
    pub fn new(repo_root: PathBuf) -> Self {
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        Context { repo_root }
    }

    /// Resolve a repo-relative path that must already exist, rejecting any path
    /// that canonicalizes to outside the repo root (`..`/symlink escapes).
    pub fn resolve_existing(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let canon = self
            .repo_root
            .join(rel)
            .canonicalize()
            .map_err(|e| ToolError::new(format!("{rel}: {e}")))?;
        if !canon.starts_with(&self.repo_root) {
            return Err(ToolError::new(format!("path '{rel}' escapes the repository root")));
        }
        Ok(canon)
    }
}

/// The universe of registered tools.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}
impl Registry {
    pub fn new() -> Self {
        Registry { tools: Vec::new() }
    }
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
    /// OpenAI-style tool schemas for the request `tools` field.
    pub fn schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": { "name": t.name(), "description": t.description(), "parameters": t.schema() }
                })
            })
            .collect()
    }
    /// Route a call to its tool. Unknown tool or a `ToolError` becomes an error
    /// `ToolOutput` (never a panic) so the loop can feed it back to the model.
    pub fn dispatch(&self, call: &ToolCall, cx: &mut Context) -> ToolOutput {
        match self.tools.iter().find(|t| t.name() == call.name) {
            None => ToolOutput::new(
                format!("error: tool '{}' is not available", call.name),
                "unknown tool",
            ),
            Some(t) => match t.run(&call.arguments, cx) {
                Ok(out) => out,
                Err(e) => ToolOutput::new(format!("error: {}", e.message), "error"),
            },
        }
    }
}
impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Truncate large tool output to a byte budget, appending a marker.
pub fn cap_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… {} bytes truncated …]", &s[..end], s.len() - end)
}
```

- [ ] **Step 4: Declare the module and re-export** — update `quecto-agent/src/lib.rs`

Add after `mod agent;`:
```rust
mod tools;
```
Add a re-export line:
```rust
pub use tools::{cap_output, Context, Registry, Tool, ToolError, ToolOutput, ToolResult};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS (new tool tests + all prior). Warning-free.

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/tools/mod.rs quecto-agent/src/lib.rs
git commit -m "feat(agent): add Tool trait, Context path-safety, and Registry"
```

---

### Task 3: Filesystem tools — `read_file`, `list_files`

**Files:**
- Modify: `quecto-agent/Cargo.toml` (add `ignore`; dev `tempfile`)
- Create: `quecto-agent/src/tools/fs.rs`
- Modify: `quecto-agent/src/tools/mod.rs` (declare `pub mod fs;`)

**Interfaces:**
- Consumes: `Tool`, `ToolOutput`, `ToolError`, `Context`, `cap_output` (Task 2).
- Produces: `pub struct ReadFile;` and `pub struct ListFiles;`, each `impl Tool`.

- [ ] **Step 1: Add dependencies** — edit `quecto-agent/Cargo.toml`

```toml
[dependencies]
quecto = { path = ".." }
serde_json = "1"
ignore = "0.4"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Write the failing tests** — create `quecto-agent/src/tools/fs.rs` with this test module + the `use` header (impl added in Step 4)

```rust
use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn read_file_returns_contents() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = ReadFile.run(&json!({"path":"a.txt"}), &mut cx).unwrap();
        assert_eq!(out.content, "hello\nworld\n");
    }

    #[test]
    fn read_file_honors_line_range() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = ReadFile.run(&json!({"path":"a.txt","start_line":2,"end_line":3}), &mut cx).unwrap();
        assert_eq!(out.content, "two\nthree");
    }

    #[test]
    fn read_file_missing_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        assert!(ReadFile.run(&json!({"path":"nope.txt"}), &mut cx).is_err());
    }

    #[test]
    fn list_files_lists_entries_gitignore_aware() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();
        fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = ListFiles.run(&json!({}), &mut cx).unwrap();
        assert!(out.content.contains("kept.txt"));
        assert!(!out.content.contains("ignored.txt"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib fs`
Expected: FAIL — `ReadFile`/`ListFiles` not found. (Complete Step 5 so the module is compiled if needed.)

- [ ] **Step 4: Implement the tools** — add above the `#[cfg(test)]` module in `quecto-agent/src/tools/fs.rs`

```rust
/// Read a UTF-8 file, optionally a `start_line`..=`end_line` slice (1-based).
pub struct ReadFile;
impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str {
        "Read a UTF-8 text file in the repository. Optional 1-based start_line/end_line select a range."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type":"string","description":"repo-relative file path"},
                "start_line": {"type":"integer"},
                "end_line": {"type":"integer"}
            },
            "required": ["path"]
        })
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("read_file: 'path' is required"))?;
        let full = cx.resolve_existing(path)?;
        let text = std::fs::read_to_string(&full)
            .map_err(|e| ToolError::new(format!("{path}: {e}")))?;

        let start = args.get("start_line").and_then(|v| v.as_u64());
        let end = args.get("end_line").and_then(|v| v.as_u64());
        let selected = if start.is_some() || end.is_some() {
            let lines: Vec<&str> = text.lines().collect();
            let s = start.unwrap_or(1).max(1) as usize;
            let e = (end.unwrap_or(lines.len() as u64) as usize).min(lines.len());
            lines.get(s.saturating_sub(1)..e).unwrap_or(&[]).join("\n")
        } else {
            text
        };
        let n = selected.lines().count();
        Ok(ToolOutput::new(cap_output(&selected, 64_000), format!("{n} lines")))
    }
}

/// List entries under a repo-relative directory (default the repo root),
/// gitignore-aware, depth-limited.
pub struct ListFiles;
impl Tool for ListFiles {
    fn name(&self) -> &str { "list_files" }
    fn description(&self) -> &str {
        "List files and directories under a repo-relative path (default the repo root). Respects .gitignore."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type":"string","description":"repo-relative directory (default '.')"} },
            "required": []
        })
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = cx.resolve_existing(rel)?;
        let mut entries = Vec::new();
        for dent in ignore::WalkBuilder::new(&base).max_depth(Some(2)).build() {
            let dent = match dent { Ok(d) => d, Err(_) => continue };
            if dent.depth() == 0 { continue; } // skip the base dir itself
            let shown = dent.path().strip_prefix(&cx.repo_root).unwrap_or(dent.path());
            entries.push(shown.display().to_string());
            if entries.len() >= 500 { break; }
        }
        entries.sort();
        let n = entries.len();
        Ok(ToolOutput::new(cap_output(&entries.join("\n"), 32_000), format!("{n} entries")))
    }
}
```

- [ ] **Step 5: Declare the submodule** — add to `quecto-agent/src/tools/mod.rs` (top level, above or below the infra)

```rust
pub mod fs;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib fs`
Expected: PASS (4 fs tests). Then `cargo test -p quecto-agent` all green.

- [ ] **Step 7: Commit**

```bash
git add quecto-agent/Cargo.toml quecto-agent/src/tools/fs.rs quecto-agent/src/tools/mod.rs
git commit -m "feat(agent): add read_file and list_files tools"
```

---

### Task 4: `search_text` + git tools (`git_diff`, `git_status`)

**Files:**
- Modify: `quecto-agent/Cargo.toml` (add `regex`)
- Create: `quecto-agent/src/tools/search.rs`
- Create: `quecto-agent/src/tools/git.rs`
- Modify: `quecto-agent/src/tools/mod.rs` (declare `pub mod search; pub mod git;`)

**Interfaces:**
- Consumes: `Tool`, `ToolOutput`, `ToolError`, `Context`, `cap_output` (Task 2).
- Produces: `pub struct SearchText;`, `pub struct GitDiff;`, `pub struct GitStatus;`, each `impl Tool`.

- [ ] **Step 1: Add dependency** — edit `quecto-agent/Cargo.toml` `[dependencies]`

```toml
regex = "1"
```

- [ ] **Step 2: Write the failing tests** — create `quecto-agent/src/tools/search.rs` and `quecto-agent/src/tools/git.rs` with their test modules

`quecto-agent/src/tools/search.rs`:
```rust
use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn search_text_finds_matches_with_line_numbers() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn main() {}\nlet x = 1;\nfn helper() {}\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = SearchText.run(&json!({"pattern":"fn "}), &mut cx).unwrap();
        assert!(out.content.contains("a.rs:1:"));
        assert!(out.content.contains("a.rs:3:"));
        assert!(!out.content.contains("a.rs:2:"));
    }

    #[test]
    fn search_text_reports_no_matches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "nothing here\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = SearchText.run(&json!({"pattern":"zzz"}), &mut cx).unwrap();
        assert!(out.content.contains("no matches"));
    }

    #[test]
    fn search_text_invalid_regex_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        assert!(SearchText.run(&json!({"pattern":"("}), &mut cx).is_err());
    }
}
```

`quecto-agent/src/tools/git.rs`:
```rust
use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn git_status_reports_not_a_repo() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        // A bare temp dir is not a git repo → a clear error result (not a panic).
        let res = GitStatus.run(&json!({}), &mut cx);
        assert!(res.is_err());
        assert!(res.err().unwrap().message.contains("not a git repository"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib search git`
Expected: FAIL — `SearchText`/`GitStatus` not found.

- [ ] **Step 4: Implement `search_text`** — add above the test module in `quecto-agent/src/tools/search.rs`

```rust
/// Regex search across the repository (gitignore-aware). Returns `path:line: text`.
pub struct SearchText;
impl Tool for SearchText {
    fn name(&self) -> &str { "search_text" }
    fn description(&self) -> &str {
        "Search the repository for a regular expression. Returns matching lines as path:line: text. Respects .gitignore."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type":"string","description":"regular expression"},
                "path": {"type":"string","description":"repo-relative directory to search (default '.')"}
            },
            "required": ["pattern"]
        })
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("search_text: 'pattern' is required"))?;
        let re = regex::Regex::new(pattern)
            .map_err(|e| ToolError::new(format!("invalid regex: {e}")))?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = cx.resolve_existing(rel)?;

        let mut hits: Vec<String> = Vec::new();
        'walk: for dent in ignore::WalkBuilder::new(&base).build() {
            let dent = match dent { Ok(d) => d, Err(_) => continue };
            if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
            let text = match std::fs::read_to_string(dent.path()) { Ok(t) => t, Err(_) => continue };
            let shown = dent.path().strip_prefix(&cx.repo_root).unwrap_or(dent.path()).display().to_string();
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}: {}", shown, i + 1, line.trim_end()));
                    if hits.len() >= 200 { break 'walk; }
                }
            }
        }
        let n = hits.len();
        let content = if hits.is_empty() { "no matches".to_string() } else { hits.join("\n") };
        Ok(ToolOutput::new(cap_output(&content, 32_000), format!("{n} matches")))
    }
}
```

- [ ] **Step 5: Implement the git tools** — add above the test module in `quecto-agent/src/tools/git.rs`

```rust
/// Run a read-only git command at the repo root. Fixed args, not model-controlled.
fn run_git(repo: &Path, args: &[&str]) -> Result<String, ToolError> {
    let out = std::process::Command::new("git")
        .arg("-C").arg(repo)
        .args(args)
        .output()
        .map_err(|e| ToolError::new(format!("git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("not a git repository") {
            return Err(ToolError::new("not a git repository"));
        }
        return Err(ToolError::new(format!("git failed: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Show the working-tree diff (read-only).
pub struct GitDiff;
impl Tool for GitDiff {
    fn name(&self) -> &str { "git_diff" }
    fn description(&self) -> &str { "Show the working-tree git diff." }
    fn schema(&self) -> Value { json!({"type":"object","properties":{},"required":[]}) }
    fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
        let diff = run_git(&cx.repo_root, &["diff"])?;
        let content = if diff.trim().is_empty() { "no changes".to_string() } else { diff };
        let summary = format!("{} lines", content.lines().count());
        Ok(ToolOutput::new(cap_output(&content, 64_000), summary))
    }
}

/// Show the working-tree status (read-only, porcelain).
pub struct GitStatus;
impl Tool for GitStatus {
    fn name(&self) -> &str { "git_status" }
    fn description(&self) -> &str { "Show the working-tree git status (porcelain)." }
    fn schema(&self) -> Value { json!({"type":"object","properties":{},"required":[]}) }
    fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
        let status = run_git(&cx.repo_root, &["status", "--porcelain"])?;
        let n = status.lines().filter(|l| !l.trim().is_empty()).count();
        let content = if status.trim().is_empty() { "clean".to_string() } else { status };
        Ok(ToolOutput::new(cap_output(&content, 32_000), format!("{n} changed")))
    }
}
```

- [ ] **Step 6: Declare the submodules** — add to `quecto-agent/src/tools/mod.rs`

```rust
pub mod git;
pub mod search;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib`
Expected: PASS (search + git tests + all prior). Then `cargo test -p quecto-agent` all green.

- [ ] **Step 8: Commit**

```bash
git add quecto-agent/Cargo.toml quecto-agent/src/tools/search.rs quecto-agent/src/tools/git.rs quecto-agent/src/tools/mod.rs
git commit -m "feat(agent): add search_text and git_diff/git_status tools"
```

---

### Task 5: Wire tools into the agent loop + built-ins + CLI

**Files:**
- Modify: `quecto-agent/src/agent.rs`
- Modify: `quecto-agent/src/tools/mod.rs` (add `builtin_tools()`)
- Modify: `quecto-agent/src/lib.rs` (re-export the tool structs + `builtin_tools`)
- Modify: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: `Registry`, `Context`, `Tool`, and the five tool structs (Tasks 2–4); `Model`, `Message` (Task 1).
- Produces:
  - `Agent::new(model: Box<dyn Model>, system: impl Into<String>, max_steps: usize, repo_root: PathBuf) -> Self`
  - `Agent::register(self, tool: Box<dyn Tool>) -> Self`
  - `Agent::register_builtins(self) -> Self`
  - `pub fn builtin_tools() -> Vec<Box<dyn Tool>>` (the five read-only tools).
  - Loop: sends `registry.schemas()`, dispatches each `tool_call`, appends a `tool_result`, prints `● name  summary` to stderr.

- [ ] **Step 1: Add `builtin_tools()`** — add to `quecto-agent/src/tools/mod.rs`

```rust
/// The read-only built-in tool set (milestone 2).
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFile),
        Box::new(fs::ListFiles),
        Box::new(search::SearchText),
        Box::new(git::GitDiff),
        Box::new(git::GitStatus),
    ]
}
```

- [ ] **Step 2: Write the failing loop-wiring test** — replace `quecto-agent/src/agent.rs`'s `mod tests` with this version (adds a recording tool and updates constructions for the new signature)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantMessage, ToolCall};
    use crate::tools::{Context, Tool, ToolOutput, ToolResult};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct Scripted {
        replies: Mutex<Vec<AssistantMessage>>,
    }
    impl Scripted {
        fn new(replies: Vec<AssistantMessage>) -> Self { Scripted { replies: Mutex::new(replies) } }
    }
    impl Model for Scripted {
        fn complete(&self, _messages: &[Message], _tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() { return Err("no more scripted replies".into()); }
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
    fn agent(model: Scripted) -> Agent {
        Agent::new(Box::new(model), "sys", 10, PathBuf::from("."))
    }

    /// A tool that flips a shared flag when run, so we can prove dispatch happened.
    struct Recording { ran: Arc<AtomicBool> }
    impl Tool for Recording {
        fn name(&self) -> &str { "rec" }
        fn description(&self) -> &str { "records that it ran" }
        fn schema(&self) -> Value { json!({"type":"object","properties":{},"required":[]}) }
        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            self.ran.store(true, Ordering::SeqCst);
            Ok(ToolOutput::new("recorded", "ok"))
        }
    }

    #[test]
    fn completes_on_text_only_reply() {
        match agent(Scripted::new(vec![text("hello")])).run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn dispatches_a_registered_tool_then_completes() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("rec"), text("done")]);
        let mut a = agent(model).register(Box::new(Recording { ran: ran.clone() }));
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete"),
        }
        assert!(ran.load(Ordering::SeqCst), "the tool should have been dispatched");
    }

    #[test]
    fn unknown_tool_is_reported_then_completes() {
        let model = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        match agent(model).run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after error observation"),
        }
    }

    #[test]
    fn step_limit_stops_a_spinning_model() {
        let model = Scripted::new(vec![wants_tool("x"), wants_tool("x"), wants_tool("x")]);
        let mut a = Agent::new(Box::new(model), "sys", 2, PathBuf::from("."));
        assert!(matches!(a.run("hi"), Outcome::StepLimit));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib agent`
Expected: FAIL — `Agent::new` takes 3 args / `register` not found.

- [ ] **Step 4: Rewrite `Agent`, its constructors, and the loop** — replace the non-test portion of `quecto-agent/src/agent.rs` (keep the imports updated)

```rust
use crate::model::{Message, Model};
use crate::tools::{builtin_tools, Context, Registry, Tool};
use crate::BoxErr;
use std::path::PathBuf;

/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    Error(BoxErr),
}

/// The agent loop. Milestone 2: reason → call read-only tools → observe → answer.
pub struct Agent {
    model: Box<dyn Model>,
    registry: Registry,
    cx: Context,
    messages: Vec<Message>,
    max_steps: usize,
}

impl Agent {
    /// Create an agent with a model, a system prompt, a step limit, and the
    /// repository root that filesystem tools are scoped to.
    pub fn new(
        model: Box<dyn Model>,
        system: impl Into<String>,
        max_steps: usize,
        repo_root: PathBuf,
    ) -> Self {
        Agent {
            model,
            registry: Registry::new(),
            cx: Context::new(repo_root),
            messages: vec![Message::system(system.into())],
            max_steps,
        }
    }

    /// Register one tool (builder style).
    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        self.registry.register(tool);
        self
    }

    /// Register the read-only built-in tool set.
    pub fn register_builtins(mut self) -> Self {
        for tool in builtin_tools() {
            self.registry.register(tool);
        }
        self
    }

    /// Run one task to completion (or a limit/error): call the model with the
    /// available tool schemas, execute any tool calls, feed results back, and
    /// finish when the model stops requesting tools.
    pub fn run(&mut self, task: &str) -> Outcome {
        self.messages.push(Message::user(task));
        let schemas = self.registry.schemas();
        let mut step = 0;
        loop {
            if step >= self.max_steps {
                return Outcome::StepLimit;
            }
            let msg = match self.model.complete(&self.messages, &schemas) {
                Ok(m) => m,
                Err(e) => return Outcome::Error(e),
            };
            self.messages
                .push(Message::assistant_with_calls(msg.content.clone(), msg.tool_calls.clone()));
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                let out = self.registry.dispatch(call, &mut self.cx);
                eprintln!("● {}  {}", call.name, out.summary);
                self.messages.push(Message::tool_result(&call.id, out.content));
            }
            step += 1;
        }
    }
}
```

- [ ] **Step 5: Re-export tool structs + `builtin_tools`** — update `quecto-agent/src/lib.rs`

```rust
pub use tools::fs::{ListFiles, ReadFile};
pub use tools::git::{GitDiff, GitStatus};
pub use tools::search::SearchText;
pub use tools::{builtin_tools, cap_output, Context, Registry, Tool, ToolError, ToolOutput, ToolResult};
```

- [ ] **Step 6: Register built-ins in the CLI** — update `quecto-agent/src/main.rs`

Replace the model/agent construction:
```rust
    let repo_root = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let model = HttpModel::from_env();
    let mut agent = Agent::new(Box::new(model), system, max_steps, repo_root).register_builtins();
```

- [ ] **Step 7: Run tests + verify**

Run: `cargo test -p quecto-agent && cargo clippy -p quecto-agent --all-targets`
Expected: all tests pass (Task-1 model tests, Task-2 registry tests, Task-3 fs tests, Task-4 search/git tests, Task-5 loop tests, model + cli integration), no clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add quecto-agent/src/agent.rs quecto-agent/src/tools/mod.rs quecto-agent/src/lib.rs quecto-agent/src/main.rs
git commit -m "feat(agent): wire read-only tools into the agent loop + register built-ins"
```

---

## Self-Review

**Spec coverage** (against `2026-07-10-quecto-agent-architecture.md`, scoped to M2):

- Tool system — `Tool` trait (name/description/schema/run), `ToolResult = Result<ToolOutput, ToolError>` (error reported to the model, not fatal), `Context`, `Registry` (schemas/dispatch, dispatch never panics): T2. ✅
- Path safety shared by fs tools — resolve against repo root, reject `..`/symlink escapes via canonicalization: `Context::resolve_existing` (T2), used by every fs tool (T3–T4). ✅ (The not-yet-existing-file parent-dir variant is added in M3 with `write_file`/`apply_patch`.)
- The essential read-only tools — `read_file` (range + output cap), `list_files` (gitignore via `ignore`), `search_text` (regex, gitignore), `git_diff`, `git_status` (read-only, no-git degradation): T3–T4. ✅
- Loop executes tool calls, appends results, per-tool output truncation, activity line per tool: T5 + `cap_output`. ✅
- Buffered `quecto_raw`, native tool protocol, tools transported in the body: T1 (`Model::complete` + `HttpModel`). ✅
- No generic unrestricted filesystem tool: only the five scoped tools. ✅

**Deliberately deferred (later milestones):** `write_file`/`apply_patch` + change tracking (M3); `run_command` + sandbox + approval policy + denylist + interactivity + `ctrlc` cancel + the repeated-action guard (M4); verify gate + instruction loader + context seed (M5); SQLite session + resume/undo + chat/slash-commands + the rich `crossterm` renderer (M6); flavors + `[tools]` allow-list + `text` protocol + `ask_user` (M7). None are in M2.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to Task N"; every code step contains complete code. ✅

**Type consistency:** `Model::complete(&self, &[Message], &[Value]) -> Result<AssistantMessage, BoxErr>` identical at the trait, `HttpModel`, and the `Scripted` fake; `Tool::run(&self, &Value, &mut Context) -> ToolResult` identical across all five tools + test tools; `Context::resolve_existing`/`repo_root` used consistently by fs/search tools; `Registry::{register,schemas,dispatch}` and `dispatch(&ToolCall,&mut Context)->ToolOutput` match call sites; `Agent::new(Box<dyn Model>, impl Into<String>, usize, PathBuf)` matches every construction (main + tests); `builtin_tools() -> Vec<Box<dyn Tool>>` matches `register_builtins`. ✅

**Scope decisions flagged** (see Global Constraints → Milestone simplifications): (1) `search_text` uses `ignore`+`regex`, not the ripgrep `grep-*` libraries — same behavior, simpler; (2) git tools shell out to `git` (fixed read-only args) rather than routing through the not-yet-built M4 sandbox; (3) activity output is a minimal inline stderr line, not the M6 renderer.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-10-quecto-agent-m2-tools.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach? (Or hand it to Codex as you did M1 — I'll verify.)**
