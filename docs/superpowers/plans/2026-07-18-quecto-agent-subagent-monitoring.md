# Concurrent Subagent Spawning + Monitoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `spawn_subagent`, `monitor_subagents`, and `cancel_subagent` tools so the model can run more than one subagent concurrently and check on them, without changing the existing synchronous `invoke_subagent` tool.

**Architecture:** A `SubagentPool` (two cheap `Arc`s: an id counter and a `Mutex<HashMap<u32, SubagentInfo>>`) is created once per top-level `Agent` and shared (via `.clone()`) across the three new tools. `spawn_subagent` starts a subagent on a plain `std::thread`, attaches a `ProgressRecorder` (implements the existing `RunRecorder` trait) that streams tool-call/result summaries into a shared ring buffer, and gives the subagent its own `CancelToken` cascaded from the parent's via a small watcher thread. `monitor_subagents` and `cancel_subagent` read/mutate pool state through the same shared `Arc`s. Tasks are ordered so each one compiles and its own tests pass standalone: pool → recorder → monitor → cancel → spawn (the only tool that references the other two, for self-registration inside nested subagents) → wiring.

**Tech Stack:** Rust, `std::thread`/`std::sync` only (no new dependencies — `Model` is already `Send + Sync`, so no async runtime is needed).

## Global Constraints

- `invoke_subagent`'s existing behavior, signature, and tests are untouched.
- No new crate dependencies.
- Subagents run as threads in the same process (no cross-process/distributed execution).
- Concurrent subagents capped at `MAX_CONCURRENT_SUBAGENTS = 8`; `spawn_subagent` returns an error `ToolOutput` past the cap instead of spawning.
- `monitor_subagents` is read-only and joins `invoke_subagent` in `policy.rs`'s always-allow list. `spawn_subagent` and `cancel_subagent` route through `Policy.run` (`Decision::Ask` under the default `read-only` preset), exactly like `start_background_process`/`kill_background_process` today.
- Spec: `docs/superpowers/specs/2026-07-18-quecto-agent-subagent-monitoring-design.md`

---

### Task 1: `SubagentPool` core data structures

**Files:**
- Modify: `quecto-agent/src/tools/subagent.rs` (append to existing file, which currently ends at line 148 with `InvokeSubagent`'s closing brace)
- Test: same file, `#[cfg(test)] mod tests` block appended at the end (there is no existing test module in this file today)

**Interfaces:**
- Produces: `pub const MAX_CONCURRENT_SUBAGENTS: usize = 8;`, `#[derive(Clone, Debug, PartialEq)] pub enum RunStatus { Running, Complete(String), Cancelled, Failed(String) }`, `#[derive(Clone, Debug)] pub struct SubagentSnapshot { pub id: u32, pub role: String, pub prompt: String, pub status: RunStatus, pub elapsed: Duration, pub progress: Vec<String> }`, `#[derive(Clone)] pub struct SubagentPool { .. }` with methods `new() -> Self`, `allocate(&self, role: String, prompt: String, cancel: CancelToken) -> (u32, Arc<Mutex<Vec<String>>>, Arc<Mutex<RunStatus>>)`, `running_count(&self) -> usize`, `set_status(&self, id: u32, status: RunStatus)`, `cancel(&self, id: u32) -> Option<bool>`, `get(&self, id: u32) -> Option<SubagentSnapshot>`, `all(&self) -> Vec<SubagentSnapshot>`. Also a free function `push_progress(buf: &Arc<Mutex<Vec<String>>>, line: String)` capping the buffer at `PROGRESS_CAP = 50` lines.
- Consumes: `crate::sandbox::CancelToken` (`= Arc<AtomicBool>`, already `pub` at `sandbox.rs:17`).

- [ ] **Step 1: Write the failing tests**

Append to `quecto-agent/src/tools/subagent.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn token() -> CancelToken {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn allocate_assigns_increasing_ids_and_starts_running() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "task one".into(), token());
        let (id2, ..) = pool.allocate("Subagent".into(), "task two".into(), token());
        assert!(id2 > id1);
        let snap = pool.get(id1).unwrap();
        assert_eq!(snap.status, RunStatus::Running);
        assert_eq!(snap.prompt, "task one");
    }

    #[test]
    fn running_count_only_counts_running() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (_id2, ..) = pool.allocate("Subagent".into(), "b".into(), token());
        assert_eq!(pool.running_count(), 2);
        pool.set_status(id1, RunStatus::Complete("done".into()));
        assert_eq!(pool.running_count(), 1);
    }

    #[test]
    fn cancel_unknown_id_returns_none() {
        let pool = SubagentPool::new();
        assert_eq!(pool.cancel(999), None);
    }

    #[test]
    fn cancel_running_flips_token_and_returns_some_true() {
        let pool = SubagentPool::new();
        let t = token();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), t.clone());
        assert_eq!(pool.cancel(id), Some(true));
        assert!(t.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_finished_returns_some_false_without_flipping_token() {
        let pool = SubagentPool::new();
        let t = token();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), t.clone());
        pool.set_status(id, RunStatus::Complete("done".into()));
        assert_eq!(pool.cancel(id), Some(false));
        assert!(!t.load(Ordering::SeqCst));
    }

    #[test]
    fn all_lists_every_spawned_subagent_newest_first() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (id2, ..) = pool.allocate("Reviewer".into(), "b".into(), token());
        let all = pool.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, id2);
        assert_eq!(all[1].id, id1);
    }

    #[test]
    fn push_progress_caps_at_50_lines() {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        for i in 0..60 {
            push_progress(&buf, format!("line {i}"));
        }
        let locked = buf.lock().unwrap();
        assert_eq!(locked.len(), 50);
        assert_eq!(locked[0], "line 10");
        assert_eq!(locked[49], "line 59");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -30`
Expected: compile errors like `cannot find type \`SubagentPool\` in this scope`, `cannot find function \`push_progress\``.

- [ ] **Step 3: Implement `SubagentPool` and friends**

Extend the `use` list at the top of `quecto-agent/src/tools/subagent.rs` (keep the existing `use crate::agent::{Agent, AgentConfig, Outcome};` and `use crate::model::Message;` as-is):

```rust
use crate::sandbox::CancelToken;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
```

Then add, before the existing `#[derive(Clone)] pub struct InvokeSubagent`:

```rust
pub const MAX_CONCURRENT_SUBAGENTS: usize = 8;
const PROGRESS_CAP: usize = 50;

fn push_progress(buf: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut v = buf.lock().unwrap();
    v.push(line);
    let len = v.len();
    if len > PROGRESS_CAP {
        v.drain(0..len - PROGRESS_CAP);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunStatus {
    Running,
    Complete(String),
    Cancelled,
    Failed(String),
}

struct SubagentInfo {
    id: u32,
    role: String,
    prompt: String,
    started: Instant,
    cancel: CancelToken,
    progress: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<RunStatus>>,
}

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: u32,
    pub role: String,
    pub prompt: String,
    pub status: RunStatus,
    pub elapsed: Duration,
    pub progress: Vec<String>,
}

impl From<&SubagentInfo> for SubagentSnapshot {
    fn from(info: &SubagentInfo) -> Self {
        SubagentSnapshot {
            id: info.id,
            role: info.role.clone(),
            prompt: info.prompt.clone(),
            status: info.status.lock().unwrap().clone(),
            elapsed: info.started.elapsed(),
            progress: info.progress.lock().unwrap().clone(),
        }
    }
}

#[derive(Clone)]
pub struct SubagentPool {
    next_id: Arc<AtomicU32>,
    handles: Arc<Mutex<HashMap<u32, SubagentInfo>>>,
}

impl SubagentPool {
    pub fn new() -> Self {
        SubagentPool {
            next_id: Arc::new(AtomicU32::new(1)),
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn allocate(
        &self,
        role: String,
        prompt: String,
        cancel: CancelToken,
    ) -> (u32, Arc<Mutex<Vec<String>>>, Arc<Mutex<RunStatus>>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let progress = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(RunStatus::Running));
        let info = SubagentInfo {
            id,
            role,
            prompt,
            started: Instant::now(),
            cancel,
            progress: progress.clone(),
            status: status.clone(),
        };
        self.handles.lock().unwrap().insert(id, info);
        (id, progress, status)
    }

    pub fn running_count(&self) -> usize {
        self.handles
            .lock()
            .unwrap()
            .values()
            .filter(|i| matches!(*i.status.lock().unwrap(), RunStatus::Running))
            .count()
    }

    pub fn set_status(&self, id: u32, status: RunStatus) {
        if let Some(info) = self.handles.lock().unwrap().get(&id) {
            *info.status.lock().unwrap() = status;
        }
    }

    /// `Some(true)` if a running subagent was signalled to stop, `Some(false)`
    /// if it had already finished, `None` if `id` is unknown.
    pub fn cancel(&self, id: u32) -> Option<bool> {
        let handles = self.handles.lock().unwrap();
        let info = handles.get(&id)?;
        let running = matches!(*info.status.lock().unwrap(), RunStatus::Running);
        if running {
            info.cancel.store(true, Ordering::SeqCst);
        }
        Some(running)
    }

    pub fn get(&self, id: u32) -> Option<SubagentSnapshot> {
        self.handles.lock().unwrap().get(&id).map(SubagentSnapshot::from)
    }

    pub fn all(&self) -> Vec<SubagentSnapshot> {
        let handles = self.handles.lock().unwrap();
        let mut v: Vec<SubagentSnapshot> = handles.values().map(SubagentSnapshot::from).collect();
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -30`
Expected: `test result: ok. 7 passed; 0 failed`

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/tools/subagent.rs
git commit -m "feat: add SubagentPool for tracking concurrently-spawned subagents"
```

---

### Task 2: `ProgressRecorder`

**Files:**
- Modify: `quecto-agent/src/tools/subagent.rs`

**Interfaces:**
- Consumes: `pub trait RunRecorder { fn message(&mut self, m: &Message); fn message_with_metadata(&mut self, m: &Message, _metadata: &MessageMetadata) { self.message(m); } fn change(&mut self, c: &FileChange); }` (`agent.rs:41-49`), `push_progress` from Task 1.
- Produces: `struct ProgressRecorder { buf: Arc<Mutex<Vec<String>>> }` implementing `RunRecorder`.

- [ ] **Step 1: Confirm `Message` constructor shapes before writing the test**

Run: `grep -n "pub fn tool(\|pub fn assistant_with_calls(" quecto-agent/src/model.rs`
Expected: both found — they're already used by `agent.rs`'s own run loop and tests (e.g. `Message::assistant_with_calls(msg.content.clone(), msg.tool_calls.clone())` at `agent.rs:358`). Note the exact parameter order/types shown so the test below matches; if `Message::tool` takes arguments in a different order than `(id, content)`, adjust the test call accordingly rather than guessing.

- [ ] **Step 2: Write the failing test**

Add to the `mod tests` block in `quecto-agent/src/tools/subagent.rs`:

```rust
    #[test]
    fn progress_recorder_logs_tool_calls_and_results() {
        use crate::agent::RunRecorder;
        use crate::model::ToolCall;

        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut rec = ProgressRecorder { buf: buf.clone() };

        let mut assistant = Message::assistant_with_calls(
            String::new(),
            vec![ToolCall {
                id: "1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }],
        );
        assistant.role = "assistant".into();
        rec.message(&assistant);

        let tool_result = Message::tool("1", "42 lines");
        rec.message(&tool_result);

        let locked = buf.lock().unwrap();
        assert_eq!(locked.len(), 2);
        assert!(locked[0].contains("read_file"));
        assert!(locked[1].contains("42 lines"));
    }

    #[test]
    fn progress_recorder_caps_at_50_entries() {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut rec = ProgressRecorder { buf: buf.clone() };
        for i in 0..60 {
            let m = Message::tool("1", &format!("result {i}"));
            rec.message(&m);
        }
        assert_eq!(buf.lock().unwrap().len(), 50);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -30`
Expected: `cannot find struct \`ProgressRecorder\`` compile error.

- [ ] **Step 4: Implement `ProgressRecorder`**

Add to `quecto-agent/src/tools/subagent.rs` (near the top, with the other `use` additions):

```rust
use crate::agent::RunRecorder;
use crate::tools::FileChange;

struct ProgressRecorder {
    buf: Arc<Mutex<Vec<String>>>,
}

impl RunRecorder for ProgressRecorder {
    fn message(&mut self, m: &Message) {
        match m.role.as_str() {
            "assistant" => {
                for call in &m.tool_calls {
                    push_progress(&self.buf, format!("called {}({})", call.name, call.arguments));
                }
                if m.tool_calls.is_empty() && !m.content.is_empty() {
                    let snippet: String = m.content.chars().take(160).collect();
                    push_progress(&self.buf, format!("said: {snippet}"));
                }
            }
            "tool" => {
                let snippet: String = m.content.chars().take(160).collect();
                push_progress(&self.buf, format!("-> {snippet}"));
            }
            _ => {}
        }
    }

    fn change(&mut self, _c: &FileChange) {}
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -30`
Expected: `test result: ok. 9 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/tools/subagent.rs
git commit -m "feat: add ProgressRecorder to stream subagent activity into SubagentPool"
```

---

### Task 3: `monitor_subagents` tool

**Files:**
- Modify: `quecto-agent/src/tools/subagent.rs`

**Interfaces:**
- Consumes: `SubagentPool::get`/`all` (Task 1), `SubagentSnapshot`, `RunStatus` (Task 1).
- Produces: `pub struct MonitorSubagents { pub pool: SubagentPool }` with `pub fn new(pool: SubagentPool) -> Self`, tool name `"monitor_subagents"`. Task 5 (`spawn_subagent`) registers this inside every subagent it spawns via `MonitorSubagents::new`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `quecto-agent/src/tools/subagent.rs`:

```rust
    #[test]
    fn monitor_reports_single_subagent_by_id() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Reviewer".into(), "look for bugs".into(), token());
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("running"));
        assert!(out.content.contains("Reviewer"));
        assert!(out.content.contains("look for bugs"));
    }

    #[test]
    fn monitor_reports_complete_result() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Subagent".into(), "count files".into(), token());
        pool.set_status(id, RunStatus::Complete("42 files".to_string()));
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("complete"));
        assert!(out.content.contains("42 files"));
    }

    #[test]
    fn monitor_unknown_id_is_an_error() {
        let pool = SubagentPool::new();
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        assert!(tool.run(&json!({"id": 999}), &mut cx).is_err());
    }

    #[test]
    fn monitor_without_id_lists_all_newest_first() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (id2, ..) = pool.allocate("Reviewer".into(), "b".into(), token());
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({}), &mut cx).unwrap();
        let id1_pos = out.content.find(&format!("#{id1}")).unwrap();
        let id2_pos = out.content.find(&format!("#{id2}")).unwrap();
        assert!(id2_pos < id1_pos, "newest (#{id2}) should be listed first");
    }

    #[test]
    fn monitor_without_id_and_no_subagents_says_so() {
        let pool = SubagentPool::new();
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({}), &mut cx).unwrap();
        assert!(out.content.contains("no subagents"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -40`
Expected: `cannot find struct \`MonitorSubagents\`` compile error.

- [ ] **Step 3: Implement `MonitorSubagents`**

Add to `quecto-agent/src/tools/subagent.rs`:

```rust
fn status_label(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Complete(_) => "complete",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Failed(_) => "failed",
    }
}

fn render_summary_line(snap: &SubagentSnapshot) -> String {
    format!(
        "#{} [{}] role={} elapsed={:.1}s",
        snap.id,
        status_label(&snap.status),
        snap.role,
        snap.elapsed.as_secs_f64()
    )
}

fn render_snapshot(snap: &SubagentSnapshot) -> String {
    let mut out = render_summary_line(snap);
    out.push_str("\nprompt: ");
    out.push_str(&snap.prompt);
    if !snap.progress.is_empty() {
        out.push_str("\nrecent activity:\n");
        out.push_str(&snap.progress.join("\n"));
    }
    match &snap.status {
        RunStatus::Complete(text) => {
            out.push_str("\nresult:\n");
            out.push_str(text);
        }
        RunStatus::Failed(msg) => {
            out.push_str("\nfailure reason: ");
            out.push_str(msg);
        }
        _ => {}
    }
    out
}

#[derive(Clone)]
pub struct MonitorSubagents {
    pub pool: SubagentPool,
}

impl MonitorSubagents {
    pub fn new(pool: SubagentPool) -> Self {
        MonitorSubagents { pool }
    }
}

impl Tool for MonitorSubagents {
    fn name(&self) -> &str {
        "monitor_subagents"
    }

    fn description(&self) -> &str {
        "Reports status, elapsed time, and recent activity for subagents started with \
spawn_subagent. Pass an id to check one; omit it to list all spawned this session."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The id returned by spawn_subagent. Omit to list all spawned subagents."
                }
            },
            "required": []
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let id = args.get("id").and_then(|v| v.as_u64()).map(|v| v as u32);
        match id {
            Some(id) => {
                let snap = self
                    .pool
                    .get(id)
                    .ok_or_else(|| ToolError::new(format!("no subagent with id {id}")))?;
                Ok(ToolOutput::new(render_snapshot(&snap), "subagent status"))
            }
            None => {
                let all = self.pool.all();
                if all.is_empty() {
                    return Ok(ToolOutput::new("no subagents have been spawned yet", "no subagents"));
                }
                let lines: Vec<String> = all.iter().map(render_summary_line).collect();
                Ok(ToolOutput::new(lines.join("\n"), "subagent list"))
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -40`
Expected: `test result: ok. 14 passed; 0 failed`

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/tools/subagent.rs
git commit -m "feat: add monitor_subagents tool"
```

---

### Task 4: `cancel_subagent` tool

**Files:**
- Modify: `quecto-agent/src/tools/subagent.rs`

**Interfaces:**
- Consumes: `SubagentPool::cancel` (Task 1), `AgentConfig` (`agent.rs:90-98`), `Agent::new`/`register`/`with_recorder`/`run` (`agent.rs`), `ProgressRecorder` (Task 2).
- Produces: `pub struct CancelSubagent { pub pool: SubagentPool }` with `pub fn new(pool: SubagentPool) -> Self`, tool name `"cancel_subagent"`. Also introduces test helpers `test_config`, `wait_until_finished`, `ImmediateReply`, `AlwaysWantsTool`, `SlowCounter` in the `mod tests` block, reused by Task 5's tests.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `quecto-agent/src/tools/subagent.rs`. This introduces the shared test scaffolding (a couple of fake `Model`s and a slow test-only `Tool`) that Task 5 will reuse:

```rust
    #[derive(Clone)]
    struct ImmediateReply {
        text: &'static str,
    }
    impl crate::model::Model for ImmediateReply {
        fn clone_box(&self) -> Box<dyn crate::model::Model> {
            Box::new(self.clone())
        }
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<crate::model::AssistantMessage, crate::BoxErr> {
            Ok(crate::model::AssistantMessage {
                content: self.text.to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: None,
            })
        }
        fn complete_with_options(
            &self,
            messages: &[Message],
            tools: &[Value],
            _options: &crate::reasoning::CompletionOptions,
        ) -> Result<crate::model::ModelCompletion, crate::BoxErr> {
            self.complete(messages, tools).map(crate::model::ModelCompletion::from)
        }
    }

    #[derive(Clone)]
    struct AlwaysWantsTool {
        replies_left: Arc<AtomicU32>,
    }
    impl crate::model::Model for AlwaysWantsTool {
        fn clone_box(&self) -> Box<dyn crate::model::Model> {
            Box::new(self.clone())
        }
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<crate::model::AssistantMessage, crate::BoxErr> {
            let n = self.replies_left.fetch_sub(1, Ordering::SeqCst);
            if n == 0 {
                return Ok(crate::model::AssistantMessage {
                    content: "gave up".to_string(),
                    tool_calls: vec![],
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                });
            }
            Ok(crate::model::AssistantMessage {
                content: String::new(),
                tool_calls: vec![crate::model::ToolCall {
                    id: n.to_string(),
                    name: "slow_counter".to_string(),
                    arguments: json!({}),
                }],
                finish_reason: "tool_calls".to_string(),
                reasoning_content: None,
            })
        }
        fn complete_with_options(
            &self,
            messages: &[Message],
            tools: &[Value],
            _options: &crate::reasoning::CompletionOptions,
        ) -> Result<crate::model::ModelCompletion, crate::BoxErr> {
            self.complete(messages, tools).map(crate::model::ModelCompletion::from)
        }
    }

    struct SlowCounter {
        count: Arc<AtomicU32>,
    }
    impl Tool for SlowCounter {
        fn name(&self) -> &str {
            "slow_counter"
        }
        fn description(&self) -> &str {
            "test-only tool that sleeps briefly and returns a changing value"
        }
        fn schema(&self) -> Value {
            json!({"type": "object", "properties": {}, "required": []})
        }
        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            std::thread::sleep(Duration::from_millis(30));
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::new(format!("tick {n}"), "tick"))
        }
    }

    fn test_config(model: impl crate::model::Model + 'static) -> AgentConfig {
        AgentConfig {
            model: Box::new(model),
            base_system_prompt: "you are a test agent".to_string(),
            max_steps: 30,
            repo_root: std::env::current_dir().unwrap(),
            cancel: token(),
            approval: crate::approval::ApprovalMode::AutoApprove,
        }
    }

    fn wait_until_finished(pool: &SubagentPool, id: u32) -> SubagentSnapshot {
        for _ in 0..200 {
            let snap = pool.get(id).unwrap();
            if snap.status != RunStatus::Running {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("subagent #{id} did not finish within 2s");
    }

    #[test]
    fn cancel_subagent_stops_a_running_subagent() {
        let model = AlwaysWantsTool {
            replies_left: Arc::new(AtomicU32::new(30)),
        };
        let config = test_config(model);
        let pool = SubagentPool::new();

        let child_cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let (id, progress, _status) =
            pool.allocate("Subagent".into(), "count forever".into(), child_cancel.clone());
        let pool_for_thread = pool.clone();
        thread::spawn(move || {
            let mut subagent = Agent::new(
                config.model.clone(),
                config.base_system_prompt.clone(),
                config.max_steps,
                config.repo_root.clone(),
                child_cancel,
                config.approval.clone(),
            )
            .register(Box::new(SlowCounter {
                count: Arc::new(AtomicU32::new(0)),
            }))
            .with_recorder(Box::new(ProgressRecorder { buf: progress }));
            let outcome = subagent.run("count forever");
            let status = match outcome {
                Outcome::Cancelled => RunStatus::Cancelled,
                Outcome::Complete(t) => RunStatus::Complete(t),
                _ => RunStatus::Failed("unexpected outcome".to_string()),
            };
            pool_for_thread.set_status(id, status);
        });

        // Give the thread a moment to start looping before cancelling it.
        std::thread::sleep(Duration::from_millis(150));
        let tool = CancelSubagent::new(pool.clone());
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("cancel requested"));

        let snap = wait_until_finished(&pool, id);
        assert_eq!(snap.status, RunStatus::Cancelled);
    }

    #[test]
    fn cancel_subagent_unknown_id_is_an_error() {
        let pool = SubagentPool::new();
        let tool = CancelSubagent::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        assert!(tool.run(&json!({"id": 999}), &mut cx).is_err());
    }

    #[test]
    fn cancel_subagent_already_finished_says_so() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        pool.set_status(id, RunStatus::Complete("done".into()));
        let tool = CancelSubagent::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("already finished"));
    }
```

This test uses `AtomicU32`, `AtomicBool`, and `thread` directly — add these to the top-level `use` list from Task 1/2 if not already present: `use std::sync::atomic::AtomicU32;` (alongside the existing `AtomicU32` import for `next_id` — reuse it), `use std::sync::atomic::AtomicBool;`, `use std::thread;`. Since `token()` in the test module already uses `std::sync::atomic::AtomicBool` via a local `use` inside `mod tests` (Task 1, Step 1), add `use std::thread;` at the top of `mod tests` (next to `use super::*;`) rather than duplicating `AtomicBool`'s import.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -40`
Expected: `cannot find struct \`CancelSubagent\`` compile error.

- [ ] **Step 3: Implement `CancelSubagent`**

Add to `quecto-agent/src/tools/subagent.rs`:

```rust
#[derive(Clone)]
pub struct CancelSubagent {
    pub pool: SubagentPool,
}

impl CancelSubagent {
    pub fn new(pool: SubagentPool) -> Self {
        CancelSubagent { pool }
    }
}

impl Tool for CancelSubagent {
    fn name(&self) -> &str {
        "cancel_subagent"
    }

    fn description(&self) -> &str {
        "Stops a subagent started with spawn_subagent before it finishes."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The id returned by spawn_subagent."
                }
            },
            "required": ["id"]
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let id = args
            .get("id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .ok_or_else(|| ToolError::new("missing \"id\" parameter"))?;
        match self.pool.cancel(id) {
            Some(true) => Ok(ToolOutput::new(
                format!("cancel requested for subagent #{id}"),
                "cancel requested",
            )),
            Some(false) => Ok(ToolOutput::new(
                format!("subagent #{id} already finished"),
                "already finished",
            )),
            None => Err(ToolError::new(format!("no subagent with id {id}"))),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -60`
Expected: `test result: ok. 17 passed; 0 failed`. If `cancel_subagent_stops_a_running_subagent` is flaky (the 150ms startup sleep races the spawned thread on a loaded machine), increase it to `Duration::from_millis(300)` and re-run — do not delete or weaken the assertion.

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/tools/subagent.rs
git commit -m "feat: add cancel_subagent tool"
```

---

### Task 5: `spawn_subagent` tool

**Files:**
- Modify: `quecto-agent/src/tools/subagent.rs`
- Modify: `quecto-agent/src/render.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-4 (`SubagentPool`, `ProgressRecorder`, `MonitorSubagents::new`, `CancelSubagent::new`), plus `AgentConfig`, `Agent::new`/`register_builtins`/`register`/`with_recorder`/`with_renderer`/`run`, `SUBAGENT_DIRECTIVE` (already defined at the top of `subagent.rs`).
- Produces: `pub struct SpawnSubagent { pub config: AgentConfig, pub pool: SubagentPool }` with `pub fn new(config: AgentConfig, pool: SubagentPool) -> Self`, tool name `"spawn_subagent"`. Also `render::NullRenderer`, a silent `Renderer` used for background subagents.

Concurrent subagents would otherwise use the default `stderr_renderer()`, interleaving several subagents' activity lines on one stream at once. `NullRenderer` avoids that — progress is inspected through `monitor_subagents` instead.

- [ ] **Step 1: Add `NullRenderer` to `render.rs`**

Read the `Renderer` trait first (`quecto-agent/src/render.rs:139-146`) to confirm which methods have no default (`tool`, `verify`, `notice`, `assistant`; `working`/`working_done` do). Then add, right after the trait definition:

```rust
/// Discards all activity. Used for subagents running on a background thread,
/// where interleaving raw stderr output from several concurrent runs would
/// be unreadable — their progress is inspected via `monitor_subagents` instead.
pub struct NullRenderer;

impl Renderer for NullRenderer {
    fn tool(&mut self, _name: &str, _summary: &str) {}
    fn verify(&mut self, _command: &str, _passed: bool) {}
    fn notice(&mut self, _text: &str) {}
    fn assistant(&mut self, _text: &str) {}
}
```

- [ ] **Step 2: Write the failing tests**

Add to the `mod tests` block in `quecto-agent/src/tools/subagent.rs` (reuses `ImmediateReply`, `test_config`, `wait_until_finished` from Task 4):

```rust
    #[test]
    fn spawn_subagent_completes_and_reports_result() {
        let config = test_config(ImmediateReply { text: "42 files" });
        let pool = SubagentPool::new();
        let tool = SpawnSubagent::new(config, pool.clone());
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());

        let out = tool
            .run(&json!({"prompt": "count files"}), &mut cx)
            .unwrap();
        assert!(out.content.contains("spawned subagent #"));

        let id: u32 = out
            .content
            .rsplit('#')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let snap = wait_until_finished(&pool, id);
        assert_eq!(snap.status, RunStatus::Complete("42 files".to_string()));
    }

    #[test]
    fn spawn_subagent_rejects_past_the_concurrency_cap() {
        let config = test_config(ImmediateReply { text: "done" });
        let pool = SubagentPool::new();
        for _ in 0..MAX_CONCURRENT_SUBAGENTS {
            pool.allocate("Subagent".into(), "busy".into(), token());
        }
        let tool = SpawnSubagent::new(config, pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let err = tool.run(&json!({"prompt": "one more"}), &mut cx).unwrap_err();
        assert!(err.message.contains("already running"));
    }

    #[test]
    fn spawn_subagent_runs_two_concurrently() {
        let pool = SubagentPool::new();
        let config_a = test_config(ImmediateReply { text: "result a" });
        let config_b = test_config(ImmediateReply { text: "result b" });
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());

        let tool_a = SpawnSubagent::new(config_a, pool.clone());
        let out_a = tool_a.run(&json!({"prompt": "task a"}), &mut cx).unwrap();
        let tool_b = SpawnSubagent::new(config_b, pool.clone());
        let out_b = tool_b.run(&json!({"prompt": "task b"}), &mut cx).unwrap();

        let id_a: u32 = out_a.content.rsplit('#').next().unwrap().trim().parse().unwrap();
        let id_b: u32 = out_b.content.rsplit('#').next().unwrap().trim().parse().unwrap();
        assert_ne!(id_a, id_b);

        let snap_a = wait_until_finished(&pool, id_a);
        let snap_b = wait_until_finished(&pool, id_b);
        assert_eq!(snap_a.status, RunStatus::Complete("result a".to_string()));
        assert_eq!(snap_b.status, RunStatus::Complete("result b".to_string()));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -40`
Expected: `cannot find struct \`SpawnSubagent\`` compile error.

- [ ] **Step 4: Implement `SpawnSubagent`**

Add to the top-level `use` list: `use std::panic::AssertUnwindSafe;` (alongside `use std::thread;`, already added in Task 4).

Add to `quecto-agent/src/tools/subagent.rs`:

```rust
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[derive(Clone)]
pub struct SpawnSubagent {
    pub config: AgentConfig,
    pub pool: SubagentPool,
}

impl SpawnSubagent {
    pub fn new(config: AgentConfig, pool: SubagentPool) -> Self {
        SpawnSubagent { config, pool }
    }
}

impl Tool for SpawnSubagent {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Starts a subagent in the background and returns immediately with an id. \
Use this instead of invoke_subagent when you want more than one subagent working \
at once. Use monitor_subagents to check progress/results and cancel_subagent to \
stop one early."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The instruction or task description for the subagent to complete."
                },
                "role": {
                    "type": "string",
                    "description": "Optional specific role or persona for the subagent (e.g., Debugger, Researcher)."
                }
            },
            "required": ["prompt"]
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("missing \"prompt\" parameter"))?
            .to_string();
        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("Subagent")
            .to_string();

        if self.pool.running_count() >= MAX_CONCURRENT_SUBAGENTS {
            return Err(ToolError::new(format!(
                "cannot spawn: {MAX_CONCURRENT_SUBAGENTS} subagents are already running; \
cancel one with cancel_subagent or wait for one to finish first"
            )));
        }

        let child_cancel: CancelToken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (id, progress, _status) =
            self.pool.allocate(role.clone(), prompt.clone(), child_cancel.clone());

        {
            let parent_cancel = self.config.cancel.clone();
            let watch_cancel = child_cancel.clone();
            thread::spawn(move || {
                while !parent_cancel.load(Ordering::SeqCst) && !watch_cancel.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(200));
                }
                watch_cancel.store(true, Ordering::SeqCst);
            });
        }

        let mut system_prompt = format!("{}\n\n{}", SUBAGENT_DIRECTIVE, self.config.base_system_prompt);
        if role != "Subagent" {
            system_prompt.push_str(&format!("\n\nYou are acting as a specialized subagent: {}", role));
        }

        let model = self.config.model.clone();
        let max_steps = self.config.max_steps;
        let repo_root = self.config.repo_root.clone();
        let approval = self.config.approval.clone();
        let pool = self.pool.clone();
        let config = self.config.clone();

        thread::spawn(move || {
            let mut subagent = Agent::new(model, system_prompt, max_steps, repo_root, child_cancel, approval)
                .register_builtins()
                .with_recorder(Box::new(ProgressRecorder { buf: progress }))
                .with_renderer(Box::new(crate::render::NullRenderer));
            subagent = subagent.register(Box::new(InvokeSubagent::new(config.clone())));
            subagent = subagent.register(Box::new(SpawnSubagent::new(config.clone(), pool.clone())));
            subagent = subagent.register(Box::new(MonitorSubagents::new(pool.clone())));
            subagent = subagent.register(Box::new(CancelSubagent::new(pool.clone())));

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| subagent.run(&prompt)));
            let final_status = match result {
                Ok(Outcome::Complete(text)) => RunStatus::Complete(text),
                Ok(Outcome::Cancelled) => RunStatus::Cancelled,
                Ok(Outcome::StepLimit) => {
                    RunStatus::Failed("step limit reached before finishing".to_string())
                }
                Ok(Outcome::RepeatedAction) => {
                    RunStatus::Failed("stuck repeating the same tool call".to_string())
                }
                Ok(Outcome::Blocked) => RunStatus::Failed("blocked by policy or approval".to_string()),
                Ok(Outcome::VerificationFailed { attempts }) => {
                    RunStatus::Failed(format!("verification failed after {attempts} attempts"))
                }
                Ok(Outcome::Error(e)) => RunStatus::Failed(format!("error: {e}")),
                Err(panic) => RunStatus::Failed(format!("panicked: {}", panic_message(&panic))),
            };
            pool.set_status(id, final_status);
        });

        Ok(ToolOutput::new(format!("spawned subagent #{id}"), "subagent spawned"))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib subagent:: 2>&1 | tail -60`
Expected: `test result: ok. 20 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/tools/subagent.rs quecto-agent/src/render.rs
git commit -m "feat: add spawn_subagent tool, completing the async subagent trio"
```

---

### Task 6: Wire the new tools into `Agent` and `Policy`

**Files:**
- Modify: `quecto-agent/src/agent.rs:226-235` (`register_builtins_filtered`)
- Modify: `quecto-agent/src/policy.rs:85-90` (`Policy::decide`)

**Interfaces:**
- Consumes: `crate::tools::subagent::{SubagentPool, SpawnSubagent, MonitorSubagents, CancelSubagent}` (Tasks 1, 3, 4, 5).
- Produces: the three new tools appear in `Agent::tool_names()` by default and respect the existing `tools.enabled` allow-list mechanism, exactly like every other builtin.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` block in `quecto-agent/src/agent.rs` (it already has `use super::*;`, `configured_agent`/`agent` helpers, and `cancel_token()` — see `agent.rs:544-634` for the existing patterns to match):

```rust
    #[test]
    fn register_builtins_includes_new_subagent_tools_by_default() {
        let model = Scripted::new(vec![text("done")]);
        let a = agent(model).register_builtins();
        let names = a.tool_names();
        assert!(names.contains(&"spawn_subagent".to_string()));
        assert!(names.contains(&"monitor_subagents".to_string()));
        assert!(names.contains(&"cancel_subagent".to_string()));
    }

    #[test]
    fn register_builtins_filtered_can_exclude_subagent_tools() {
        let model = Scripted::new(vec![text("done")]);
        let allow: Vec<String> = vec!["read_file".to_string()];
        let a = agent(model).register_builtins_filtered(Some(&allow));
        let names = a.tool_names();
        assert!(!names.contains(&"spawn_subagent".to_string()));
        assert!(!names.contains(&"monitor_subagents".to_string()));
        assert!(!names.contains(&"cancel_subagent".to_string()));
        assert!(!names.contains(&"invoke_subagent".to_string()));
    }
```

Add to `quecto-agent/src/policy.rs`'s existing `mod tests` block (locate it first: `grep -n "mod tests" quecto-agent/src/policy.rs`; it already has a test using `Policy::from_preset` and `with_override` around line 530+):

```rust
    #[test]
    fn monitor_subagents_is_always_allowed() {
        let p = Policy::from_preset(Preset::ReadOnly);
        let call = ToolCall {
            id: "1".into(),
            name: "monitor_subagents".into(),
            arguments: serde_json::json!({}),
        };
        assert_eq!(p.decide(&call), Decision::Allow);
    }

    #[test]
    fn spawn_and_cancel_subagent_follow_the_run_decision() {
        let read_only = Policy::from_preset(Preset::ReadOnly);
        let full = Policy::from_preset(Preset::Full);
        for name in ["spawn_subagent", "cancel_subagent"] {
            let call = ToolCall {
                id: "1".into(),
                name: name.into(),
                arguments: serde_json::json!({}),
            };
            assert_eq!(read_only.decide(&call), Decision::Ask);
            assert_eq!(full.decide(&call), Decision::Allow);
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib agent:: policy:: 2>&1 | tail -60`
Expected: the `agent::tests` assertions fail (tool names missing). For the `policy::tests`, first check the tail of `Policy::decide`'s `match` (`grep -n "fn decide" -A 30 quecto-agent/src/policy.rs`) to see what the fallthrough arm does for unrecognized tool names — the new tests should fail against that current behavior since `monitor_subagents`/`spawn_subagent`/`cancel_subagent` aren't classified yet.

- [ ] **Step 3: Update `register_builtins_filtered` in `agent.rs`**

Replace (`agent.rs:225-235`):

```rust
    /// Register the built-in tools filtered by an allow-list (`None` = all).
    pub fn register_builtins_filtered(mut self, enabled: Option<&[String]>) -> Self {
        for tool in crate::tools::builtin_tools_filtered(enabled) {
            self.registry.register(tool);
        }
        if enabled.map_or(true, |list| list.iter().any(|n| n == "invoke_subagent")) {
            let subagent_tool = crate::tools::subagent::InvokeSubagent::new(self.config());
            self.registry.register(Box::new(subagent_tool));
        }
        self
    }
```

with:

```rust
    /// Register the built-in tools filtered by an allow-list (`None` = all).
    pub fn register_builtins_filtered(mut self, enabled: Option<&[String]>) -> Self {
        for tool in crate::tools::builtin_tools_filtered(enabled) {
            self.registry.register(tool);
        }
        let allow = |name: &str| enabled.map_or(true, |list| list.iter().any(|n| n == name));
        if allow("invoke_subagent") {
            let subagent_tool = crate::tools::subagent::InvokeSubagent::new(self.config());
            self.registry.register(Box::new(subagent_tool));
        }
        let pool = crate::tools::subagent::SubagentPool::new();
        if allow("spawn_subagent") {
            self.registry.register(Box::new(crate::tools::subagent::SpawnSubagent::new(
                self.config(),
                pool.clone(),
            )));
        }
        if allow("monitor_subagents") {
            self.registry
                .register(Box::new(crate::tools::subagent::MonitorSubagents::new(pool.clone())));
        }
        if allow("cancel_subagent") {
            self.registry
                .register(Box::new(crate::tools::subagent::CancelSubagent::new(pool)));
        }
        self
    }
```

- [ ] **Step 4: Update `Policy::decide` in `policy.rs`**

Find the always-allow match arm (`policy.rs:86-88`):

```rust
            "read_file" | "list_files" | "search_text" | "git_diff" | "git_status" | "search_notes" | "list_background_processes" | "invoke_subagent" => {
                Decision::Allow
            }
```

Replace with:

```rust
            "read_file" | "list_files" | "search_text" | "git_diff" | "git_status" | "search_notes" | "list_background_processes" | "invoke_subagent" | "monitor_subagents" => {
                Decision::Allow
            }
```

Find the `kill_background_process` arm:

```rust
            "kill_background_process" => self.run.clone(),
```

Replace with:

```rust
            "kill_background_process" | "spawn_subagent" | "cancel_subagent" => self.run.clone(),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib agent:: policy:: 2>&1 | tail -60`
Expected: all pass, including the 4 new tests added in Step 1.

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/agent.rs quecto-agent/src/policy.rs
git commit -m "feat: wire spawn_subagent/monitor_subagents/cancel_subagent into Agent and Policy"
```

---

### Task 7: Full-suite verification

**Files:** none (verification only)

- [ ] **Step 1: Run the whole `quecto-agent` test suite**

Run: `cargo build -p quecto-agent 2>&1 | tail -30 && cargo test -p quecto-agent 2>&1 | tail -80`
Expected: clean build, all tests pass (existing `invoke_subagent` tests untouched and still green, plus every test added in Tasks 1-6).

- [ ] **Step 2: Run clippy to catch anything the tests don't**

Run: `cargo clippy -p quecto-agent -- -D warnings 2>&1 | tail -60`
Expected: no warnings. If clippy flags something in `SpawnSubagent::run`'s thread-setup closure, fix the specific lint — do not add a blanket `#[allow(...)]` without reading what it's flagging first.

- [ ] **Step 3: Optional live smoke test**

If a local model is available (e.g. via Ollama, as used earlier in this session), manually verify concurrent spawning end to end:

Run: `./target/debug/quecto-agent --model <your-model> --yes "Spawn two subagents concurrently: one to count the .rs files in quecto-agent/src, another to count the .rs files in quecto-mcp/src. Then check on both with monitor_subagents until they're done and report both counts."`

Expected: two `spawn_subagent` calls, one or more `monitor_subagents` calls, and a final answer with both counts. This step is exploratory (model behavior varies) — its purpose is to confirm the plumbing works with a real model, not to assert a specific transcript shape.

- [ ] **Step 4: Update the open PR**

```bash
git push
```

Then update PR #22's description (or leave a comment) noting that concurrent subagent spawning/monitoring was added on top of the original `invoke_subagent` observability fix, since both now live on `fix/subagent-repeated-action-feedback`.
