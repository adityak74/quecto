# OpenTelemetry Tracing Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement OpenTelemetry (OTEL) support to trace the main loop, model completions (including reasoning content), and tool dispatches in `quecto-agent`.

**Architecture:** We use the standard Rust `tracing` ecosystem for lightweight code instrumentation. The heavy OpenTelemetry OTLP dependencies and initialization logic will be behind the `otel` Cargo feature flag. Inside `main.rs`, we conditionally instantiate a dedicated single-threaded `tokio` runtime to manage asynchronous span exporting without introducing an async runtime to default builds.

**Tech Stack:** Rust, `tracing` 0.1, `tracing-subscriber` 0.3, `tracing-opentelemetry` 0.22, `opentelemetry` 0.21, `opentelemetry_sdk` 0.21, `opentelemetry-otlp` 0.14, `tokio` 1.0.

## Global Constraints

- Tracing dependencies must be feature-gated behind the `otel` Cargo feature.
- Bytewise footprint optimizations must be preserved when building without the `otel` feature.
- Do not introduce any asynchronous runner or tokio dependency to the default crate build.

---

### Task 1: Add Dependencies & Feature Gate to Cargo.toml

**Files:**
- Modify: `quecto-agent/Cargo.toml`

**Interfaces:**
- Produces: `otel` Cargo feature flag with optional dependencies compile-ready.

- [ ] **Step 1: Edit Cargo.toml to add features and dependencies**

Replace the contents of `quecto-agent/Cargo.toml` to declare the feature gate:
```toml
[package]
name = "quecto-agent"
version = "0.1.0"
edition = "2021"
description = "Coding agent built on the quecto core."
license = "MIT"

[dependencies]
crossterm = "0.27"
quecto = { path = ".." }
serde_json = "1"
ignore = "0.4"
regex = "1"
ctrlc = "3"
libc = "0.2"
rusqlite = { version = "0.31", features = ["bundled"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
sha2 = "0.10"

# Optional OpenTelemetry and Tracing Dependencies
tracing = { version = "0.1", optional = true }
tracing-subscriber = { version = "0.3", features = ["env-filter", "registry"], optional = true }
tracing-opentelemetry = { version = "0.22", optional = true }
opentelemetry = { version = "0.21", features = ["trace"], optional = true }
opentelemetry_sdk = { version = "0.21", features = ["rt-tokio"], optional = true }
opentelemetry-otlp = { version = "0.14", features = ["http-proto", "reqwest-client"], optional = true }
tokio = { version = "1", features = ["rt-multi-thread"], optional = true }

[features]
default = []
otel = [
    "dep:tracing",
    "dep:tracing-subscriber",
    "dep:tracing-opentelemetry",
    "dep:opentelemetry",
    "dep:opentelemetry_sdk",
    "dep:opentelemetry-otlp",
    "dep:tokio"
]

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Run build to verify default compilation is unchanged**

Run: `cargo build -p quecto-agent`
Expected: Passes successfully with zero tracing crate dependencies compiled.

- [ ] **Step 3: Run build with otel feature enabled**

Run: `cargo build -p quecto-agent --features otel`
Expected: Passes successfully, compiling tracing, opentelemetry, and tokio.

- [ ] **Step 4: Commit**

```bash
git add quecto-agent/Cargo.toml
git commit -m "feat(otel): add optional tracing and opentelemetry dependencies"
```

---

### Task 2: Data Model Upgrades (Reasoning Content Support)

**Files:**
- Modify: `quecto-agent/src/model.rs`

**Interfaces:**
- Produces: `Message` and `AssistantMessage` containing `reasoning_content: Option<String>`.
- Produces: Tag-extraction helper `extract_think_tags(content: &str) -> (Option<String>, String)`.

- [ ] **Step 1: Add failing tests for parsing reasoning content**

Add the following unit tests at the end of `quecto-agent/src/model.rs`'s `mod tests`:
```rust
    #[test]
    fn parses_reasoning_content_field() {
        let r = json!({"choices":[{"message":{"content":"hello", "reasoning_content":"thinking 123"},"finish_reason":"stop"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.reasoning_content, Some("thinking 123".to_string()));
    }

    #[test]
    fn parses_think_tags_in_content() {
        let r = json!({"choices":[{"message":{"content":"<think>\nthinking 456\n</think>\nhello"},"finish_reason":"stop"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.reasoning_content, Some("thinking 456".to_string()));
    }
```

- [ ] **Step 2: Verify the new tests fail**

Run: `cargo test -p quecto-agent`
Expected: Compile error because `reasoning_content` is missing from `AssistantMessage` structure.

- [ ] **Step 3: Implement data model changes and helper**

Modify `quecto-agent/src/model.rs` to include the `reasoning_content` fields, serialize them, and update `parse_assistant`:
```rust
// In Structs (lines ~5 and ~68)
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub reasoning_content: Option<String>, // added field
}

// In Message constructors, set reasoning_content to None (or handle it)
impl Message {
    fn plain(role: &str, content: impl Into<String>) -> Self {
        Message {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(c: impl Into<String>) -> Self {
        Message {
            role: "assistant".into(),
            content: c.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message {
            role: "assistant".into(),
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message {
            role: "tool".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub reasoning_content: Option<String>, // added field
}

// Helper to extract think tags
pub fn extract_think_tags(content: &str) -> (Option<String>, String) {
    if let (Some(start), Some(end)) = (content.find("<think>"), content.find("</think>")) {
        if start < end {
            let reasoning = content[start + 7..end].trim().to_string();
            let cleaned_content = format!("{}{}", &content[..start], &content[end + 8..]).trim().to_string();
            return (Some(reasoning), cleaned_content);
        }
    }
    (None, content.to_string())
}

// In parse_assistant:
pub fn parse_assistant(resp: &Value) -> Result<AssistantMessage, BoxErr> {
    let choice = resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or("no choices in response")?;
    let message = choice.get("message").ok_or("no message in choice")?;
    let content_raw = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("")
        .to_string();

    let mut reasoning_content = message
        .get("reasoning_content")
        .or_else(|| message.get("thinking"))
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    let (extracted_reasoning, content) = extract_think_tags(&content_raw);
    if reasoning_content.is_none() {
        reasoning_content = extracted_reasoning;
    }

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for call in calls {
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let func = call.get("function").ok_or("tool_call missing function")?;
            let name = func
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    Ok(AssistantMessage {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
    })
}
```

- [ ] **Step 4: Verify all tests pass**

Run: `cargo test -p quecto-agent`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/model.rs
git commit -m "feat(otel): parse and store reasoning content in assistant responses"
```

---

### Task 3: Instrument Agent Loop and Model Client

**Files:**
- Modify: `quecto-agent/src/agent.rs`
- Modify: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: `tracing` macros (`span!`, `event!`, etc.).
- Produces: Instrumentation spans on run, steps, model completion, and tool dispatches.

- [ ] **Step 1: Instrument HttpModel completion**

Modify `complete` inside `quecto-agent/src/model.rs` to wrap LLM endpoint calls:
```rust
impl Model for HttpModel {
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "model_complete",
            quecto.model = self.model,
            quecto.messages_sent = messages.len(),
            quecto.tools_provided = tools.len()
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

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
        let parsed = parse_assistant(&resp);
        
        #[cfg(feature = "otel")]
        if let Ok(msg) = &parsed {
            if let Some(reasoning) = &msg.reasoning_content {
                tracing::event!(tracing::Level::INFO, name = "model_thinking", content = %reasoning);
            }
            if !msg.content.is_empty() {
                tracing::event!(tracing::Level::INFO, name = "model_response", content = %msg.content);
            }
        }
        
        parsed
    }
}
```

- [ ] **Step 2: Instrument Agent run loop**

Modify `quecto-agent/src/agent.rs` to declare traces:
```rust
    pub fn run(&mut self, task: &str) -> Outcome {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "agent_run",
            quecto.task = task,
            quecto.max_steps = self.max_steps
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

        self.messages.push(Message::user(task));
        self.run_loop()
    }

    pub fn resume(&mut self) -> Outcome {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "agent_run",
            quecto.max_steps = self.max_steps
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

        self.run_loop()
    }
```

And instrument each step iteration and tool dispatch inside `run_loop`:
```rust
    fn run_loop(&mut self) -> Outcome {
        let schemas = self.registry.schemas();
        let mut step = 0;
        let mut repeats = RepeatGuard::default();
        let mut failed_verify_changes: Option<usize> = None;
        let mut failed_verify_attempts = 0;
        let mut denial_streak = 0usize;
        let outcome = loop {
            self.sync();
            if step >= self.max_steps {
                break Outcome::StepLimit;
            }
            if self.cancel.load(Ordering::SeqCst) {
                break Outcome::Cancelled;
            }

            #[cfg(feature = "otel")]
            let step_span = tracing::span!(
                tracing::Level::INFO,
                "agent_step",
                quecto.step_number = step
            );
            #[cfg(feature = "otel")]
            let _step_guard = step_span.enter();

            self.renderer.working();
            let completed = self.model.complete(&self.messages, &schemas);
            self.renderer.working_done();
            let msg = match completed {
                Ok(m) => m,
                Err(e) => break Outcome::Error(e),
            };
            self.messages.push(Message::assistant_with_calls(
                msg.content.clone(),
                msg.tool_calls.clone(),
            ));
            if msg.tool_calls.is_empty() {
                // ... verification logic ...
                break Outcome::Complete(msg.content);
            }
            let mut stop: Option<Outcome> = None;
            for call in &msg.tool_calls {
                if self.cancel.load(Ordering::SeqCst) {
                    stop = Some(Outcome::Cancelled);
                    break;
                }

                #[cfg(feature = "otel")]
                let tool_span = tracing::span!(
                    tracing::Level::INFO,
                    "tool_execute",
                    quecto.tool_name = call.name,
                    quecto.tool_arguments = %call.arguments,
                    quecto.tool_summary = tracing::field::Empty
                );
                #[cfg(feature = "otel")]
                let _tool_guard = tool_span.enter();

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
                self.renderer.tool(&call.name, &out.summary);
                
                #[cfg(feature = "otel")]
                {
                    tool_span.record("quecto.tool_summary", &out.summary);
                    tracing::event!(tracing::Level::INFO, name = "tool_output", content = %out.content);
                }

                if out.summary == "denied" {
                    denial_streak += 1;
                } else {
                    denial_streak = 0;
                }
                let repeated = repeats.observe(call, &out.content, self.cx.changes().len());
                self.messages
                    .push(Message::tool_result(&call.id, out.content));
                if repeated {
                    stop = Some(Outcome::RepeatedAction);
                    break;
                }
                if denial_streak >= DENIAL_STREAK_LIMIT {
                    stop = Some(Outcome::Blocked);
                    break;
                }
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

- [ ] **Step 3: Verify execution and existing test coverage passes**

Run: `cargo test -p quecto-agent` (runs default features)
Expected: PASS

Run: `cargo test -p quecto-agent --features otel`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add quecto-agent/src/agent.rs quecto-agent/src/model.rs
git commit -m "feat(otel): instrument agent loop, tool dispatch, and HTTP model calls"
```

---

### Task 4: Setup OTEL Exporter and Registry in main.rs

**Files:**
- Modify: `quecto-agent/src/main.rs`

**Interfaces:**
- Produces: Startup initialization of OpenTelemetry tracer exporter and background tokio runtime context for exporting spans when run via the CLI.

- [ ] **Step 1: Implement conditional compilation module for OTEL in main.rs**

Add the following initialization wrapper to `quecto-agent/src/main.rs` near the top:
```rust
#[cfg(feature = "otel")]
mod otel_init {
    pub struct OtelGuard {
        _rt: tokio::runtime::Runtime,
    }

    impl Drop for OtelGuard {
        fn drop(&mut self) {
            opentelemetry::global::shutdown_tracer_provider();
        }
    }

    pub fn init_otel() -> Option<OtelGuard> {
        // gRPC/HTTP OTLP exporter batch processor runs asynchronously. 
        // Create a dedicated single-threaded runtime to orchestrate exports.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .ok()?;
        
        let _guard = rt.enter();

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(opentelemetry_otlp::new_exporter().http())
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .ok()?;

        use tracing_subscriber::prelude::*;
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = tracing_subscriber::registry().with(telemetry);
        
        tracing::subscriber::set_global_default(subscriber).ok()?;

        Some(OtelGuard { _rt: rt })
    }
}
```

- [ ] **Step 2: Bind the initialization wrapper in main()**

Modify `fn main()` in `quecto-agent/src/main.rs`:
```rust
fn main() {
    #[cfg(feature = "otel")]
    let _otel_guard = otel_init::init_otel();

    let cli = Cli::parse();
    // ... rest of main logic ...
```

- [ ] **Step 3: Validate the build compiles fully**

Run: `cargo check -p quecto-agent --features otel`
Expected: Compiles with zero errors.

- [ ] **Step 4: Commit**

```bash
git add quecto-agent/src/main.rs
git commit -m "feat(otel): initialize standard OTLP HTTP pipeline in CLI main"
```
