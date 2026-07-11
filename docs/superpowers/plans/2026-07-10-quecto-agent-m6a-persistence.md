# quecto-agent M6a — Session Persistence, clap CLI, and resume/undo/diff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every run to a SQLite session store, restructure the CLI onto `clap` subcommands, and add `resume <id>`, `undo`, and `diff` on top of that store.

**Architecture:** A `session::Store` (rusqlite, bundled) owns a small schema (`sessions`, `messages`, `file_changes`). The agent gains an optional `RunRecorder` trait it drives during the loop; `main` wires a SQLite-backed recorder so a normal run is recorded incrementally. The loop is split into `run`/`resume` over a shared `run_loop`, letting `resume` replay a stored transcript and continue. `main` moves to `clap` with an optional subcommand plus a bare one-shot task positional (`args_conflicts_with_subcommands`), dispatching `resume`/`undo`/`diff` against the store.

**Tech Stack:** Rust 2021; `rusqlite = { version = "0.31", features = ["bundled"] }`; `clap = { version = "4", features = ["derive"] }`; existing `serde_json`. `main` stays a plain `fn main()` (no async).

## Global Constraints

- Build on completed M5 APIs exactly as present: `Message { role, content, tool_calls, tool_call_id }`, `ToolCall { id, name, arguments }`, `FileChange { path, before: Option<String>, after }`, `Context::changes() -> &[FileChange]`, `Agent::new(model, system, max_steps, repo_root, cancel, approval)`, `Outcome`, and the existing `run` loop with policy gate, cancellation, repeat guard, and verification gate.
- Two new dependencies only: `rusqlite` (bundled) and `clap` (derive). No others. Both are verified to build in this environment.
- The default DB path is `$QUECTO_STATE_DB` when set, else `$XDG_STATE_HOME/quecto/sessions.db`, else `$HOME/.local/state/quecto/sessions.db`. Tests and CLI tests MUST point `QUECTO_STATE_DB` at a temp file; never touch the real state dir in tests.
- Persistence must never break a run: a store/record error is logged to stderr and the run continues. Recording is best-effort side output, not a gate.
- Preserve the existing one-shot UX: `quecto-agent [--yes] [--no-verify] "<task>"` still works with the task as a bare positional. `--yes`/`--no-verify` are global flags placed before the task.
- `resume`/`undo`/`diff` operate on an explicit `<id>` where applicable; `undo`/`diff` with no id act on the latest session.
- Message rehydration for `resume` must round-trip exactly, including `tool_calls` and `tool_call_id`, so the Chat API never sees a dangling `tool_call_id`.
- Run repository shell commands through `rtk` per `AGENTS.md`. Stage/commit only the files named by each task. `fmt`/`clippy -D warnings`/`git diff --check` must pass at the end.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `rusqlite` (bundled) and `clap` (derive).
- `quecto-agent/src/session.rs` — `Store`, `SessionRow`, schema, CRUD, `new_session_id`, change/message (de)serialization, `render_change_summary`.
- `quecto-agent/src/agent.rs` — `RunRecorder` trait, optional recorder field, `sync`, `run_loop`/`resume`/`with_messages` split.
- `quecto-agent/src/recorder.rs` — `SqliteRecorder` implementing `RunRecorder` over a `Store`.
- `quecto-agent/src/lib.rs` — declare/export the new modules.
- `quecto-agent/src/main.rs` — `clap` CLI, subcommand dispatch, wiring the recorder, `undo`/`diff`/`resume`.
- `quecto-agent/tests/cli.rs` — subcommand and one-shot-recording tests using `QUECTO_STATE_DB`.

---

### Task 1: SQLite session store

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Create: `quecto-agent/src/session.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `Message`, `ToolCall`, `FileChange`, `BoxErr`.
- Produces:
  - `SessionRow { id, task, repo, model, status }`
  - `Store` with: `open_in_memory() -> Result<Store, BoxErr>`, `open_at(&Path) -> Result<Store, BoxErr>`, `open_default() -> Result<Store, BoxErr>`, `default_path() -> PathBuf`, `create_session(&self, id, task, repo, model) -> Result<(), BoxErr>`, `set_status(&self, id, status) -> Result<(), BoxErr>`, `record_message(&self, id, seq: i64, &Message) -> Result<(), BoxErr>`, `record_change(&self, id, seq: i64, &FileChange) -> Result<(), BoxErr>`, `message_count(&self, id) -> Result<i64, BoxErr>`, `change_count(&self, id) -> Result<i64, BoxErr>`, `latest_session(&self) -> Result<Option<SessionRow>, BoxErr>`, `load_messages(&self, id) -> Result<Vec<Message>, BoxErr>`, `load_changes(&self, id) -> Result<Vec<FileChange>, BoxErr>`, `take_last_change(&self, id) -> Result<Option<FileChange>, BoxErr>`.
  - `new_session_id() -> String`
  - `render_change_summary(&[FileChange]) -> String`

- [ ] **Step 1: Add dependencies**

Add to `quecto-agent/Cargo.toml` under `[dependencies]`:

```toml
rusqlite = { version = "0.31", features = ["bundled"] }
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 2: Write the failing tests**

Create `quecto-agent/src/session.rs` with the tests at the bottom:

```rust
use crate::model::{Message, ToolCall};
use crate::tools::FileChange;
use crate::BoxErr;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    task TEXT NOT NULL,
    repo TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT
);
CREATE TABLE IF NOT EXISTS file_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    path TEXT NOT NULL,
    before TEXT,
    after TEXT NOT NULL
);";

/// A stored session's header row.
pub struct SessionRow {
    pub id: String,
    pub task: String,
    pub repo: String,
    pub model: String,
    pub status: String,
}

/// SQLite-backed session persistence.
pub struct Store {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A time-ordered, process-unique session id.
pub fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", nanos, std::process::id())
}

fn calls_to_json(calls: &[ToolCall]) -> Option<String> {
    if calls.is_empty() {
        return None;
    }
    let arr: Vec<Value> = calls
        .iter()
        .map(|c| json!({"id": c.id, "name": c.name, "arguments": c.arguments}))
        .collect();
    Some(Value::Array(arr).to_string())
}

fn calls_from_json(raw: Option<String>) -> Vec<ToolCall> {
    let Some(raw) = raw else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|v| ToolCall {
            id: v.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            arguments: v.get("arguments").cloned().unwrap_or(Value::Null),
        })
        .collect()
}

impl Store {
    fn init(conn: Connection) -> Result<Store, BoxErr> {
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Store, BoxErr> {
        Store::init(Connection::open_in_memory()?)
    }

    pub fn open_at(path: &Path) -> Result<Store, BoxErr> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Store::init(Connection::open(path)?)
    }

    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("QUECTO_STATE_DB") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        let base = std::env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from(".quecto-state"));
        base.join("quecto").join("sessions.db")
    }

    pub fn open_default() -> Result<Store, BoxErr> {
        Store::open_at(&Store::default_path())
    }

    pub fn create_session(
        &self,
        id: &str,
        task: &str,
        repo: &str,
        model: &str,
    ) -> Result<(), BoxErr> {
        let t = now();
        self.conn.execute(
            "INSERT INTO sessions (id, task, repo, model, status, created, updated) \
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
            (id, task, repo, model, t),
        )?;
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: &str) -> Result<(), BoxErr> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, updated = ?3 WHERE id = ?1",
            (id, status, now()),
        )?;
        Ok(())
    }

    pub fn record_message(&self, id: &str, seq: i64, m: &Message) -> Result<(), BoxErr> {
        self.conn.execute(
            "INSERT INTO messages (session_id, seq, role, content, tool_calls, tool_call_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id,
                seq,
                &m.role,
                &m.content,
                calls_to_json(&m.tool_calls),
                &m.tool_call_id,
            ),
        )?;
        self.conn
            .execute("UPDATE sessions SET updated = ?2 WHERE id = ?1", (id, now()))?;
        Ok(())
    }

    pub fn record_change(&self, id: &str, seq: i64, c: &FileChange) -> Result<(), BoxErr> {
        self.conn.execute(
            "INSERT INTO file_changes (session_id, seq, path, before, after) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (id, seq, &c.path, &c.before, &c.after),
        )?;
        Ok(())
    }

    pub fn message_count(&self, id: &str) -> Result<i64, BoxErr> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn change_count(&self, id: &str) -> Result<i64, BoxErr> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM file_changes WHERE session_id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn latest_session(&self) -> Result<Option<SessionRow>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task, repo, model, status FROM sessions \
             ORDER BY updated DESC, created DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SessionRow {
                id: row.get(0)?,
                task: row.get(1)?,
                repo: row.get(2)?,
                model: row.get(3)?,
                status: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn load_messages(&self, id: &str) -> Result<Vec<Message>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, tool_calls, tool_call_id FROM messages \
             WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([id], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            let tool_calls: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            Ok(Message {
                role,
                content,
                tool_calls: calls_from_json(tool_calls),
                tool_call_id,
            })
        })?;
        let mut out = Vec::new();
        for m in rows {
            out.push(m?);
        }
        Ok(out)
    }

    pub fn load_changes(&self, id: &str) -> Result<Vec<FileChange>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT path, before, after FROM file_changes \
             WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(FileChange {
                path: row.get(0)?,
                before: row.get(1)?,
                after: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for c in rows {
            out.push(c?);
        }
        Ok(out)
    }

    pub fn take_last_change(&self, id: &str) -> Result<Option<FileChange>, BoxErr> {
        let row: Option<(i64, String, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT id, path, before, after FROM file_changes \
                 WHERE session_id = ?1 ORDER BY seq DESC, id DESC LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        let Some((row_id, path, before, after)) = row else {
            return Ok(None);
        };
        self.conn
            .execute("DELETE FROM file_changes WHERE id = ?1", [row_id])?;
        Ok(Some(FileChange {
            path,
            before,
            after,
        }))
    }
}

/// A compact, git-free summary of the file changes recorded in a session.
pub fn render_change_summary(changes: &[FileChange]) -> String {
    if changes.is_empty() {
        return "no recorded changes".to_string();
    }
    let mut out = format!("{} file change(s)\n", changes.len());
    for c in changes {
        let now_lines = c.after.lines().count();
        match &c.before {
            None => out.push_str(&format!("  created   {}  ({} lines)\n", c.path, now_lines)),
            Some(before) => out.push_str(&format!(
                "  modified  {}  (was {} lines, now {} lines)\n",
                c.path,
                before.lines().count(),
                now_lines
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_call() -> Message {
        Message::assistant_with_calls(
            "",
            vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }],
        )
    }

    #[test]
    fn messages_round_trip_with_tool_calls() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        store.record_message("s1", 0, &Message::system("sys")).unwrap();
        store.record_message("s1", 1, &Message::user("hi")).unwrap();
        store.record_message("s1", 2, &assistant_call()).unwrap();
        store
            .record_message("s1", 3, &Message::tool_result("c1", "file body"))
            .unwrap();
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].role, "system");
        assert_eq!(loaded[2].tool_calls.len(), 1);
        assert_eq!(loaded[2].tool_calls[0].name, "read_file");
        assert_eq!(loaded[2].tool_calls[0].arguments, json!({"path": "a.rs"}));
        assert_eq!(loaded[3].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn latest_session_picks_most_recent() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("a", "first", "/r", "m").unwrap();
        store.create_session("b", "second", "/r", "m").unwrap();
        store.set_status("b", "done").unwrap();
        assert_eq!(store.latest_session().unwrap().unwrap().id, "b");
    }

    #[test]
    fn changes_persist_and_take_last_pops_in_reverse() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s1", "t", "/r", "m").unwrap();
        store
            .record_change("s1", 0, &FileChange { path: "a".into(), before: None, after: "x".into() })
            .unwrap();
        store
            .record_change(
                "s1",
                1,
                &FileChange { path: "b".into(), before: Some("old".into()), after: "new".into() },
            )
            .unwrap();
        assert_eq!(store.change_count("s1").unwrap(), 2);
        let last = store.take_last_change("s1").unwrap().unwrap();
        assert_eq!(last.path, "b");
        assert_eq!(last.before.as_deref(), Some("old"));
        assert_eq!(store.change_count("s1").unwrap(), 1);
        let first = store.take_last_change("s1").unwrap().unwrap();
        assert_eq!(first.path, "a");
        assert!(store.take_last_change("s1").unwrap().is_none());
    }

    #[test]
    fn summary_labels_created_and_modified() {
        let changes = vec![
            FileChange { path: "new.rs".into(), before: None, after: "a\nb\n".into() },
            FileChange { path: "old.rs".into(), before: Some("a\n".into()), after: "a\nb\nc\n".into() },
        ];
        let s = render_change_summary(&changes);
        assert!(s.contains("created   new.rs"));
        assert!(s.contains("modified  old.rs"));
    }

    #[test]
    fn empty_summary_is_explicit() {
        assert_eq!(render_change_summary(&[]), "no recorded changes");
    }
}
```

- [ ] **Step 3: Declare the module and run the tests**

Add `mod session;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib session`

Expected: PASS (5 tests). The bundled `rusqlite` compiles its vendored SQLite on first build; allow extra time.

- [ ] **Step 4: Export the public items**

Add to `lib.rs`:

```rust
pub use session::{new_session_id, render_change_summary, SessionRow, Store};
```

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/Cargo.toml quecto-agent/Cargo.lock quecto-agent/src/session.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add SQLite session store"
```

---

### Task 2: Recorder hook in the agent loop

**Files:**
- Modify: `quecto-agent/src/agent.rs`

**Interfaces:**
- Produces:
  - `pub trait RunRecorder: Send { fn message(&mut self, m: &Message); fn change(&mut self, c: &FileChange); }`
  - `Agent::with_recorder(self, Box<dyn RunRecorder>) -> Self`
  - `Agent::with_messages(self, Vec<Message>) -> Self`
  - `Agent::resume(&mut self) -> Outcome`
- Behavior: during a run every message appended to the transcript and every new `FileChange` is emitted to the recorder in order; `with_messages` replaces the seed transcript and marks those messages already-recorded.

- [ ] **Step 1: Add the imports and tests**

Add to the top of `agent.rs`:

```rust
use crate::model::{Message, Model, ToolCall};
use crate::tools::FileChange;
```

(The existing `use crate::model::{Message, Model};` line is replaced by the first line above; keep `ToolCall` if already imported elsewhere in the file—if a duplicate import results, merge into one `use`.)

Add tests to the `tests` module in `agent.rs`:

```rust
#[derive(Default)]
struct FakeRecorder {
    roles: Arc<Mutex<Vec<String>>>,
    changed: Arc<Mutex<Vec<String>>>,
}
impl RunRecorder for FakeRecorder {
    fn message(&mut self, m: &Message) {
        self.roles.lock().unwrap().push(m.role.clone());
    }
    fn change(&mut self, c: &FileChange) {
        self.changed.lock().unwrap().push(c.path.clone());
    }
}

#[test]
fn recorder_captures_seed_task_and_turns() {
    let roles = Arc::new(Mutex::new(Vec::new()));
    let changed = Arc::new(Mutex::new(Vec::new()));
    let model = Scripted::new(vec![text("done")]);
    let mut a = Agent::new(
        Box::new(model),
        "sys",
        10,
        PathBuf::from("."),
        cancel_token(),
        ApprovalMode::NonInteractive,
    )
    .with_recorder(Box::new(FakeRecorder {
        roles: roles.clone(),
        changed: changed.clone(),
    }));
    assert!(matches!(a.run("hi"), Outcome::Complete(_)));
    let got = roles.lock().unwrap().clone();
    assert_eq!(got, vec!["system", "user", "assistant"]);
    assert!(changed.lock().unwrap().is_empty());
}

#[test]
fn recorder_captures_file_changes() {
    use crate::tools::fs::WriteFile;
    let changed = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let write = AssistantMessage {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({"path":"a.txt","content":"hi\n"}),
        }],
        finish_reason: "tool_calls".into(),
    };
    let model = Scripted::new(vec![write, text("done")]);
    let mut a = Agent::new(
        Box::new(model),
        "sys",
        10,
        dir.path().to_path_buf(),
        cancel_token(),
        ApprovalMode::AutoApprove,
    )
    .register(Box::new(WriteFile))
    .with_recorder(Box::new(FakeRecorder {
        roles: Arc::new(Mutex::new(Vec::new())),
        changed: changed.clone(),
    }));
    assert!(matches!(a.run("edit"), Outcome::Complete(_)));
    assert_eq!(changed.lock().unwrap().clone(), vec!["a.txt".to_string()]);
}

#[test]
fn resume_continues_a_seeded_transcript_without_re_recording() {
    let roles = Arc::new(Mutex::new(Vec::new()));
    let seed = vec![
        Message::system("sys"),
        Message::user("original"),
        Message::assistant_with_calls("partial", vec![]),
    ];
    let model = Scripted::new(vec![text("resumed")]);
    let mut a = Agent::new(
        Box::new(model),
        "unused",
        10,
        PathBuf::from("."),
        cancel_token(),
        ApprovalMode::NonInteractive,
    )
    .with_messages(seed)
    .with_recorder(Box::new(FakeRecorder {
        roles: roles.clone(),
        changed: Arc::new(Mutex::new(Vec::new())),
    }));
    match a.resume() {
        Outcome::Complete(s) => assert_eq!(s, "resumed"),
        _ => panic!("expected Complete"),
    }
    // Only the new assistant turn is recorded; the three seeded messages are not.
    assert_eq!(roles.lock().unwrap().clone(), vec!["assistant"]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib agent::tests::recorder agent::tests::resume`

Expected: FAIL — `RunRecorder`, `with_recorder`, `with_messages`, and `resume` do not exist.

- [ ] **Step 3: Add the trait, fields, and builders**

Add near the top of `agent.rs` (after `Outcome`):

```rust
/// Receives the transcript and file mutations of a run in order, for
/// persistence. Recording is best-effort and must never fail the run.
pub trait RunRecorder: Send {
    fn message(&mut self, m: &Message);
    fn change(&mut self, c: &FileChange);
}
```

Add fields to `struct Agent` (after `verifier`):

```rust
    recorder: Option<Box<dyn RunRecorder>>,
    recorded_messages: usize,
    recorded_changes: usize,
```

Initialize them in `Agent::new` (in the struct literal, after `verifier: None,`):

```rust
            recorder: None,
            recorded_messages: 0,
            recorded_changes: 0,
```

Add builders in `impl Agent` (next to `with_verifier`):

```rust
    /// Attach a recorder for session persistence.
    pub fn with_recorder(mut self, recorder: Box<dyn RunRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Replace the seed transcript (used by `resume`). The provided messages are
    /// treated as already recorded so `resume` only persists new turns.
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.recorded_messages = messages.len();
        self.messages = messages;
        self
    }
```

- [ ] **Step 4: Add `sync`, split `run`, and add `resume`**

Add this private method to `impl Agent`:

```rust
    /// Flush any newly-appended messages and file changes to the recorder.
    fn sync(&mut self) {
        if self.recorder.is_none() {
            return;
        }
        while self.recorded_messages < self.messages.len() {
            let m = self.messages[self.recorded_messages].clone();
            if let Some(r) = self.recorder.as_mut() {
                r.message(&m);
            }
            self.recorded_messages += 1;
        }
        while self.recorded_changes < self.cx.changes().len() {
            let c = self.cx.changes()[self.recorded_changes].clone();
            if let Some(r) = self.recorder.as_mut() {
                r.change(&c);
            }
            self.recorded_changes += 1;
        }
    }
```

Replace the existing `pub fn run(&mut self, task: &str) -> Outcome { ... }` signature line and its body opening so that `run` delegates to a shared loop, and add `resume`:

```rust
    pub fn run(&mut self, task: &str) -> Outcome {
        self.messages.push(Message::user(task));
        self.run_loop()
    }

    /// Continue a seeded transcript (from `with_messages`) without appending a
    /// new task.
    pub fn resume(&mut self) -> Outcome {
        self.run_loop()
    }

    fn run_loop(&mut self) -> Outcome {
        let schemas = self.registry.schemas();
        let mut step = 0;
        let mut repeats = RepeatGuard::default();
        let outcome = loop {
            self.sync();
            if step >= self.max_steps {
                break Outcome::StepLimit;
            }
            if self.cancel.load(Ordering::SeqCst) {
                break Outcome::Cancelled;
            }
            let msg = match self.model.complete(&self.messages, &schemas) {
                Ok(m) => m,
                Err(e) => break Outcome::Error(e),
            };
            self.messages.push(Message::assistant_with_calls(
                msg.content.clone(),
                msg.tool_calls.clone(),
            ));
            if msg.tool_calls.is_empty() {
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            eprintln!(
                                "● verify {}  {}",
                                r.command,
                                if r.passed { "passed" } else { "failed" }
                            );
                        }
                        if !report.all_passed() {
                            self.messages.push(Message::user(report.observation()));
                            step += 1;
                            continue;
                        }
                    }
                }
                break Outcome::Complete(msg.content);
            }
            let mut stop: Option<Outcome> = None;
            for call in &msg.tool_calls {
                if self.cancel.load(Ordering::SeqCst) {
                    stop = Some(Outcome::Cancelled);
                    break;
                }
                let out = match self.policy.decide(call) {
                    Decision::Allow => self.registry.dispatch(call, &mut self.cx),
                    Decision::Ask if self.approval.allows(call) => {
                        self.registry.dispatch(call, &mut self.cx)
                    }
                    Decision::Ask => ToolOutput::new("denied: approval required", "denied"),
                    Decision::Deny(reason) => {
                        ToolOutput::new(format!("denied: {reason}"), "denied")
                    }
                };
                if self.cancel.load(Ordering::SeqCst) {
                    stop = Some(Outcome::Cancelled);
                    break;
                }
                eprintln!("● {}  {}", call.name, out.summary);
                if repeats.observe(call, &out.content, self.cx.changes().len()) {
                    self.messages
                        .push(Message::tool_result(&call.id, out.content));
                    stop = Some(Outcome::RepeatedAction);
                    break;
                }
                self.messages
                    .push(Message::tool_result(&call.id, out.content));
            }
            if let Some(outcome) = stop {
                break outcome;
            }
            step += 1;
        };
        self.sync();
        outcome
    }
```

This preserves every existing behavior (policy gate, cancellation checks, repeat guard, verification gate) exactly; it only reshapes `return` into `break` so a final `sync()` runs, and moves the schema/step/repeat setup into `run_loop`. Delete the old loop body that remains in the previous `run`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS — all prior agent tests plus the three new recorder/resume tests.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/agent.rs
rtk git commit -m "feat(agent): record transcript and changes; add resume"
```

---

### Task 3: SQLite-backed recorder

**Files:**
- Create: `quecto-agent/src/recorder.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Consumes: `Store`, `RunRecorder`, `Message`, `FileChange`.
- Produces: `SqliteRecorder::new(store: Store, session_id: String, msg_seq: i64, change_seq: i64) -> SqliteRecorder` implementing `RunRecorder`.

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/recorder.rs`:

```rust
use crate::agent::RunRecorder;
use crate::model::Message;
use crate::session::Store;
use crate::tools::FileChange;

/// A `RunRecorder` that appends the transcript and file changes to a `Store`,
/// assigning monotonically increasing per-session sequence numbers. Persistence
/// errors are logged to stderr and never propagate into the run.
pub struct SqliteRecorder {
    store: Store,
    session_id: String,
    msg_seq: i64,
    change_seq: i64,
}

impl SqliteRecorder {
    pub fn new(store: Store, session_id: String, msg_seq: i64, change_seq: i64) -> Self {
        SqliteRecorder {
            store,
            session_id,
            msg_seq,
            change_seq,
        }
    }
}

impl RunRecorder for SqliteRecorder {
    fn message(&mut self, m: &Message) {
        if let Err(e) = self.store.record_message(&self.session_id, self.msg_seq, m) {
            eprintln!("quecto-agent: failed to persist message: {e}");
        }
        self.msg_seq += 1;
    }

    fn change(&mut self, c: &FileChange) {
        if let Err(e) = self.store.record_change(&self.session_id, self.change_seq, c) {
            eprintln!("quecto-agent: failed to persist change: {e}");
        }
        self.change_seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_appends_messages_and_changes_with_sequence() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s1", "t", "/r", "m").unwrap();
        // Second store handle over the same in-memory DB is not possible; verify
        // via a file-backed store instead.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = Store::open_at(&path).unwrap();
        store.create_session("s1", "t", "/r", "m").unwrap();
        let mut rec = SqliteRecorder::new(Store::open_at(&path).unwrap(), "s1".into(), 0, 0);
        rec.message(&Message::user("hi"));
        rec.change(&FileChange { path: "a".into(), before: None, after: "x".into() });
        let verify = Store::open_at(&path).unwrap();
        assert_eq!(verify.message_count("s1").unwrap(), 1);
        assert_eq!(verify.change_count("s1").unwrap(), 1);
    }
}
```

- [ ] **Step 2: Declare the module and run**

Add `mod recorder;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib recorder`

Expected: PASS (1 test).

- [ ] **Step 3: Export it**

Add to `lib.rs`:

```rust
pub use recorder::SqliteRecorder;
```

Also ensure `RunRecorder` is exported from `agent`:

```rust
pub use agent::{Agent, Outcome, RunRecorder};
```

- [ ] **Step 4: Run and confirm**

Run: `rtk cargo test -p quecto-agent --lib recorder`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/recorder.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add SQLite-backed run recorder"
```

---

### Task 4: clap CLI, recording wiring, and resume/undo/diff

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Consumes: `quecto_agent::{Store, SqliteRecorder, new_session_id, render_change_summary, load_instructions, seed_context, Verifier, Agent, ApprovalMode, Outcome, cancel_token, HttpModel}`.
- Behavior:
  - Default (no subcommand): one-shot run, records a new session, then prints the answer.
  - `resume <id>`: rehydrate messages, continue the loop, record onto the same session.
  - `undo`: pop the latest session's last change and restore the file's prior contents (delete the file when `before` is `None`).
  - `diff`: print `render_change_summary` for the latest session.

- [ ] **Step 1: Add CLI tests**

Replace the top of `quecto-agent/tests/cli.rs` imports and add tests. Keep the existing `oneshot_prints_model_answer`, `no_args_is_usage_error`, `yes_without_task_is_usage_error`, `yes_flag_is_removed_from_the_user_task`, and `no_verify_flag_is_removed_from_the_user_task` tests unchanged except as noted below.

Add a helper and tests:

```rust
#[test]
fn one_shot_run_is_recorded_and_diff_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    // A mock that first asks to write a file, then stops.
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"note.txt\",\"content\":\"hello\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let run = Command::new(bin())
        .args(["--yes", "write", "note.txt"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", &db)
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(), "hello\n");

    let diff = Command::new(bin())
        .arg("diff")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .output()
        .unwrap();
    assert!(diff.status.success());
    assert!(String::from_utf8_lossy(&diff.stdout).contains("note.txt"));
}

#[test]
fn undo_restores_prior_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    std::fs::write(dir.path().join("note.txt"), "old\n").unwrap();
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"note.txt\",\"content\":\"new\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let run = Command::new(bin())
        .args(["--yes", "overwrite note.txt"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", &db)
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(), "new\n");

    let undo = Command::new(bin())
        .arg("undo")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .output()
        .unwrap();
    assert!(undo.status.success(), "undo failed: {}", String::from_utf8_lossy(&undo.stderr));
    assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(), "old\n");
}
```

Add `mock_script` to `quecto-agent/tests/common/mod.rs` — a mock server that serves a queued list of response bodies, one per connection, in order:

```rust
pub fn mock_script(bodies: Vec<&str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let owned: Vec<String> = bodies.into_iter().map(|s| s.to_string()).collect();
    thread::spawn(move || {
        for body in owned {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        }
    });
    format!("http://{addr}")
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            let end = pos + 4;
            let headers = String::from_utf8_lossy(&buf[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while buf.len() < end + length {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}
```

If `find_subslice` is already defined in `common/mod.rs` (added for `mock_capture` in M4), reuse it; otherwise add:

```rust
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
```

Update the test import to `use common::{mock, mock_capture, mock_script};`.

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli one_shot_run_is_recorded undo_restores`

Expected: FAIL — `diff`/`undo` subcommands are not implemented yet (they will be treated as one-shot tasks and try to contact a model).

- [ ] **Step 3: Rewrite `main.rs` with clap**

Replace the entire contents of `quecto-agent/src/main.rs`:

```rust
use clap::{Parser, Subcommand};
use quecto_agent::{
    cancel_token, load_instructions, new_session_id, render_change_summary, seed_context, Agent,
    ApprovalMode, HttpModel, Outcome, SqliteRecorder, Store, Verifier,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

const DEFAULT_SYSTEM: &str =
    "You are quecto-agent, a helpful coding assistant. Answer concisely and accurately.";

#[derive(Parser)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[arg(long, global = true)]
    yes: bool,
    #[arg(long, global = true)]
    no_verify: bool,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    task: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Continue a previous session by id.
    Resume { id: String },
    /// Revert the most recent recorded file change.
    Undo,
    /// Print a summary of the latest session's file changes.
    Diff,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Resume { id }) => resume(&id, cli.yes, cli.no_verify),
        Some(Command::Undo) => undo(),
        Some(Command::Diff) => diff(),
        None => {
            if cli.task.is_empty() {
                eprintln!("usage: quecto-agent [--yes] [--no-verify] \"<task>\"");
                std::process::exit(2);
            }
            run(cli.task.join(" "), cli.yes, cli.no_verify);
        }
    }
}

fn open_store() -> Option<Store> {
    match Store::open_default() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("quecto-agent: session store unavailable: {e}");
            None
        }
    }
}

fn install_cancel() -> quecto_agent::CancelToken {
    let cancel = cancel_token();
    let signal = cancel.clone();
    if let Err(e) = ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst)) {
        eprintln!("quecto-agent: failed to install Ctrl-C handler: {e}");
        std::process::exit(1);
    }
    cancel
}

fn compose_system(cwd: &PathBuf) -> String {
    let mut system = std::env::var("QUECTO_SYSTEM").unwrap_or_else(|_| DEFAULT_SYSTEM.to_string());
    if let Some(rules) = load_instructions(cwd, cwd) {
        system.push_str("\n\n# Repository rules\n");
        system.push_str(&rules);
    }
    system.push_str("\n\n");
    system.push_str(&seed_context(cwd));
    system
}

fn max_steps() -> usize {
    std::env::var("QUECTO_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

fn attach_verifier(mut agent: Agent, no_verify: bool) -> Agent {
    if !no_verify {
        if let Some(verifier) = Verifier::from_env() {
            agent = agent.with_verifier(verifier);
        }
    }
    agent
}

fn finish(outcome: Outcome, store_status: Option<(&Store, &str)>) {
    let status = match &outcome {
        Outcome::Complete(answer) => {
            println!("{answer}");
            "done"
        }
        Outcome::StepLimit => {
            eprintln!("quecto-agent: step limit reached");
            "step_limit"
        }
        Outcome::Error(e) => {
            eprintln!("quecto-agent: {e}");
            "error"
        }
        Outcome::Cancelled => {
            eprintln!("quecto-agent: cancelled");
            "cancelled"
        }
        Outcome::RepeatedAction => {
            eprintln!("quecto-agent: repeated action detected");
            "repeated_action"
        }
    };
    if let Some((store, id)) = store_status {
        let _ = store.set_status(id, status);
    }
    if !matches!(outcome, Outcome::Complete(_)) {
        std::process::exit(1);
    }
}

fn run(task: String, auto_approve: bool, no_verify: bool) {
    let cancel = install_cancel();
    let approval = ApprovalMode::terminal(auto_approve);
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let system = compose_system(&cwd);
    let model = HttpModel::from_env();

    let session_id = new_session_id();
    let mut agent = Agent::new(
        Box::new(model),
        system,
        max_steps(),
        cwd.clone(),
        cancel,
        approval,
    )
    .register_builtins();
    agent = attach_verifier(agent, no_verify);

    // Attach a recorder when the store is available; the run proceeds regardless.
    let recorder_store = open_store();
    if let Some(store) = &recorder_store {
        if let Err(e) =
            store.create_session(&session_id, &task, &cwd.display().to_string(), "")
        {
            eprintln!("quecto-agent: could not create session: {e}");
        } else if let Ok(rec_store) = Store::open_default() {
            agent = agent.with_recorder(Box::new(SqliteRecorder::new(
                rec_store,
                session_id.clone(),
                0,
                0,
            )));
        }
    }

    let outcome = agent.run(&task);
    let status_target = recorder_store.as_ref().map(|s| (s, session_id.as_str()));
    finish(outcome, status_target);
}

fn resume(id: &str, auto_approve: bool, no_verify: bool) {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    let messages = match store.load_messages(id) {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => {
            eprintln!("quecto-agent: no session '{id}'");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    };
    let cancel = install_cancel();
    let approval = ApprovalMode::terminal(auto_approve);
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let model = HttpModel::from_env();

    let msg_seq = store.message_count(id).unwrap_or(0);
    let change_seq = store.change_count(id).unwrap_or(0);
    let mut agent = Agent::new(
        Box::new(model),
        String::new(),
        max_steps(),
        cwd,
        cancel,
        approval,
    )
    .register_builtins()
    .with_messages(messages);
    agent = attach_verifier(agent, no_verify);
    if let Ok(rec_store) = Store::open_default() {
        agent = agent.with_recorder(Box::new(SqliteRecorder::new(
            rec_store,
            id.to_string(),
            msg_seq,
            change_seq,
        )));
    }

    let outcome = agent.resume();
    finish(outcome, Some((&store, id)));
}

fn undo() {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    let latest = match store.latest_session() {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("quecto-agent: no sessions to undo");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    };
    match store.take_last_change(&latest.id) {
        Ok(Some(change)) => {
            let path = PathBuf::from(&latest.repo).join(&change.path);
            let result = match &change.before {
                Some(before) => std::fs::write(&path, before),
                None => std::fs::remove_file(&path),
            };
            match result {
                Ok(()) => println!("reverted {}", change.path),
                Err(e) => {
                    eprintln!("quecto-agent: could not revert {}: {e}", change.path);
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => {
            eprintln!("quecto-agent: no changes to undo");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}

fn diff() {
    let store = match open_store() {
        Some(s) => s,
        None => std::process::exit(1),
    };
    match store.latest_session() {
        Ok(Some(s)) => {
            let changes = store.load_changes(&s.id).unwrap_or_default();
            print!("{}", render_change_summary(&changes));
        }
        Ok(None) => {
            eprintln!("quecto-agent: no sessions");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("quecto-agent: {e}");
            std::process::exit(1);
        }
    }
}
```

Note: `CancelToken` must be exported from the crate root; it already is (`pub use sandbox::{cancel_token, CancelToken, ...}`).

- [ ] **Step 4: Run the CLI tests**

Run: `rtk cargo test -p quecto-agent --test cli`

Expected: PASS for all CLI tests. If `oneshot_prints_model_answer` (which does not set `QUECTO_STATE_DB`) now writes to the real state dir, add `.env("QUECTO_STATE_DB", ...)` pointing at a temp file in that test as well, or accept that it opens the default store harmlessly. Prefer setting `QUECTO_STATE_DB` on every `Command::new(bin())` invocation in this file for isolation.

- [ ] **Step 5: Full verification**

Run: `rtk cargo fmt --all -- --check`
Expected: PASS (run `rtk cargo fmt --all` then re-check if needed).

Run: `rtk cargo test --workspace -- --test-threads=1`
Expected: PASS for both crates.

Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

Run: `rtk git diff --check`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs quecto-agent/tests/common/mod.rs
rtk git commit -m "feat(agent): clap CLI with resume, undo, and diff"
```

---

## Final Acceptance Checklist

- [ ] A normal one-shot run creates a session and records the system+user+assistant+tool messages and every file change, in order.
- [ ] `resume <id>` rehydrates the exact transcript (tool_calls and tool_call_id intact) and continues without re-recording seeded messages.
- [ ] `undo` restores the most recent recorded change (rewrites `before`, or deletes a created file) and removes that change row so repeated `undo` walks backward.
- [ ] `diff` prints a git-free summary of the latest session's changes.
- [ ] `QUECTO_STATE_DB` overrides the DB path; tests never write to the real state dir.
- [ ] Persistence failures degrade to a stderr note; the run still completes.
- [ ] The bare one-shot UX (`quecto-agent [--yes] [--no-verify] "<task>"`) is preserved; empty invocation exits 2.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work (M6b and beyond)

- **M6b:** interactive `chat` REPL mode; the `crossterm` rich renderer (color, live activity lines, spinner); slash-commands (`/help /model /context /diff /status /undo /approve /deny /clear /exit`); `usage` accounting (prompt/completion tokens per turn).
- **M7:** flavor manifests (`toml`), flavor-configured verification/policy, `sha2` trust-on-first-use, `new`/`init` scaffolding subcommands.
- A true unified-diff renderer for `diff` (current output is a per-file summary). Windows support. `mcp` feature (`tokio`+`rmcp`).
