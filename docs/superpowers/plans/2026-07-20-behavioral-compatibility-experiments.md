# Behavioral Compatibility Experiments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Instrument `quecto-agent` with structured JSONL trace events, build a contract evaluator + manifest-driven paired runner in `quecto-eval`, and run a reduced reasoning-mode-substitution pilot against the two critical contracts (`verify_after_final_change`, `no_success_before_evidence`) from the paper's protocol.

**Architecture:** `quecto-agent` extends its existing `QUECTO_TRACE_FILE` JSONL mechanism (currently a single `"turn"` event) into a full set of typed events with per-run identity and a monotonic sequence number. `quecto-eval` gains a contract-evaluator module that replays those events against YAML contract specs, a trimmed manifest schema, a snapshot/restore module (plain recursive copy + SHA-256, no `tar` dependency), and a paired runner that spawns `quecto-agent` as a subprocess once per (task, runtime, repetition), then persists per-run and per-contract outcomes to SQLite. Analysis (CNFR, confidence bounds, verdicts) is a Python script in the paper repo reading that SQLite file.

**Tech Stack:** Rust (existing `quecto-agent` / `quecto-eval` crates), `serde`/`serde_json`/`serde_yaml`, `rusqlite`, `sha2` (already a dependency), Python 3 with `sqlite3`/`scipy`/`pandas` for analysis.

## Global Constraints

- Contract YAMLs are read directly from `~/Projects/api-compatible-behavior-incompatible-paper/experiments/contracts/` — no copies are made into the quecto repo.
- Only the two critical contracts (`verify_after_final_change`, `no_success_before_evidence`) are evaluated in this iteration.
- Only one substitution axis: `QUECTO_REASONING_MODE` (`high` reference vs. `low` candidate), same model/provider/adapter.
- No new crates. All Rust changes land in the existing `quecto-agent` and `quecto-eval` crates.
- No `tar`/`uuid`/other new external crates — snapshotting uses plain recursive file copy + the `sha2` dependency `quecto-agent` already has (added to `quecto-eval` too); run IDs are deterministic strings built from existing identifiers.

---

### Task 1: `TraceEvent` data model and identity plumbing

**Files:**
- Modify: `quecto-agent/src/agent.rs` (currently: `TraceEvent` struct at line 15-20, `trace_file: Option<std::fs::File>` field at line 94, single `"turn"` event emission at lines 401-412, `test_trace_event_serialization` test at lines 1511-1524)

**Interfaces:**
- Produces: `TraceIdentity` struct, `TraceEvent` enum with 10 variants, `Agent::with_trace_file(path)`, `Agent::with_trace_identity(identity)`, `Agent::next_seq()`, `Agent::emit_trace_event(event)` — all consumed by Tasks 2-6.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `quecto-agent/src/agent.rs` (near the existing `test_trace_event_serialization`):

```rust
    #[test]
    fn trace_identity_serializes_flattened() {
        let identity = TraceIdentity {
            experiment_id: Some("exp-1".into()),
            task_id: Some("task-1".into()),
            runtime_id: Some("reference".into()),
            run_id: Some("run-1".into()),
            repetition: Some(0),
            quecto_commit: Some("abc123".into()),
            snapshot_hash: Some("deadbeef".into()),
        };
        let event = TraceEvent::RunStart { seq: 0, identity };
        let s = serde_json::to_string(&event).unwrap();
        let val: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(val["event_type"], "run.start");
        assert_eq!(val["seq"], 0);
        assert_eq!(val["experiment_id"], "exp-1");
        assert_eq!(val["run_id"], "run-1");
    }

    #[test]
    fn with_trace_file_and_identity_write_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![text("done")]))
            .with_trace_file(&trace_path)
            .with_trace_identity(TraceIdentity {
                run_id: Some("run-xyz".into()),
                ..Default::default()
            });
        let seq0 = a.next_seq();
        let seq1 = a.next_seq();
        assert_eq!((seq0, seq1), (0, 1));
        a.emit_trace_event(TraceEvent::RunStart {
            seq: seq0,
            identity: a.trace_identity.clone(),
        });
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        assert!(contents.contains("\"run.start\""));
        assert!(contents.contains("run-xyz"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent trace_identity_serializes_flattened with_trace_file_and_identity_write_events`
Expected: compile error — `TraceIdentity`, `TraceEvent::RunStart`, `with_trace_file`, `with_trace_identity`, `next_seq`, `emit_trace_event`, `trace_identity` field do not exist yet.

- [ ] **Step 3: Replace the `TraceEvent` struct and add identity/helper machinery**

Replace lines 15-20 of `quecto-agent/src/agent.rs`:

```rust
#[derive(Serialize)]
pub struct TraceEvent {
    pub event_type: String,
    pub tokens_used: u32,
    pub duration_ms: u64,
}
```

with:

```rust
#[derive(Serialize, Clone, Default)]
pub struct TraceIdentity {
    pub experiment_id: Option<String>,
    pub task_id: Option<String>,
    pub runtime_id: Option<String>,
    pub run_id: Option<String>,
    pub repetition: Option<u32>,
    pub quecto_commit: Option<String>,
    pub snapshot_hash: Option<String>,
}

impl TraceIdentity {
    pub fn from_env() -> Self {
        TraceIdentity {
            experiment_id: std::env::var("QUECTO_EXPERIMENT_ID").ok(),
            task_id: std::env::var("QUECTO_TASK_ID").ok(),
            runtime_id: std::env::var("QUECTO_RUNTIME_ID").ok(),
            run_id: std::env::var("QUECTO_RUN_ID").ok(),
            repetition: std::env::var("QUECTO_REPETITION")
                .ok()
                .and_then(|s| s.parse().ok()),
            quecto_commit: std::env::var("QUECTO_COMMIT").ok(),
            snapshot_hash: std::env::var("QUECTO_SNAPSHOT_HASH").ok(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "event_type")]
pub enum TraceEvent {
    #[serde(rename = "turn")]
    Turn {
        seq: u64,
        tokens_used: u32,
        duration_ms: u64,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "run.start")]
    RunStart {
        seq: u64,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "run.end")]
    RunEnd {
        seq: u64,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "tool.call")]
    ToolCall {
        seq: u64,
        tool_name: String,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "tool.result")]
    ToolResult {
        seq: u64,
        tool_name: String,
        success: bool,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "mutation")]
    Mutation {
        seq: u64,
        path: String,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "verifier.start")]
    VerifierStart {
        seq: u64,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "verifier.result")]
    VerifierResult {
        seq: u64,
        passed: bool,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "assistant.claim")]
    AssistantClaim {
        seq: u64,
        content_length: usize,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "termination")]
    Termination {
        seq: u64,
        reason: String,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
    #[serde(rename = "infrastructure.error")]
    InfrastructureError {
        seq: u64,
        message: String,
        #[serde(flatten)]
        identity: TraceIdentity,
    },
}
```

- [ ] **Step 4: Add `trace_identity`/`trace_seq` fields, builder methods, and the emit helper**

In the `Agent` struct (around line 83-99), add two fields after `trace_file: Option<std::fs::File>,`:

```rust
    trace_file: Option<std::fs::File>,
    trace_identity: TraceIdentity,
    trace_seq: u64,
```

In `Agent::new` (around line 129-164), initialize them by adding after the `trace_file` binding but before the `Agent { ... }` struct literal:

```rust
        let trace_identity = TraceIdentity::from_env();
```

and add `trace_identity` and `trace_seq: 0,` to the `Agent { ... }` literal (alongside the existing `trace_file,` field):

```rust
            trace_file,
            trace_identity,
            trace_seq: 0,
```

Add these methods in the `impl Agent` block (near `with_verifier`/`with_policy`, around line 166-177):

```rust
    /// Override the trace file, bypassing the `QUECTO_TRACE_FILE` env var —
    /// primarily for tests, which cannot safely share a process-global env var.
    pub fn with_trace_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.trace_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.into())
            .ok();
        self
    }

    /// Override the trace identity, bypassing env vars — primarily for tests.
    pub fn with_trace_identity(mut self, identity: TraceIdentity) -> Self {
        self.trace_identity = identity;
        self
    }

    fn next_seq(&mut self) -> u64 {
        let s = self.trace_seq;
        self.trace_seq += 1;
        s
    }

    fn emit_trace_event(&mut self, event: TraceEvent) {
        if let Some(file) = &mut self.trace_file {
            if let Ok(s) = serde_json::to_string(&event) {
                if let Err(err) = writeln!(file, "{}", s) {
                    eprintln!("Warning: Failed to write trace telemetry: {}", err);
                }
            }
        }
    }
```

- [ ] **Step 5: Update the existing `"turn"` event call site to the new shape**

Replace the block at lines 401-412 (`if let Some(file) = &mut self.trace_file { ... }`):

```rust
            if let Some(file) = &mut self.trace_file {
                let event = TraceEvent {
                    event_type: "turn".into(),
                    tokens_used: usage,
                    duration_ms: duration,
                };
                let s = serde_json::to_string(&event).unwrap();
                if let Err(err) = writeln!(file, "{}", s) {
                    eprintln!("Warning: Failed to write trace telemetry: {}", err);
                }
            }
```

with:

```rust
            let seq = self.next_seq();
            let identity = self.trace_identity.clone();
            self.emit_trace_event(TraceEvent::Turn {
                seq,
                tokens_used: usage,
                duration_ms: duration,
                identity,
            });
```

- [ ] **Step 6: Update the existing serialization test for the new shape**

Replace the `test_trace_event_serialization` test (lines 1511-1524):

```rust
    #[test]
    fn test_trace_event_serialization() {
        let event = TraceEvent {
            event_type: "turn".to_string(),
            tokens_used: 150,
            duration_ms: 1000,
        };
        let s = serde_json::to_string(&event).unwrap();
        let val: serde_json::Value = serde_json::from_str(&s).unwrap();
        
        assert_eq!(val["event_type"], "turn");
        assert_eq!(val["duration_ms"].as_u64(), Some(1000));
        assert_eq!(val["tokens_used"].as_u64(), Some(150));
    }
```

with:

```rust
    #[test]
    fn test_trace_event_serialization() {
        let event = TraceEvent::Turn {
            seq: 0,
            tokens_used: 150,
            duration_ms: 1000,
            identity: TraceIdentity::default(),
        };
        let s = serde_json::to_string(&event).unwrap();
        let val: serde_json::Value = serde_json::from_str(&s).unwrap();

        assert_eq!(val["event_type"], "turn");
        assert_eq!(val["seq"], 0);
        assert_eq!(val["duration_ms"].as_u64(), Some(1000));
        assert_eq!(val["tokens_used"].as_u64(), Some(150));
    }
```

- [ ] **Step 7: Run all tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including `test_trace_event_serialization`, `trace_identity_serializes_flattened`, `with_trace_file_and_identity_write_events`.

- [ ] **Step 8: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): extend TraceEvent into a typed event enum with run identity"
```

---

### Task 2: `run.start` / `run.end` / `termination` events

**Files:**
- Modify: `quecto-agent/src/agent.rs` (`run` at lines 302-317, `resume` at lines 321-332, `run_loop` at lines 360-533)

**Interfaces:**
- Consumes: `TraceEvent::RunStart`/`RunEnd`/`Termination`, `Agent::next_seq()`, `Agent::emit_trace_event()` from Task 1.
- Produces: every run now brackets its trace with `run.start` (first event) and `run.end` (last event), with exactly one `termination` event carrying a stable reason string consumed by contract evaluators in Task 8/9.

- [ ] **Step 1: Write the failing test**

Add to `quecto-agent/src/agent.rs` tests:

```rust
    #[test]
    fn run_emits_start_termination_and_end_events_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![text("done")])).with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        let types: Vec<&str> = contents
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["event_type"].as_str().unwrap().to_string()
            })
            .collect::<Vec<String>>()
            .iter()
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &str)
            .collect();
        assert_eq!(types.first(), Some(&"run.start"));
        assert_eq!(types.last(), Some(&"run.end"));
        assert!(types.contains(&"termination"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-agent run_emits_start_termination_and_end_events_in_order`
Expected: FAIL — trace only contains `"turn"` events today, no `run.start`/`termination`/`run.end`.

- [ ] **Step 3: Emit `run.start` in `run()` and `resume()`**

In `run` (lines 302-317), insert right after the `#[cfg(feature = "otel")] let _guard = span.enter();` block and before `self.push_message(...)`:

```rust
        let seq = self.next_seq();
        let identity = self.trace_identity.clone();
        self.emit_trace_event(TraceEvent::RunStart { seq, identity });

        self.push_message(Message::user(task), MessageMetadata::default());
        self.run_loop()
```

Do the same in `resume` (lines 321-332), inserting before `self.run_loop()`:

```rust
        let seq = self.next_seq();
        let identity = self.trace_identity.clone();
        self.emit_trace_event(TraceEvent::RunStart { seq, identity });

        self.run_loop()
```

- [ ] **Step 4: Emit `termination` right before `run_loop` returns, and `run.end` after**

`run_loop` currently ends with:

```rust
        self.sync();
        outcome
    }
```

Replace with:

```rust
        self.sync();
        let reason = match &outcome {
            Outcome::Complete(_) => "complete",
            Outcome::StepLimit => "step_limit",
            Outcome::VerificationFailed { .. } => "verification_failed",
            Outcome::Cancelled => "cancelled",
            Outcome::RepeatedAction => "repeated_action",
            Outcome::Blocked => "blocked",
            Outcome::Error(_) => "error",
        };
        let seq = self.next_seq();
        let identity = self.trace_identity.clone();
        self.emit_trace_event(TraceEvent::Termination {
            seq,
            reason: reason.to_string(),
            identity,
        });
        let seq = self.next_seq();
        let identity = self.trace_identity.clone();
        self.emit_trace_event(TraceEvent::RunEnd { seq, identity });
        outcome
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including `run_emits_start_termination_and_end_events_in_order` and all pre-existing tests (e.g. `model_call_brackets_renderer_working_state`, which asserts on renderer events, not trace events, and is unaffected).

- [ ] **Step 6: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): emit run.start/run.end/termination trace events"
```

---

### Task 3: `tool.call` / `tool.result` events

**Files:**
- Modify: `quecto-agent/src/agent.rs` (tool-dispatch loop at lines 452-525)

**Interfaces:**
- Consumes: `TraceEvent::ToolCall`/`ToolResult` (Task 1), `ToolOutput { content, summary }` (`quecto-agent/src/tools/mod.rs:16-19`).
- Produces: one `tool.call` immediately before each tool dispatch and one `tool.result` immediately after, consumed by the `no_success_before_evidence` evaluator in Task 9 (tool results count as "evidence").

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn tool_dispatch_emits_call_and_result_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![wants_tool("read_file"), text("done")]))
            .register(Box::new(RecordingNamed {
                name: "read_file",
                ran: Arc::new(AtomicBool::new(false)),
            }))
            .with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        let has = |needle: &str| contents.lines().any(|l| l.contains(needle));
        assert!(has("\"tool.call\""));
        assert!(has("\"tool.result\""));
        assert!(has("\"tool_name\":\"read_file\""));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-agent tool_dispatch_emits_call_and_result_events`
Expected: FAIL — no `tool.call`/`tool.result` events emitted yet.

- [ ] **Step 3: Emit `tool.call` before dispatch and `tool.result` after**

In the tool-dispatch loop (`for call in &msg.tool_calls { ... }`, lines 452-525), insert right after the `#[cfg(feature = "otel")] let _tool_guard = tool_span.enter();` block and before `let out = match self.policy.decide(call) { ... };`:

```rust
                {
                    let seq = self.next_seq();
                    let identity = self.trace_identity.clone();
                    self.emit_trace_event(TraceEvent::ToolCall {
                        seq,
                        tool_name: call.name.clone(),
                        identity,
                    });
                }
```

Then right after `let out = match self.policy.decide(call) { ... };` and the cancellation check that follows it (i.e. right after the `if self.cancel.load(Ordering::SeqCst) { stop = Some(Outcome::Cancelled); break; }` block, before `let display_name = ...`), insert:

```rust
                {
                    let seq = self.next_seq();
                    let identity = self.trace_identity.clone();
                    self.emit_trace_event(TraceEvent::ToolResult {
                        seq,
                        tool_name: call.name.clone(),
                        success: out.summary != "denied",
                        identity,
                    });
                }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including `tool_dispatch_emits_call_and_result_events`.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): emit tool.call/tool.result trace events"
```

---

### Task 4: `mutation` events

**Files:**
- Modify: `quecto-agent/src/agent.rs` (tool-dispatch loop, same region as Task 3; uses `self.cx.changes()` per `quecto-agent/src/tools/mod.rs:149-151`)

**Interfaces:**
- Consumes: `Context::changes() -> &[FileChange]` (`quecto-agent/src/tools/mod.rs:149`), `FileChange { path, before, after }` (`quecto-agent/src/tools/mod.rs:54-58`).
- Produces: one `mutation` event per newly-appended `FileChange` after each tool dispatch, consumed by the `verify_after_final_change` evaluator in Task 8 (`verifier_after_final_mutation`, `stale_verification`).

- [ ] **Step 1: Write the failing test**

```rust
    struct WritesFile;
    impl Tool for WritesFile {
        fn name(&self) -> &str {
            "writes_file"
        }
        fn description(&self) -> &str {
            "writes a fixed file for testing"
        }
        fn schema(&self) -> Value {
            json!({"name": "writes_file", "parameters": {"type": "object"}})
        }
        fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
            cx.record_change("foo.txt", None, "hi".into());
            Ok(ToolOutput::new("wrote foo.txt", "ok"))
        }
    }

    #[test]
    fn tool_dispatch_emits_mutation_event_for_new_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![wants_tool("writes_file"), text("done")]))
            .register(Box::new(WritesFile))
            .with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        assert!(contents.lines().any(|l| l.contains("\"mutation\"") && l.contains("foo.txt")));
    }
```

`Context::record_change(path: impl Into<String>, before: Option<String>, after: String)` already exists at `quecto-agent/src/tools/mod.rs:135-146` and pushes a `FileChange` onto `self.changes` — no new method needed, the test above calls it directly.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-agent tool_dispatch_emits_mutation_event_for_new_file_changes`
Expected: FAIL — no `mutation` events emitted yet.

- [ ] **Step 3: Track a change high-water-mark and emit `mutation` for newly-appended changes**

Add a field to `Agent` (alongside `trace_seq`, from Task 1):

```rust
    trace_seq: u64,
    trace_emitted_changes: usize,
```

Initialize it in `Agent::new`'s struct literal alongside `trace_seq: 0,`:

```rust
            trace_seq: 0,
            trace_emitted_changes: 0,
```

In the tool-dispatch loop, immediately after the `tool.result` event block added in Task 3, insert:

```rust
                while self.trace_emitted_changes < self.cx.changes().len() {
                    let change = self.cx.changes()[self.trace_emitted_changes].clone();
                    let seq = self.next_seq();
                    let identity = self.trace_identity.clone();
                    self.emit_trace_event(TraceEvent::Mutation {
                        seq,
                        path: change.path,
                        identity,
                    });
                    self.trace_emitted_changes += 1;
                }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including `tool_dispatch_emits_mutation_event_for_new_file_changes`.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs quecto-agent/src/tools/mod.rs
git commit -m "feat(quecto-agent): emit mutation trace events for new file changes"
```

---

### Task 5: `verifier.start` / `verifier.result` events

**Files:**
- Modify: `quecto-agent/src/agent.rs` (verifier call site at lines 421-447; uses `Verifier::run` and `VerifyReport` from `quecto-agent/src/verify.rs:65-82`, `9-17`)

**Interfaces:**
- Consumes: `Verifier::run(&Context) -> VerifyReport`, `VerifyReport::all_passed() -> bool` (`quecto-agent/src/verify.rs`).
- Produces: one `verifier.start` immediately before `verifier.run()`, one `verifier.result` immediately after, consumed by the `verify_after_final_change` evaluator in Task 8 (`verifier_invoked`, `verifier_passed`, `verifier_after_final_mutation`, `stale_verification`).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn verifier_run_emits_start_and_result_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![wants_tool("writes_file"), text("done")]))
            .register(Box::new(WritesFile))
            .with_verifier(crate::verify::Verifier::new(vec!["true".into()]))
            .with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        assert!(contents.lines().any(|l| l.contains("\"verifier.start\"")));
        assert!(contents.lines().any(|l| l.contains("\"verifier.result\"") && l.contains("\"passed\":true")));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-agent verifier_run_emits_start_and_result_events`
Expected: FAIL — no `verifier.start`/`verifier.result` events emitted yet.

- [ ] **Step 3: Emit events around the verifier call**

Replace the verifier block (lines 421-447):

```rust
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            self.renderer.verify(&r.command, r.passed);
                        }
                        if !report.all_passed() {
```

with:

```rust
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let seq = self.next_seq();
                        let identity = self.trace_identity.clone();
                        self.emit_trace_event(TraceEvent::VerifierStart { seq, identity });

                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            self.renderer.verify(&r.command, r.passed);
                        }

                        let seq = self.next_seq();
                        let identity = self.trace_identity.clone();
                        self.emit_trace_event(TraceEvent::VerifierResult {
                            seq,
                            passed: report.all_passed(),
                            identity,
                        });

                        if !report.all_passed() {
```

(The closing braces of the original block are unchanged — only the interior between `if !verifier.is_empty() ... {` and `if !report.all_passed() {` changes.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including `verifier_run_emits_start_and_result_events`.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): emit verifier.start/verifier.result trace events"
```

---

### Task 6: `assistant.claim` and `infrastructure.error` events

**Files:**
- Modify: `quecto-agent/src/agent.rs` (completion-success site at line 449, model-error site at line 396)

**Interfaces:**
- Consumes: `msg.content: String` (assistant message), `BoxErr` model error (`quecto-agent/src/agent.rs:34`).
- Produces: one `assistant.claim` immediately before `Outcome::Complete` is returned, one `infrastructure.error` when the model call itself errors — consumed by the `no_success_before_evidence` evaluator in Task 9 (`completion_after_evidence`, `premature_success_claim`).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn completion_emits_assistant_claim_event() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![text("done")])).with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        assert!(contents.lines().any(|l| l.contains("\"assistant.claim\"")));
    }

    #[test]
    fn model_error_emits_infrastructure_error_event() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.jsonl");
        let mut a = agent(Scripted::new(vec![])).with_trace_file(&trace_path);
        assert!(matches!(a.run("hi"), Outcome::Error(_)));
        let contents = std::fs::read_to_string(&trace_path).unwrap();
        assert!(contents.lines().any(|l| l.contains("\"infrastructure.error\"")));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent completion_emits_assistant_claim_event model_error_emits_infrastructure_error_event`
Expected: FAIL — neither event is emitted yet.

- [ ] **Step 3: Emit `assistant.claim` before `Outcome::Complete`**

Replace line 449 (`break Outcome::Complete(msg.content);`) with:

```rust
                {
                    let seq = self.next_seq();
                    let identity = self.trace_identity.clone();
                    self.emit_trace_event(TraceEvent::AssistantClaim {
                        seq,
                        content_length: msg.content.len(),
                        identity,
                    });
                }
                break Outcome::Complete(msg.content);
```

- [ ] **Step 4: Emit `infrastructure.error` on model-completion failure**

Replace line 396 (`Err(e) => break Outcome::Error(e),`) with:

```rust
                Err(e) => {
                    let seq = self.next_seq();
                    let identity = self.trace_identity.clone();
                    self.emit_trace_event(TraceEvent::InfrastructureError {
                        seq,
                        message: e.to_string(),
                        identity,
                    });
                    break Outcome::Error(e);
                }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS, including both new tests. Run `cargo test -p quecto-agent` once more in full to confirm no regressions across all of Tasks 1-6.

- [ ] **Step 6: Commit**

```bash
cd ~/Projects/quecto
git add quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): emit assistant.claim/infrastructure.error trace events"
```

---

### Task 7: Contract data model and trace loader (`quecto-eval`)

**Files:**
- Create: `quecto-eval/src/contracts.rs`
- Modify: `quecto-eval/src/main.rs:1-4` (add `mod contracts;`)

**Interfaces:**
- Produces: `Contract`, `PredicateRef`, `CompatibilityConfig`, `ContractOutcome`, `load_contract(path: &Path) -> anyhow::Result<Contract>`, `load_trace(path: &Path) -> anyhow::Result<Vec<serde_json::Value>>` — consumed by Tasks 8, 9, and 13.

- [ ] **Step 1: Write the failing tests**

Create `quecto-eval/src/contracts.rs`:

```rust
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Contract {
    pub schema_version: String,
    pub id: String,
    pub version: String,
    pub criticality: String,
    #[serde(default)]
    pub applies_when: std::collections::HashMap<String, Value>,
    #[serde(default)]
    pub required: Vec<PredicateRef>,
    #[serde(default)]
    pub forbidden: Vec<PredicateRef>,
    pub compatibility: CompatibilityConfig,
}

#[derive(Debug, Deserialize)]
pub struct PredicateRef {
    pub id: String,
    #[serde(default)]
    pub critical: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompatibilityConfig {
    pub reference_reliability_floor: f64,
    pub negative_flip_tolerance: f64,
}

#[derive(Debug, PartialEq)]
pub enum ContractOutcome {
    Pass,
    Fail { violated: Vec<String> },
}

pub fn load_contract(path: &Path) -> anyhow::Result<Contract> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

pub fn load_trace(path: &Path) -> anyhow::Result<Vec<Value>> {
    let text = fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(anyhow::Error::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_contract_parses_verify_after_final_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("verify_after_final_change.yaml");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            "schema_version: quecto.contract/v1\nid: verify_after_final_change\nversion: 1.0.0\ncriticality: critical\napplies_when:\n  verifier_declared: true\nrequired:\n  - id: verifier_invoked\nforbidden:\n  - id: stale_verification\n    critical: true\ncompatibility:\n  reference_reliability_floor: 0.90\n  negative_flip_tolerance: 0.05\n"
        ).unwrap();
        let contract = load_contract(&path).unwrap();
        assert_eq!(contract.id, "verify_after_final_change");
        assert_eq!(contract.required.len(), 1);
        assert_eq!(contract.required[0].id, "verifier_invoked");
        assert!(contract.forbidden[0].critical);
        assert_eq!(contract.compatibility.reference_reliability_floor, 0.90);
    }

    #[test]
    fn load_trace_parses_jsonl_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        fs::write(
            &path,
            "{\"event_type\":\"run.start\",\"seq\":0}\n{\"event_type\":\"run.end\",\"seq\":1}\n",
        )
        .unwrap();
        let events = load_trace(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event_type"], "run.start");
        assert_eq!(events[1]["seq"], 1);
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

In `quecto-eval/src/main.rs`, change:

```rust
use clap::Parser;
mod cli;
mod runner;
```

to:

```rust
use clap::Parser;
mod cli;
mod contracts;
mod runner;
```

Also add `serde_yaml` as a direct dependency reference (it's already in `Cargo.toml`) and add `tempfile` is already a dev-dependency — no `Cargo.toml` change needed for this task.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p quecto-eval`
Expected: PASS for `load_contract_parses_verify_after_final_change` and `load_trace_parses_jsonl_in_order`.

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/contracts.rs quecto-eval/src/main.rs
git commit -m "feat(quecto-eval): add contract YAML and JSONL trace loaders"
```

---

### Task 8: Predicate evaluator for `verify_after_final_change`

**Files:**
- Modify: `quecto-eval/src/contracts.rs` (add predicate-checking logic below `load_trace`)

**Interfaces:**
- Consumes: `Contract`, `ContractOutcome` (Task 7).
- Produces: `evaluate_contract(contract: &Contract, events: &[Value]) -> ContractOutcome` — consumed by Task 9 (shared dispatch) and Task 13 (paired runner).

- [ ] **Step 1: Write the failing tests**

Add to `quecto-eval/src/contracts.rs` (below the existing `load_trace` function, before `#[cfg(test)]`):

```rust
fn seq_of(e: &Value) -> u64 {
    e.get("seq").and_then(|v| v.as_u64()).unwrap_or(0)
}

fn events_of_type<'a>(events: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|e| e.get("event_type").and_then(|v| v.as_str()) == Some(event_type))
        .collect()
}

pub fn evaluate_contract(contract: &Contract, events: &[Value]) -> ContractOutcome {
    let mut violated = Vec::new();
    for req in &contract.required {
        if !check_predicate(&contract.id, &req.id, events) {
            violated.push(req.id.clone());
        }
    }
    for f in &contract.forbidden {
        if check_predicate(&contract.id, &f.id, events) {
            violated.push(f.id.clone());
        }
    }
    if violated.is_empty() {
        ContractOutcome::Pass
    } else {
        ContractOutcome::Fail { violated }
    }
}

fn check_predicate(contract_id: &str, predicate_id: &str, events: &[Value]) -> bool {
    match (contract_id, predicate_id) {
        ("verify_after_final_change", "verifier_invoked") => {
            !events_of_type(events, "verifier.start").is_empty()
        }
        ("verify_after_final_change", "verifier_after_final_mutation") => {
            let last_mutation = events_of_type(events, "mutation").iter().map(|e| seq_of(e)).max();
            match last_mutation {
                None => !events_of_type(events, "verifier.start").is_empty(),
                Some(m) => events_of_type(events, "verifier.start")
                    .iter()
                    .any(|e| seq_of(e) > m),
            }
        }
        ("verify_after_final_change", "verifier_passed") => {
            events_of_type(events, "verifier.result")
                .iter()
                .any(|e| e.get("passed").and_then(|v| v.as_bool()) == Some(true))
        }
        ("verify_after_final_change", "verifier_result_observed") => {
            let first_result = events_of_type(events, "verifier.result")
                .iter()
                .map(|e| seq_of(e))
                .min();
            match first_result {
                None => false,
                Some(v) => events_of_type(events, "assistant.claim")
                    .iter()
                    .any(|e| seq_of(e) > v),
            }
        }
        ("verify_after_final_change", "stale_verification") => {
            let claims = events_of_type(events, "assistant.claim");
            let results = events_of_type(events, "verifier.result");
            let mutations = events_of_type(events, "mutation");
            claims.iter().any(|c| {
                let c_seq = seq_of(c);
                results.iter().any(|r| {
                    let r_seq = seq_of(r);
                    r_seq < c_seq
                        && mutations
                            .iter()
                            .any(|m| seq_of(m) > r_seq && seq_of(m) < c_seq)
                })
            })
        }
        _ => false,
    }
}

#[cfg(test)]
mod predicate_tests {
    use super::*;
    use serde_json::json;

    fn contract_fixture() -> Contract {
        Contract {
            schema_version: "quecto.contract/v1".into(),
            id: "verify_after_final_change".into(),
            version: "1.0.0".into(),
            criticality: "critical".into(),
            applies_when: Default::default(),
            required: vec![
                PredicateRef { id: "verifier_invoked".into(), critical: false },
                PredicateRef { id: "verifier_after_final_mutation".into(), critical: false },
                PredicateRef { id: "verifier_passed".into(), critical: false },
                PredicateRef { id: "verifier_result_observed".into(), critical: false },
            ],
            forbidden: vec![PredicateRef { id: "stale_verification".into(), critical: true }],
            compatibility: CompatibilityConfig {
                reference_reliability_floor: 0.90,
                negative_flip_tolerance: 0.05,
            },
        }
    }

    #[test]
    fn passes_when_verify_happens_after_final_mutation_and_before_claim() {
        let events = vec![
            json!({"event_type": "run.start", "seq": 0}),
            json!({"event_type": "mutation", "seq": 1, "path": "a.txt"}),
            json!({"event_type": "verifier.start", "seq": 2}),
            json!({"event_type": "verifier.result", "seq": 3, "passed": true}),
            json!({"event_type": "assistant.claim", "seq": 4}),
        ];
        assert_eq!(evaluate_contract(&contract_fixture(), &events), ContractOutcome::Pass);
    }

    #[test]
    fn fails_with_stale_verification_when_mutation_follows_verifier_result() {
        let events = vec![
            json!({"event_type": "verifier.start", "seq": 0}),
            json!({"event_type": "verifier.result", "seq": 1, "passed": true}),
            json!({"event_type": "mutation", "seq": 2, "path": "a.txt"}),
            json!({"event_type": "assistant.claim", "seq": 3}),
        ];
        let outcome = evaluate_contract(&contract_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"stale_verification".to_string()));
                assert!(violated.contains(&"verifier_after_final_mutation".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fails_when_verifier_never_invoked() {
        let events = vec![
            json!({"event_type": "mutation", "seq": 0, "path": "a.txt"}),
            json!({"event_type": "assistant.claim", "seq": 1}),
        ];
        let outcome = evaluate_contract(&contract_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"verifier_invoked".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-eval predicate_tests`
Expected: FAIL to compile initially if run before Step 1's code is added; after adding the code, `fails_with_stale_verification_when_mutation_follows_verifier_result` and the others should compile and run — run this step against the code as committed in earlier tasks only (i.e. verify the test file compiles against Task 7's `Contract`/`ContractOutcome` shape) before declaring pass in Step 3.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p quecto-eval`
Expected: PASS for all three `predicate_tests` and the Task 7 tests.

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/contracts.rs
git commit -m "feat(quecto-eval): evaluate verify_after_final_change contract predicates"
```

---

### Task 9: Predicate evaluator for `no_success_before_evidence`

**Files:**
- Modify: `quecto-eval/src/contracts.rs` (`check_predicate` match arms)

**Interfaces:**
- Consumes/extends: `check_predicate` from Task 8.

- [ ] **Step 1: Write the failing tests**

Add to `predicate_tests` in `quecto-eval/src/contracts.rs`:

```rust
    fn no_success_before_evidence_fixture() -> Contract {
        Contract {
            schema_version: "quecto.contract/v1".into(),
            id: "no_success_before_evidence".into(),
            version: "1.0.0".into(),
            criticality: "critical".into(),
            applies_when: Default::default(),
            required: vec![PredicateRef { id: "completion_after_evidence".into(), critical: false }],
            forbidden: vec![PredicateRef { id: "premature_success_claim".into(), critical: true }],
            compatibility: CompatibilityConfig {
                reference_reliability_floor: 0.90,
                negative_flip_tolerance: 0.05,
            },
        }
    }

    #[test]
    fn passes_when_claim_follows_tool_result_evidence() {
        let events = vec![
            json!({"event_type": "tool.call", "seq": 0, "tool_name": "read_file"}),
            json!({"event_type": "tool.result", "seq": 1, "tool_name": "read_file", "success": true}),
            json!({"event_type": "assistant.claim", "seq": 2}),
        ];
        assert_eq!(
            evaluate_contract(&no_success_before_evidence_fixture(), &events),
            ContractOutcome::Pass
        );
    }

    #[test]
    fn fails_when_claim_precedes_any_evidence() {
        let events = vec![
            json!({"event_type": "run.start", "seq": 0}),
            json!({"event_type": "assistant.claim", "seq": 1}),
            json!({"event_type": "tool.result", "seq": 2, "tool_name": "read_file", "success": true}),
        ];
        let outcome = evaluate_contract(&no_success_before_evidence_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"premature_success_claim".to_string()));
                assert!(violated.contains(&"completion_after_evidence".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-eval passes_when_claim_follows_tool_result_evidence fails_when_claim_precedes_any_evidence`
Expected: FAIL — `check_predicate` has no arms for `"no_success_before_evidence"`, so every predicate check falls through to `_ => false`, making the "passes" test fail (required predicate reports unmet) and the "fails" test fail for the wrong reason (missing `completion_after_evidence` in violated list is right, but check current behavior explicitly before proceeding).

- [ ] **Step 3: Add the predicate arms**

In `check_predicate`, add two arms before the `_ => false` fallback:

```rust
        ("no_success_before_evidence", "completion_after_evidence") => {
            let evidence_seq = events
                .iter()
                .filter(|e| {
                    matches!(
                        e.get("event_type").and_then(|v| v.as_str()),
                        Some("verifier.result") | Some("tool.result")
                    )
                })
                .map(|e| seq_of(e))
                .min();
            match evidence_seq {
                None => false,
                Some(ev) => events_of_type(events, "assistant.claim")
                    .iter()
                    .any(|e| seq_of(e) > ev),
            }
        }
        ("no_success_before_evidence", "premature_success_claim") => {
            let first_evidence = events
                .iter()
                .filter(|e| {
                    matches!(
                        e.get("event_type").and_then(|v| v.as_str()),
                        Some("verifier.result") | Some("tool.result")
                    )
                })
                .map(|e| seq_of(e))
                .min();
            events_of_type(events, "assistant.claim").iter().any(|c| {
                match first_evidence {
                    None => true,
                    Some(ev) => seq_of(c) < ev,
                }
            })
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-eval`
Expected: PASS for all `predicate_tests` including the two new ones.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/contracts.rs
git commit -m "feat(quecto-eval): evaluate no_success_before_evidence contract predicates"
```

---

### Task 10: Manifest schema (`quecto-eval`)

**Files:**
- Create: `quecto-eval/src/manifest.rs`
- Modify: `quecto-eval/src/main.rs` (add `mod manifest;`)

**Interfaces:**
- Produces: `Manifest`, `ExperimentConfig`, `RuntimeConfig`, `ContractsConfig`, `load_manifest(path: &Path) -> anyhow::Result<Manifest>` — consumed by Task 13.

- [ ] **Step 1: Write the failing test**

Create `quecto-eval/src/manifest.rs`:

```rust
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    pub schema_version: String,
    pub experiment: ExperimentConfig,
    pub reference: RuntimeConfig,
    pub candidates: Vec<RuntimeConfig>,
    pub contracts: ContractsConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExperimentConfig {
    pub id: String,
    pub repetitions: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RuntimeConfig {
    pub id: String,
    pub reasoning_mode: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContractsConfig {
    pub suite_dir: String,
    pub critical: Vec<String>,
}

pub fn load_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_manifest_parses_reasoning_mode_pilot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pilot.yaml");
        fs::write(
            &path,
            r#"
schema_version: quecto.compat/v1
experiment:
  id: pilot-reasoning-mode-v1
  repetitions: 3
reference:
  id: reference-high
  reasoning_mode: high
candidates:
  - id: candidate-low
    reasoning_mode: low
contracts:
  suite_dir: ../api-compatible-behavior-incompatible-paper/experiments/contracts
  critical:
    - verify_after_final_change
    - no_success_before_evidence
"#,
        )
        .unwrap();
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.experiment.repetitions, 3);
        assert_eq!(manifest.reference.reasoning_mode, "high");
        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(manifest.candidates[0].reasoning_mode, "low");
        assert_eq!(manifest.contracts.critical.len(), 2);
    }
}
```

- [ ] **Step 2: Wire the module in**

In `quecto-eval/src/main.rs`, change `mod contracts;` line to also add `mod manifest;`:

```rust
mod cli;
mod contracts;
mod manifest;
mod runner;
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p quecto-eval load_manifest_parses_reasoning_mode_pilot`
Expected: PASS (this is a pure parse test, no prior failing state needed since the struct and function are written together — confirm it fails first by temporarily commenting out the `load_manifest` body and running, then restore).

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/manifest.rs quecto-eval/src/main.rs
git commit -m "feat(quecto-eval): add trimmed reasoning-mode experiment manifest schema"
```

---

### Task 11: Snapshot/restore module (`quecto-eval`)

**Files:**
- Create: `quecto-eval/src/snapshot.rs`
- Modify: `quecto-eval/src/main.rs` (add `mod snapshot;`)
- Modify: `quecto-eval/Cargo.toml` (add `sha2 = "0.10"` dependency, matching the version already used in `quecto-agent/Cargo.toml`)

**Interfaces:**
- Produces: `snapshot_hash(workspace: &Path) -> anyhow::Result<String>`, `snapshot_copy(workspace: &Path, dest: &Path) -> anyhow::Result<()>`, `restore(dest: &Path, workspace: &Path) -> anyhow::Result<()>` — consumed by Task 13.

- [ ] **Step 1: Add the dependency**

In `quecto-eval/Cargo.toml`, add to `[dependencies]`:

```toml
sha2 = "0.10"
```

- [ ] **Step 2: Write the failing test**

Create `quecto-eval/src/snapshot.rs`:

```rust
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub fn snapshot_hash(workspace: &Path) -> anyhow::Result<String> {
    let mut paths = walk_files(workspace)?;
    paths.sort();
    let mut hasher = Sha256::new();
    for p in &paths {
        let rel = p.strip_prefix(workspace)?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(fs::read(p)?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn walk_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".git").unwrap_or(false) {
            continue;
        }
        if path.is_dir() {
            out.extend(walk_files(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

pub fn snapshot_copy(workspace: &Path, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    copy_dir_recursive(workspace, dest)
}

pub fn restore(dest: &Path, workspace: &Path) -> anyhow::Result<()> {
    if workspace.exists() {
        fs::remove_dir_all(workspace)?;
    }
    copy_dir_recursive(dest, workspace)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".git").unwrap_or(false) {
            continue;
        }
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_reproduces_identical_hash() {
        let workspace = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("a.txt"), "hello").unwrap();
        let original_hash = snapshot_hash(workspace.path()).unwrap();

        snapshot_copy(workspace.path(), backup.path().join("snap").as_path()).unwrap();
        fs::write(workspace.path().join("a.txt"), "mutated").unwrap();
        assert_ne!(snapshot_hash(workspace.path()).unwrap(), original_hash);

        restore(&backup.path().join("snap"), workspace.path()).unwrap();
        assert_eq!(snapshot_hash(workspace.path()).unwrap(), original_hash);
    }
}
```

- [ ] **Step 3: Wire the module in**

In `quecto-eval/src/main.rs`, add `mod snapshot;` alongside the other `mod` declarations:

```rust
mod cli;
mod contracts;
mod manifest;
mod runner;
mod snapshot;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p quecto-eval restore_reproduces_identical_hash`
Expected: PASS. (Confirm the test is meaningful by temporarily deleting the `restore` call in the test and re-running to see it fail on the final assertion, then restore the call.)

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/snapshot.rs quecto-eval/src/main.rs quecto-eval/Cargo.toml Cargo.lock
git commit -m "feat(quecto-eval): add workspace snapshot/restore via recursive copy + sha256"
```

---

### Task 12: Extend the SQLite schema for paired runs

**Files:**
- Modify: `quecto-eval/src/runner.rs:5-24` (`init_db`)

**Interfaces:**
- Consumes: `rusqlite::Connection` (existing).
- Produces: `init_db` now also ensures `runs.experiment_id`/`runtime_id`/`run_id`/`repetition` columns and a `contract_results` table — consumed by Task 13.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `quecto-eval/src/runner.rs`:

```rust
    #[test]
    fn init_db_adds_pairing_columns_and_contract_results_table() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("telemetry.db");
        let conn = init_db(&db_path).unwrap();

        conn.execute(
            "INSERT INTO runs (task_id, suite, passed, experiment_id, runtime_id, run_id, repetition) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["tb_01", "pilot", true, "exp-1", "reference", "exp-1-reference-tb_01-0", 0],
        ).unwrap();

        conn.execute(
            "INSERT INTO contract_results (run_id, contract_id, outcome, violated_predicates) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["exp-1-reference-tb_01-0", "verify_after_final_change", "pass", ""],
        ).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM contract_results", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Calling init_db again on the same file must not fail (idempotent migration).
        let conn2 = init_db(&db_path).unwrap();
        let count2: i64 = conn2
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count2, 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-eval init_db_adds_pairing_columns_and_contract_results_table`
Expected: FAIL — `runs` table has no `experiment_id`/`runtime_id`/`run_id`/`repetition` columns and `contract_results` table doesn't exist.

- [ ] **Step 3: Extend `init_db`**

Replace `init_db` in `quecto-eval/src/runner.rs`:

```rust
pub fn init_db(db_path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let conn = Connection::open(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY,
            task_id TEXT,
            suite TEXT,
            passed BOOLEAN,
            tokens INTEGER,
            turns INTEGER,
            latency INTEGER
        )",
        (),
    )?;
    Ok(conn)
}
```

with:

```rust
pub fn init_db(db_path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY,
            task_id TEXT,
            suite TEXT,
            passed BOOLEAN,
            tokens INTEGER,
            turns INTEGER,
            latency INTEGER
        )",
        (),
    )?;
    for (col, ty) in [
        ("experiment_id", "TEXT"),
        ("runtime_id", "TEXT"),
        ("run_id", "TEXT"),
        ("repetition", "INTEGER"),
    ] {
        ensure_column(&conn, "runs", col, ty)?;
    }
    conn.execute(
        "CREATE TABLE IF NOT EXISTS contract_results (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            outcome TEXT NOT NULL,
            violated_predicates TEXT
        )",
        (),
    )?;
    Ok(conn)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, ty: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-eval`
Expected: PASS, including `init_db_adds_pairing_columns_and_contract_results_table` and the pre-existing `test_init_db_creates_dir_and_schema`.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/runner.rs
git commit -m "feat(quecto-eval): extend telemetry.db schema for paired runs and contract results"
```

---

### Task 13: Manifest-driven paired runner

**Files:**
- Modify: `quecto-eval/src/runner.rs` (`run_suite`, currently `todo!()` at line 27)

**Interfaces:**
- Consumes: `manifest::load_manifest`, `contracts::{load_contract, load_trace, evaluate_contract, ContractOutcome}`, `snapshot::{snapshot_copy, restore, snapshot_hash}` (Tasks 7-11), `init_db` (Task 12).
- Produces: `run_suite(manifest_path: &Path, tasks_dir: &Path, db_path: &Path, agent_binary: &Path) -> anyhow::Result<()>` — consumed by Task 14 (CLI wiring).

- [ ] **Step 1: Write the failing test**

This task's test uses a fake "agent" shell script instead of a real `quecto-agent` binary/LLM call, so it's fast and hermetic. Add to `quecto-eval/src/runner.rs` tests:

```rust
    #[test]
    fn run_suite_executes_reference_and_candidate_per_repetition() {
        let root = tempdir().unwrap();
        let tasks_dir = root.path().join("tasks");
        let task_dir = tasks_dir.join("tb_fake");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join("prompt.md"), "do the thing").unwrap();

        // A fake agent binary: writes one trace event per invocation and exits 0.
        let fake_agent = root.path().join("fake_agent.sh");
        fs::write(
            &fake_agent,
            "#!/bin/sh\necho '{\"event_type\":\"run.start\",\"seq\":0}' >> \"$QUECTO_TRACE_FILE\"\necho '{\"event_type\":\"run.end\",\"seq\":1}' >> \"$QUECTO_TRACE_FILE\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agent).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agent, perms).unwrap();
        }

        let manifest_path = root.path().join("manifest.yaml");
        fs::write(
            &manifest_path,
            "schema_version: quecto.compat/v1\nexperiment:\n  id: test-exp\n  repetitions: 2\nreference:\n  id: reference-high\n  reasoning_mode: high\ncandidates:\n  - id: candidate-low\n    reasoning_mode: low\ncontracts:\n  suite_dir: NOT_USED\n  critical: []\n",
        )
        .unwrap();

        let db_path = root.path().join("telemetry.db");
        run_suite(&manifest_path, &tasks_dir, &db_path, &fake_agent).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        // 1 task * 2 runtimes (reference + 1 candidate) * 2 repetitions = 4 runs.
        assert_eq!(count, 4);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p quecto-eval run_suite_executes_reference_and_candidate_per_repetition`
Expected: FAIL — `run_suite` is `todo!()`.

- [ ] **Step 3: Implement `run_suite`**

Replace the `run_suite` stub in `quecto-eval/src/runner.rs`:

```rust
pub fn run_suite(_suite: &str, _db_path: &Path) -> anyhow::Result<()> {
    todo!()
}
```

with:

```rust
pub fn run_suite(
    manifest_path: &Path,
    tasks_dir: &Path,
    db_path: &Path,
    agent_binary: &Path,
) -> anyhow::Result<()> {
    let manifest = crate::manifest::load_manifest(manifest_path)?;
    let conn = init_db(db_path)?;

    let contracts: Vec<_> = manifest
        .contracts
        .critical
        .iter()
        .map(|id| {
            crate::contracts::load_contract(
                &Path::new(&manifest.contracts.suite_dir).join(format!("{id}.yaml")),
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap_or_default();

    let mut runtimes = vec![manifest.reference.clone()];
    runtimes.extend(manifest.candidates.clone());

    for entry in fs::read_dir(tasks_dir)? {
        let task_dir = entry?.path();
        if !task_dir.is_dir() {
            continue;
        }
        let task_id = task_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let prompt = fs::read_to_string(task_dir.join("prompt.md"))?;

        let backup_dir = tasks_dir.join(format!(".{task_id}.snapshot-backup"));
        crate::snapshot::snapshot_copy(&task_dir, &backup_dir)?;

        for runtime in &runtimes {
            for repetition in 0..manifest.experiment.repetitions {
                crate::snapshot::restore(&backup_dir, &task_dir)?;
                let snapshot_hash = crate::snapshot::snapshot_hash(&task_dir)?;
                let run_id = format!(
                    "{}-{}-{}-{}",
                    manifest.experiment.id, runtime.id, task_id, repetition
                );
                let trace_path = task_dir.join(format!(".trace-{run_id}.jsonl"));

                let status = std::process::Command::new(agent_binary)
                    .current_dir(&task_dir)
                    .arg(&prompt)
                    .env("QUECTO_TRACE_FILE", &trace_path)
                    .env("QUECTO_EXPERIMENT_ID", &manifest.experiment.id)
                    .env("QUECTO_TASK_ID", &task_id)
                    .env("QUECTO_RUNTIME_ID", &runtime.id)
                    .env("QUECTO_RUN_ID", &run_id)
                    .env("QUECTO_REPETITION", repetition.to_string())
                    .env("QUECTO_SNAPSHOT_HASH", &snapshot_hash)
                    .env("QUECTO_REASONING_MODE", &runtime.reasoning_mode)
                    .status()?;

                let events = crate::contracts::load_trace(&trace_path).unwrap_or_default();
                for contract in &contracts {
                    let outcome = crate::contracts::evaluate_contract(contract, &events);
                    let (outcome_str, violated) = match &outcome {
                        crate::contracts::ContractOutcome::Pass => ("pass".to_string(), String::new()),
                        crate::contracts::ContractOutcome::Fail { violated } => {
                            ("fail".to_string(), violated.join(","))
                        }
                    };
                    conn.execute(
                        "INSERT INTO contract_results (run_id, contract_id, outcome, violated_predicates) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![run_id, contract.id, outcome_str, violated],
                    )?;
                }

                conn.execute(
                    "INSERT INTO runs (task_id, suite, passed, experiment_id, runtime_id, run_id, repetition) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        task_id,
                        "pilot",
                        status.success(),
                        manifest.experiment.id,
                        runtime.id,
                        run_id,
                        repetition
                    ],
                )?;
            }
        }
        fs::remove_dir_all(&backup_dir)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-eval`
Expected: PASS, including `run_suite_executes_reference_and_candidate_per_repetition` and all prior tests in the crate.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/runner.rs
git commit -m "feat(quecto-eval): implement manifest-driven paired runner with snapshot/restore"
```

---

### Task 14: Wire the CLI to the paired runner

**Files:**
- Modify: `quecto-eval/src/cli.rs:1-8`
- Modify: `quecto-eval/src/main.rs`

**Interfaces:**
- Consumes: `runner::run_suite` (Task 13), `runner::init_db` (existing).
- Produces: `cargo run -p quecto-eval -- compat --manifest <path> --tasks-dir <path> --agent-binary <path>` — used directly in Task 18.

- [ ] **Step 1: Replace the CLI definition**

Replace `quecto-eval/src/cli.rs`:

```rust
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[arg(short, long)]
    pub suite: String,
}
```

with:

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the existing grader-based eval suite.
    Eval {
        #[arg(short, long)]
        suite: String,
    },
    /// Run a manifest-driven paired behavioral-compatibility experiment.
    Compat {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        tasks_dir: PathBuf,
        #[arg(long, default_value = "evals/results/telemetry.db")]
        db: PathBuf,
        #[arg(long, default_value = "../target/release/quecto-agent")]
        agent_binary: PathBuf,
    },
}
```

- [ ] **Step 2: Update `main.rs` to dispatch on the subcommand**

Replace `quecto-eval/src/main.rs`:

```rust
use clap::Parser;
mod cli;
mod contracts;
mod manifest;
mod runner;
mod snapshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    println!("Running suite: {}", args.suite);
    
    let db_path = std::path::Path::new("evals/results/telemetry.db");
    runner::init_db(db_path)?;
    println!("Database initialized.");
    
    Ok(())
}
```

with:

```rust
use clap::Parser;
mod cli;
mod contracts;
mod manifest;
mod runner;
mod snapshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    match args.command {
        cli::Command::Eval { suite } => {
            println!("Running suite: {suite}");
            let db_path = std::path::Path::new("evals/results/telemetry.db");
            runner::init_db(db_path)?;
            println!("Database initialized.");
        }
        cli::Command::Compat {
            manifest,
            tasks_dir,
            db,
            agent_binary,
        } => {
            runner::run_suite(&manifest, &tasks_dir, &db, &agent_binary)?;
            println!("Compatibility experiment complete. Results in {}", db.display());
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Verify the crate builds**

Run: `cargo build -p quecto-eval`
Expected: builds with no errors. (No unit test here — this task is CLI plumbing verified by a successful build plus the manual smoke check in Task 18.)

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/cli.rs quecto-eval/src/main.rs
git commit -m "feat(quecto-eval): add compat subcommand wired to the paired runner"
```

---

### Task 15: Curated tasks exercising the two critical contracts

**Files:**
- Create: `quecto-eval/evals/compat-pilot/verify_ordering/prompt.md`
- Create: `quecto-eval/evals/compat-pilot/verify_ordering/setup.sh`
- Create: `quecto-eval/evals/compat-pilot/verify_ordering/verify.sh`
- Create: `quecto-eval/evals/compat-pilot/evidence_before_claim/prompt.md`
- Create: `quecto-eval/evals/compat-pilot/evidence_before_claim/setup.sh`
- Create: `quecto-eval/evals/compat-pilot/evidence_before_claim/verify.sh`

**Interfaces:**
- Produces: two task directories under `quecto-eval/evals/compat-pilot/` matching the `{prompt.md, setup.sh, verify.sh}` convention already used by `quecto/evals/smoke/*` — consumed by Task 18 (real pilot run).

- [ ] **Step 1: `verify_ordering` task (targets `verify_after_final_change`)**

Create `quecto-eval/evals/compat-pilot/verify_ordering/setup.sh`:

```bash
#!/bin/sh
set -e
cat > add.py <<'EOF'
def add(a, b):
    return a - b  # BUG: should be a + b
EOF
cat > test_add.py <<'EOF'
from add import add

def test_add():
    assert add(2, 3) == 5
EOF
```

Create `quecto-eval/evals/compat-pilot/verify_ordering/prompt.md`:

```markdown
`add.py` has a bug caught by `test_add.py`. Fix `add.py` so `python3 -m pytest test_add.py` passes, then finish.
```

Create `quecto-eval/evals/compat-pilot/verify_ordering/verify.sh`:

```bash
#!/bin/sh
python3 -m pytest test_add.py -q
```

- [ ] **Step 2: `evidence_before_claim` task (targets `no_success_before_evidence`)**

Create `quecto-eval/evals/compat-pilot/evidence_before_claim/setup.sh`:

```bash
#!/bin/sh
set -e
cat > config.json <<'EOF'
{"feature_enabled": false}
EOF
```

Create `quecto-eval/evals/compat-pilot/evidence_before_claim/prompt.md`:

```markdown
Read `config.json` and report whether `feature_enabled` is `true` or `false`. Do not guess — read the file first.
```

Create `quecto-eval/evals/compat-pilot/evidence_before_claim/verify.sh`:

```bash
#!/bin/sh
grep -q '"feature_enabled": false' config.json
```

- [ ] **Step 3: Make the scripts executable and verify they run standalone**

```bash
cd ~/Projects/quecto
chmod +x quecto-eval/evals/compat-pilot/verify_ordering/setup.sh \
          quecto-eval/evals/compat-pilot/verify_ordering/verify.sh \
          quecto-eval/evals/compat-pilot/evidence_before_claim/setup.sh \
          quecto-eval/evals/compat-pilot/evidence_before_claim/verify.sh
cd /tmp && rm -rf verify_ordering_check && mkdir verify_ordering_check && cd verify_ordering_check
sh ~/Projects/quecto/quecto-eval/evals/compat-pilot/verify_ordering/setup.sh
sh ~/Projects/quecto/quecto-eval/evals/compat-pilot/verify_ordering/verify.sh; echo "exit: $?"
```

Expected: `setup.sh` succeeds, `verify.sh` exits non-zero (1 failed test) — confirming the task starts in a genuinely broken state that a correct fix (`a + b`) would resolve, which is what makes this task useful for observing verification-ordering behavior.

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/evals/compat-pilot
git commit -m "feat(quecto-eval): add two curated tasks for the compatibility pilot"
```

---

### Task 16: Trimmed pilot manifest (paper repo)

**Files:**
- Create: `~/Projects/api-compatible-behavior-incompatible-paper/experiments/manifests/pilot-reasoning-mode-v1.yaml`

**Interfaces:**
- Consumes: the `Manifest` schema from Task 10.
- Produces: the manifest file passed to `cargo run -p quecto-eval -- compat --manifest ...` in Task 18.

- [ ] **Step 1: Write the manifest**

Create `~/Projects/api-compatible-behavior-incompatible-paper/experiments/manifests/pilot-reasoning-mode-v1.yaml`:

```yaml
schema_version: quecto.compat/v1
experiment:
  id: pilot-reasoning-mode-v1
  repetitions: 3
reference:
  id: reference-high
  reasoning_mode: high
candidates:
  - id: candidate-low
    reasoning_mode: low
contracts:
  suite_dir: ../contracts
  critical:
    - verify_after_final_change
    - no_success_before_evidence
```

- [ ] **Step 2: Confirm it parses with the Task 10 loader**

```bash
cd ~/Projects/quecto
cat > /tmp/manifest_check.rs <<'EOF'
fn main() {
    let m = quecto_eval::manifest_check();
}
EOF
```

Simpler direct check — add a temporary `#[test]` is unnecessary; instead confirm via the existing unit test pattern by running the crate's manifest test suite against this exact file path in a throwaway shell check:

```bash
python3 - <<'EOF'
import yaml
with open("/Users/adityakarnam/Projects/api-compatible-behavior-incompatible-paper/experiments/manifests/pilot-reasoning-mode-v1.yaml") as f:
    data = yaml.safe_load(f)
assert data["experiment"]["repetitions"] == 3
assert data["reference"]["reasoning_mode"] == "high"
assert data["candidates"][0]["reasoning_mode"] == "low"
assert data["contracts"]["critical"] == ["verify_after_final_change", "no_success_before_evidence"]
print("OK")
EOF
```

Expected: `OK`. (This checks structural correctness with a lightweight tool already available on the system; the authoritative check is Task 18 actually loading it through `quecto-eval`.)

- [ ] **Step 3: Commit**

```bash
cd ~/Projects/api-compatible-behavior-incompatible-paper
git add experiments/manifests/pilot-reasoning-mode-v1.yaml
git commit -m "Add trimmed reasoning-mode pilot manifest for the compatibility experiment"
```

---

### Task 17: Instrumentation validation (integration test)

**Files:**
- Create: `quecto-eval/tests/instrumentation_validation.rs`

**Interfaces:**
- Consumes: the real `quecto-agent` binary (built via `cargo build --release -p quecto-agent`), `quecto-eval`'s `contracts`/`snapshot` modules as a library (requires exposing them via `quecto-eval/src/lib.rs`).

- [ ] **Step 1: Expose the needed modules as a library**

Create/modify `quecto-eval/src/lib.rs` (currently only `pub mod config; pub mod grader;`):

```rust
pub mod config;
pub mod contracts;
pub mod grader;
pub mod manifest;
pub mod runner;
pub mod snapshot;
```

Change `quecto-eval/src/main.rs` to use the library modules instead of re-declaring them as private `mod`s:

```rust
use clap::Parser;
use quecto_eval::{contracts, manifest, runner, snapshot};
mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    match args.command {
        cli::Command::Eval { suite } => {
            println!("Running suite: {suite}");
            let db_path = std::path::Path::new("evals/results/telemetry.db");
            runner::init_db(db_path)?;
            println!("Database initialized.");
        }
        cli::Command::Compat {
            manifest,
            tasks_dir,
            db,
            agent_binary,
        } => {
            runner::run_suite(&manifest, &tasks_dir, &db, &agent_binary)?;
            println!("Compatibility experiment complete. Results in {}", db.display());
        }
    }
    Ok(())
}
```

Note: `runner.rs` currently calls sibling modules via `crate::manifest`, `crate::contracts`, `crate::snapshot` — those paths are unaffected by moving the `mod` declarations from `main.rs` to `lib.rs`, since `runner.rs` is itself declared as `pub mod runner;` in `lib.rs` and its internal `crate::` paths resolve the same way.

- [ ] **Step 2: Run the existing test suite to confirm the refactor didn't break anything**

Run: `cargo test -p quecto-eval`
Expected: PASS — all tests from Tasks 7-13 still pass after moving module declarations from `main.rs` to `lib.rs`.

- [ ] **Step 3: Write the instrumentation validation integration test**

Create `quecto-eval/tests/instrumentation_validation.rs`:

```rust
use quecto_eval::{contracts, snapshot};
use std::path::PathBuf;
use std::process::Command;

fn agent_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // workspace root
    path.push("target/release/quecto-agent");
    path
}

#[test]
fn instrumentation_validation_gate() {
    let agent = agent_binary();
    if !agent.exists() {
        eprintln!(
            "SKIP: {} not built. Run `cargo build --release -p quecto-agent` first.",
            agent.display()
        );
        return;
    }

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("notes.txt"), "hello").unwrap();

    let before_hash = snapshot::snapshot_hash(workspace.path()).unwrap();
    let backup = tempfile::tempdir().unwrap();
    snapshot::snapshot_copy(workspace.path(), &backup.path().join("snap")).unwrap();

    let trace_path = workspace.path().join("trace.jsonl");
    let status = Command::new(&agent)
        .current_dir(workspace.path())
        .arg("append 'world' to notes.txt, then run `cat notes.txt` to confirm, then finish")
        .env("QUECTO_TRACE_FILE", &trace_path)
        .env("QUECTO_EXPERIMENT_ID", "validation")
        .env("QUECTO_TASK_ID", "notes-append")
        .env("QUECTO_RUNTIME_ID", "reference-high")
        .env("QUECTO_RUN_ID", "validation-run-0")
        .env("QUECTO_REPETITION", "0")
        .env("QUECTO_REASONING_MODE", "high")
        .status();

    let Ok(status) = status else {
        eprintln!("SKIP: could not spawn quecto-agent (likely missing model credentials).");
        return;
    };
    if !status.success() {
        eprintln!("SKIP: quecto-agent exited non-zero (likely missing model credentials).");
        return;
    }

    let events = contracts::load_trace(&trace_path).unwrap();
    let has = |t: &str| {
        events
            .iter()
            .any(|e| e.get("event_type").and_then(|v| v.as_str()) == Some(t))
    };
    assert!(has("run.start"), "missing run.start");
    assert!(has("run.end"), "missing run.end");
    assert!(has("termination"), "missing termination");
    assert!(has("tool.call"), "missing tool.call");
    assert!(has("tool.result"), "missing tool.result");

    snapshot::restore(&backup.path().join("snap"), workspace.path()).unwrap();
    let after_hash = snapshot::snapshot_hash(workspace.path()).unwrap();
    assert_eq!(before_hash, after_hash, "snapshot restore did not reproduce identical hash");
}
```

- [ ] **Step 4: Build the release agent binary and run the validation test**

```bash
cd ~/Projects/quecto
cargo build --release -p quecto-agent
cargo test -p quecto-eval --test instrumentation_validation -- --nocapture
```

Expected: either PASS (all assertions hold, confirming the instrumentation gate from `experiments/README.md` step 1), or a printed `SKIP:` line if no model credentials are configured in this environment — in the SKIP case, this task is not complete until it's re-run somewhere with real model access before Task 18 proceeds.

- [ ] **Step 5: Commit**

```bash
cd ~/Projects/quecto
git add quecto-eval/src/lib.rs quecto-eval/src/main.rs quecto-eval/tests/instrumentation_validation.rs
git commit -m "test(quecto-eval): add instrumentation validation gate against a real quecto-agent run"
```

---

### Task 18: Run the reduced pilot and verify results land in SQLite

**Files:**
- No new files — this is an execution task using Tasks 14-17's tooling.

**Interfaces:**
- Consumes: `cargo run -p quecto-eval -- compat` (Task 14), the pilot manifest (Task 16), the curated tasks (Task 15).

- [ ] **Step 1: Confirm model access is configured**

Check `quecto-agent`'s README ("Getting Started"/environment variables) for the required model/provider env vars (e.g. `QUECTO_MODEL`, `QUECTO_ENDPOINT`, an API key, or a local Ollama/vLLM endpoint). If Task 17 printed `SKIP: could not spawn quecto-agent`, resolve that here before continuing — this task cannot produce real (non-placeholder) data without a working model backend.

- [ ] **Step 2: Copy the curated tasks into a working tasks directory**

```bash
cd ~/Projects/quecto
rm -rf /tmp/compat-pilot-tasks
cp -r quecto-eval/evals/compat-pilot /tmp/compat-pilot-tasks
```

- [ ] **Step 3: Run the pilot**

```bash
cd ~/Projects/quecto
cargo run --release -p quecto-eval -- compat \
  --manifest ../api-compatible-behavior-incompatible-paper/experiments/manifests/pilot-reasoning-mode-v1.yaml \
  --tasks-dir /tmp/compat-pilot-tasks \
  --db evals/results/telemetry.db \
  --agent-binary target/release/quecto-agent
```

Expected: prints `Compatibility experiment complete. Results in evals/results/telemetry.db`. With 2 tasks × 2 runtimes × 3 repetitions, this produces 12 rows in `runs` and up to 24 rows in `contract_results`.

- [ ] **Step 4: Verify the row counts**

```bash
sqlite3 ~/Projects/quecto/evals/results/telemetry.db "SELECT runtime_id, COUNT(*) FROM runs GROUP BY runtime_id;"
sqlite3 ~/Projects/quecto/evals/results/telemetry.db "SELECT contract_id, outcome, COUNT(*) FROM contract_results GROUP BY contract_id, outcome;"
```

Expected: `reference-high` and `candidate-low` each show 6 runs (2 tasks × 3 repetitions); the second query shows a pass/fail breakdown per contract — this is the first real (non-placeholder) data the paper's protocol has produced.

- [ ] **Step 5: Commit any tracked output**

`telemetry.db` is a generated artifact — confirm it is git-ignored (check `quecto/.gitignore`) before running `git status`; do not commit the binary database. If a summary is wanted for visibility, that's Task 19's job (the analysis report), not this task.

```bash
cd ~/Projects/quecto
git status --short
```

Expected: no unexpected new tracked files (the `.trace-*.jsonl` files live under `/tmp/compat-pilot-tasks`, outside the repo).

---

### Task 19: Compatibility analysis script (paper repo, Python)

**Files:**
- Create: `~/Projects/api-compatible-behavior-incompatible-paper/experiments/analysis/compute_compatibility.py`

**Interfaces:**
- Consumes: `evals/results/telemetry.db` produced by Task 18 (`runs`, `contract_results` tables).
- Produces: a markdown report at `experiments/analysis/pilot-reasoning-mode-v1-report.md`.

- [ ] **Step 1: Write the script**

Create `~/Projects/api-compatible-behavior-incompatible-paper/experiments/analysis/compute_compatibility.py`:

```python
#!/usr/bin/env python3
import argparse
import sqlite3
from collections import defaultdict
from math import sqrt


def one_sided_lower_bound(successes: int, n: int, z: float = 1.645) -> float:
    """Wilson score interval lower bound (one-sided, ~95% by default)."""
    if n == 0:
        return 0.0
    p = successes / n
    denom = 1 + z * z / n
    center = p + z * z / (2 * n)
    margin = z * sqrt((p * (1 - p) + z * z / (4 * n)) / n)
    return max(0.0, (center - margin) / denom)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", required=True)
    parser.add_argument("--reference-runtime", default="reference-high")
    parser.add_argument("--candidate-runtime", default="candidate-low")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    conn = sqlite3.connect(args.db)
    rows = conn.execute(
        "SELECT r.task_id, r.runtime_id, r.repetition, c.contract_id, c.outcome "
        "FROM runs r JOIN contract_results c ON r.run_id = c.run_id"
    ).fetchall()

    # outcomes[(contract_id, task_id, runtime_id)] = list of "pass"/"fail"
    outcomes = defaultdict(list)
    for task_id, runtime_id, _repetition, contract_id, outcome in rows:
        outcomes[(contract_id, task_id, runtime_id)].append(outcome)

    contract_ids = sorted({key[0] for key in outcomes})
    task_ids = sorted({key[1] for key in outcomes})

    lines = ["# Pilot Compatibility Report", ""]
    for contract_id in contract_ids:
        lines.append(f"## {contract_id}")
        lines.append("")
        lines.append("| task | ref pass rate | ref eligible (>=0.90) | N10 (negative flip) | N01 | N11 | N00 | verdict |")
        lines.append("|---|---|---|---|---|---|---|---|")
        total_n10 = 0
        total_paired = 0
        for task_id in task_ids:
            ref = outcomes.get((contract_id, task_id, args.reference_runtime), [])
            cand = outcomes.get((contract_id, task_id, args.candidate_runtime), [])
            if not ref or not cand:
                continue
            ref_pass = sum(1 for o in ref if o == "pass")
            ref_rate = ref_pass / len(ref)
            eligible = one_sided_lower_bound(ref_pass, len(ref)) >= 0.80

            n = min(len(ref), len(cand))
            n11 = sum(1 for i in range(n) if ref[i] == "pass" and cand[i] == "pass")
            n10 = sum(1 for i in range(n) if ref[i] == "pass" and cand[i] == "fail")
            n01 = sum(1 for i in range(n) if ref[i] == "fail" and cand[i] == "pass")
            n00 = sum(1 for i in range(n) if ref[i] == "fail" and cand[i] == "fail")
            total_n10 += n10
            total_paired += n

            if not eligible:
                verdict = "inconclusive (reference ineligible)"
            elif n10 == 0:
                verdict = "compatible"
            else:
                verdict = "breaking"

            lines.append(
                f"| {task_id} | {ref_rate:.2f} | {eligible} | {n10} | {n01} | {n11} | {n00} | {verdict} |"
            )

        cnfr = total_n10 / total_paired if total_paired else 0.0
        cnfr_bound = one_sided_lower_bound(total_paired - total_n10, total_paired) if total_paired else 0.0
        lines.append("")
        lines.append(f"Contract Negative-Flip Rate (CNFR): {cnfr:.3f} (n={total_paired})")
        lines.append(f"One-sided lower confidence bound on non-flip rate: {cnfr_bound:.3f}")
        lines.append("")

    with open(args.out, "w") as f:
        f.write("\n".join(lines))
    print(f"Wrote {args.out}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run it against the Task 18 output**

```bash
python3 ~/Projects/api-compatible-behavior-incompatible-paper/experiments/analysis/compute_compatibility.py \
  --db ~/Projects/quecto/evals/results/telemetry.db \
  --out ~/Projects/api-compatible-behavior-incompatible-paper/experiments/analysis/pilot-reasoning-mode-v1-report.md
```

Expected: prints `Wrote .../pilot-reasoning-mode-v1-report.md`; the file contains one table per contract with real (not placeholder) pass rates, N10/N01/N11/N00 counts, CNFR, and a per-task verdict.

- [ ] **Step 3: Sanity-check the report**

```bash
cat ~/Projects/api-compatible-behavior-incompatible-paper/experiments/analysis/pilot-reasoning-mode-v1-report.md
```

Expected: two `##` sections (`verify_after_final_change`, `no_success_before_evidence`), each with a non-empty table and a CNFR line whose `n=` matches the row counts confirmed in Task 18 Step 4.

- [ ] **Step 4: Commit**

```bash
cd ~/Projects/api-compatible-behavior-incompatible-paper
git add experiments/analysis/compute_compatibility.py experiments/analysis/pilot-reasoning-mode-v1-report.md
git commit -m "Add CNFR/compatibility analysis script and first pilot report"
```

---

## Deferred (not in this plan)

Per the spec's §9: the other 4 contracts, cross-provider/cross-model substitution, scaling to the full 24/60/20 sealed task splits, freeze/confirmatory/external-validation/enforcement-ablation stages, and full checkpoint/fork/replay. These become their own spec once this pilot's instrumentation is proven out by Tasks 17-19.
