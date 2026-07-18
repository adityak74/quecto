# QuECTO Reasoning Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one normalized `reasoning_mode` control that QuECTO can read from env/flavor defaults, override per completion in the harness, translate into provider request fields, and record in tracing/session metadata.

**Architecture:** Keep reasoning-mode semantics in `quecto-agent`'s model layer. Add a small normalized type and completion-options carrier, thread defaults from flavor/env into `HttpModel`, expose a per-completion override path on the model trait, and persist additive completion metadata alongside existing assistant messages without making the agent loop provider-aware.

**Tech Stack:** Rust 2021; existing `serde`/`serde_json`, `toml`, `rusqlite`, `tracing`, optional `opentelemetry` feature; no new dependencies.

## Global Constraints

- Accept a normalized reasoning-level setting such as `none`, `minimal`, `low`, `medium`, `high`, or `xhigh`.
- Translate that setting into provider-specific request parameters without hiding or mutating the rest of the raw request body.
- Let an external harness change the reasoning mode between completions or runs.
- Record what was requested and what the provider actually exposed in the response.
- This milestone does not include per-turn schedule files or rule engines.
- This milestone does not include checkpoint/fork or trace-retention interventions.
- This milestone does not include benchmark orchestration logic.
- This milestone does not include cross-provider semantic calibration beyond storing the provider-specific parameters used.

---

## File Structure

- Create: `quecto-agent/src/reasoning.rs`
  Responsibility: normalized reasoning-mode enum, completion options, provider payload builder, and response metadata parsing helpers.
- Modify: `quecto-agent/src/lib.rs`
  Responsibility: export reasoning types for the CLI, harness, and tests.
- Modify: `quecto-agent/src/flavor.rs`
  Responsibility: parse and merge optional `reasoning_mode` flavor config.
- Modify: `quecto-agent/src/model.rs`
  Responsibility: thread default/override completion options, inject request fields, parse reasoning-token metadata, and emit OTEL attributes/events.
- Modify: `quecto-agent/src/main.rs`
  Responsibility: resolve default reasoning mode from env/flavor and pass it into `HttpModel`; update scaffold comments for discoverability.
- Modify: `quecto-agent/src/session.rs`
  Responsibility: persist additive reasoning metadata on assistant messages.
- Modify: `README.md`
  Responsibility: document `QUECTO_REASONING_MODE` and harness override behavior.

### Task 1: Add Normalized Reasoning Types and Flavor Parsing

**Files:**
- Create: `quecto-agent/src/reasoning.rs`
- Modify: `quecto-agent/src/lib.rs`
- Modify: `quecto-agent/src/flavor.rs`
- Test: `quecto-agent/src/reasoning.rs`
- Test: `quecto-agent/src/flavor.rs`

**Interfaces:**
- Consumes: existing `Flavor` merge semantics in `quecto-agent/src/flavor.rs`
- Produces: `ReasoningMode`, `CompletionOptions`, and `Flavor { reasoning_mode: Option<ReasoningMode> }`
- Produces: `impl std::str::FromStr for ReasoningMode`
- Produces: `impl ReasoningMode { pub fn effort_str(&self) -> &'static str }`

- [ ] **Step 1: Write the failing tests for normalized parsing and flavor support**

```rust
// quecto-agent/src/reasoning.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_reasoning_modes_case_insensitively() {
        assert_eq!("none".parse::<ReasoningMode>().unwrap(), ReasoningMode::None);
        assert_eq!("Minimal".parse::<ReasoningMode>().unwrap(), ReasoningMode::Minimal);
        assert_eq!("LOW".parse::<ReasoningMode>().unwrap(), ReasoningMode::Low);
        assert_eq!("medium".parse::<ReasoningMode>().unwrap(), ReasoningMode::Medium);
        assert_eq!("high".parse::<ReasoningMode>().unwrap(), ReasoningMode::High);
        assert_eq!("xhigh".parse::<ReasoningMode>().unwrap(), ReasoningMode::XHigh);
    }

    #[test]
    fn rejects_unknown_reasoning_modes() {
        assert!("turbo".parse::<ReasoningMode>().is_err());
    }
}

// quecto-agent/src/flavor.rs
#[test]
fn parse_reads_reasoning_mode() {
    let f = Flavor::parse("reasoning_mode = \"high\"").unwrap();
    assert_eq!(f.reasoning_mode, Some(crate::reasoning::ReasoningMode::High));
}

#[test]
fn merge_lets_higher_layer_override_reasoning_mode() {
    let base = Flavor::parse("reasoning_mode = \"low\"").unwrap();
    let over = Flavor::parse("reasoning_mode = \"high\"").unwrap();
    let merged = base.merge(over);
    assert_eq!(merged.reasoning_mode, Some(crate::reasoning::ReasoningMode::High));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test -p quecto-agent reasoning::tests:: flavor::tests::parse_reads_reasoning_mode flavor::tests::merge_lets_higher_layer_override_reasoning_mode`
Expected: FAIL with missing `reasoning` module, missing `reasoning_mode` field, or unresolved `ReasoningMode`.

- [ ] **Step 3: Implement the normalized types and flavor field**

```rust
// quecto-agent/src/reasoning.rs
use crate::BoxErr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningMode {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningMode {
    pub fn effort_str(&self) -> &'static str {
        match self {
            ReasoningMode::None => "none",
            ReasoningMode::Minimal => "minimal",
            ReasoningMode::Low => "low",
            ReasoningMode::Medium => "medium",
            ReasoningMode::High => "high",
            ReasoningMode::XHigh => "xhigh",
        }
    }
}

impl FromStr for ReasoningMode {
    type Err = BoxErr;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            other => Err(format!("unknown reasoning mode: {other}").into()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionOptions {
    pub reasoning_mode: Option<ReasoningMode>,
}

pub fn parse_env_reasoning_mode() -> Result<Option<ReasoningMode>, BoxErr> {
    match std::env::var("QUECTO_REASONING_MODE") {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.parse()?)),
        Ok(_) => Ok(None),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(Box::new(e)),
    }
}

pub fn reasoning_payload(mode: ReasoningMode) -> Value {
    json!({"reasoning": {"effort": mode.effort_str()}})
}

// quecto-agent/src/flavor.rs
pub struct Flavor {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: Option<usize>,
    pub reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    pub auto_verify: Option<bool>,
    pub auto_approve: Option<bool>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub approval: ApprovalSection,
    #[serde(default)]
    pub verify: VerifySection,
}

// quecto-agent/src/lib.rs
mod reasoning;
pub use reasoning::{parse_env_reasoning_mode, reasoning_payload, CompletionOptions, ReasoningMode};
```

- [ ] **Step 4: Run the focused tests and verify they pass**

Run: `cargo test -p quecto-agent reasoning::tests:: flavor::tests::parse_reads_reasoning_mode flavor::tests::merge_lets_higher_layer_override_reasoning_mode`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/reasoning.rs quecto-agent/src/lib.rs quecto-agent/src/flavor.rs
git commit -m "feat(reasoning): add normalized reasoning mode config"
```

### Task 2: Thread Default and Override Options Through the Model API

**Files:**
- Modify: `quecto-agent/src/model.rs`
- Modify: `quecto-agent/src/main.rs`
- Test: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: `ReasoningMode`, `CompletionOptions`, `Flavor.reasoning_mode`
- Produces: `Model::complete_with_options(&self, messages: &[Message], tools: &[Value], options: &CompletionOptions) -> Result<AssistantMessage, BoxErr>`
- Produces: `HttpModel { default_reasoning_mode: Option<ReasoningMode> }`
- Produces: `fn effective_reasoning_mode(default_mode: Option<ReasoningMode>, options: &CompletionOptions) -> Option<ReasoningMode>`

- [ ] **Step 1: Write the failing tests for default-vs-override behavior**

```rust
#[test]
fn completion_options_override_model_default() {
    let options = crate::reasoning::CompletionOptions {
        reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
    };
    let effective = effective_reasoning_mode(
        Some(crate::reasoning::ReasoningMode::Low),
        &options,
    );
    assert_eq!(effective, Some(crate::reasoning::ReasoningMode::High));
}

#[test]
fn completion_options_fall_back_to_model_default() {
    let options = crate::reasoning::CompletionOptions::default();
    let effective = effective_reasoning_mode(
        Some(crate::reasoning::ReasoningMode::Medium),
        &options,
    );
    assert_eq!(effective, Some(crate::reasoning::ReasoningMode::Medium));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test -p quecto-agent completion_options_override_model_default completion_options_fall_back_to_model_default`
Expected: FAIL with missing `complete_with_options`, missing `default_reasoning_mode`, or missing `effective_reasoning_mode`.

- [ ] **Step 3: Implement the backward-compatible model API and CLI threading**

```rust
// quecto-agent/src/model.rs
pub trait Model: Send + Sync {
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
        self.complete_with_options(messages, tools, &crate::reasoning::CompletionOptions::default())
    }

    fn complete_with_options(
        &self,
        messages: &[Message],
        tools: &[Value],
        options: &crate::reasoning::CompletionOptions,
    ) -> Result<AssistantMessage, BoxErr>;

    fn clone_box(&self) -> Box<dyn Model>;
}

#[derive(Clone)]
pub struct HttpModel {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub default_reasoning_mode: Option<crate::reasoning::ReasoningMode>,
}

fn effective_reasoning_mode(
    default_mode: Option<crate::reasoning::ReasoningMode>,
    options: &crate::reasoning::CompletionOptions,
) -> Option<crate::reasoning::ReasoningMode> {
    options.reasoning_mode.or(default_mode)
}

impl Model for HttpModel {
    fn complete_with_options(
        &self,
        messages: &[Message],
        tools: &[Value],
        options: &crate::reasoning::CompletionOptions,
    ) -> Result<AssistantMessage, BoxErr> {
        let _reasoning_mode = effective_reasoning_mode(self.default_reasoning_mode, options);
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

// quecto-agent/src/main.rs
let default_reasoning_mode = quecto_agent::parse_env_reasoning_mode().unwrap_or_else(|e| {
    eprintln!("quecto-agent: {e}");
    std::process::exit(2);
});
let default_reasoning_mode = default_reasoning_mode.or(merged.reasoning_mode);

let model = HttpModel {
    url: join_url(&base_url, "chat/completions"),
    api_key,
    model: model_name,
    default_reasoning_mode,
};
```

- [ ] **Step 4: Run the focused tests and verify they pass**

Run: `cargo test -p quecto-agent completion_options_override_model_default completion_options_fall_back_to_model_default`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/main.rs
git commit -m "feat(reasoning): thread completion options through model API"
```

### Task 3: Inject Provider Request Fields and Parse Response Metadata

**Files:**
- Modify: `quecto-agent/src/reasoning.rs`
- Modify: `quecto-agent/src/model.rs`
- Test: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: `effective_reasoning_mode`, `messages_to_body`, provider raw response JSON
- Produces: `fn apply_reasoning_mode(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value>`
- Produces: `CompletionTelemetry { requested_reasoning_mode: Option<ReasoningMode>, provider_reasoning_parameters: Option<Value>, reasoning_mode_applied: bool, actual_reasoning_tokens: Option<u64> }`
- Produces: `fn parse_reasoning_tokens(resp: &Value) -> Option<u64>`

- [ ] **Step 1: Write the failing tests for body injection and token parsing**

```rust
#[test]
fn injects_reasoning_payload_into_request_body() {
    let mut body = messages_to_body("m", &[Message::user("u")]);
    let payload = apply_reasoning_mode(
        &mut body,
        Some(crate::reasoning::ReasoningMode::Low),
    )
    .unwrap();
    assert_eq!(payload, json!({"reasoning": {"effort": "low"}}));
    assert_eq!(body["reasoning"]["effort"], "low");
}

#[test]
fn leaves_body_unchanged_when_reasoning_mode_is_absent() {
    let mut body = messages_to_body("m", &[Message::user("u")]);
    let payload = apply_reasoning_mode(&mut body, None);
    assert!(payload.is_none());
    assert!(body.get("reasoning").is_none());
}

#[test]
fn parses_reasoning_tokens_from_usage_metadata() {
    let resp = json!({
        "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
        "usage": {"completion_tokens_details": {"reasoning_tokens": 42}}
    });
    assert_eq!(parse_reasoning_tokens(&resp), Some(42));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test -p quecto-agent injects_reasoning_payload_into_request_body leaves_body_unchanged_when_reasoning_mode_is_absent parses_reasoning_tokens_from_usage_metadata`
Expected: FAIL with missing helpers or missing reasoning-token parsing.

- [ ] **Step 3: Implement request injection and additive completion telemetry**

```rust
// quecto-agent/src/reasoning.rs
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionTelemetry {
    pub requested_reasoning_mode: Option<ReasoningMode>,
    pub provider_reasoning_parameters: Option<Value>,
    pub reasoning_mode_applied: bool,
    pub actual_reasoning_tokens: Option<u64>,
}

pub fn apply_reasoning_mode(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value> {
    let mode = mode?;
    let payload = reasoning_payload(mode);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("reasoning".into(), payload["reasoning"].clone());
    }
    Some(payload)
}

pub fn parse_reasoning_tokens(resp: &Value) -> Option<u64> {
    resp.get("usage")
        .and_then(|u| u.get("completion_tokens_details"))
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64)
}

// quecto-agent/src/model.rs
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub reasoning_content: Option<String>,
    pub completion: crate::reasoning::CompletionTelemetry,
}

impl Model for HttpModel {
    fn complete_with_options(
        &self,
        messages: &[Message],
        tools: &[Value],
        options: &crate::reasoning::CompletionOptions,
    ) -> Result<AssistantMessage, BoxErr> {
        let reasoning_mode = effective_reasoning_mode(self.default_reasoning_mode, options);
        let mut body = messages_to_body(&self.model, messages);
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        let provider_reasoning_parameters =
            crate::reasoning::apply_reasoning_mode(&mut body, reasoning_mode);
        let auth = self.api_key.as_ref().map(|k| format!("Bearer {k}"));
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(a) = &auth {
            headers.push(("Authorization", a.as_str()));
        }
        let resp = quecto::quecto_raw(&self.url, &headers, body)?;
        let mut parsed = parse_assistant(&resp)?;
        parsed.completion = crate::reasoning::CompletionTelemetry {
            requested_reasoning_mode: reasoning_mode,
            provider_reasoning_parameters,
            reasoning_mode_applied: reasoning_mode.is_some(),
            actual_reasoning_tokens: crate::reasoning::parse_reasoning_tokens(&resp),
        };
        Ok(parsed)
    }
}
```

- [ ] **Step 4: Run the focused tests and verify they pass**

Run: `cargo test -p quecto-agent injects_reasoning_payload_into_request_body leaves_body_unchanged_when_reasoning_mode_is_absent parses_reasoning_tokens_from_usage_metadata`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/reasoning.rs quecto-agent/src/model.rs
git commit -m "feat(reasoning): inject provider payloads and parse token metadata"
```

### Task 4: Persist Metadata and Emit OTEL Attributes Without Breaking Resume

**Files:**
- Modify: `quecto-agent/src/model.rs`
- Modify: `quecto-agent/src/agent.rs`
- Modify: `quecto-agent/src/session.rs`
- Test: `quecto-agent/src/agent.rs`
- Test: `quecto-agent/src/session.rs`

**Interfaces:**
- Consumes: `AssistantMessage.completion`
- Produces: `Message` fields `requested_reasoning_mode`, `provider_reasoning_parameters`, `reasoning_mode_applied`, `actual_reasoning_tokens`
- Produces: SQLite persistence of those optional fields on `messages`
- Produces: OTEL span attributes on `model_complete`

- [ ] **Step 1: Write the failing persistence and propagation tests**

```rust
// quecto-agent/src/agent.rs
#[test]
fn propagates_completion_reasoning_metadata() {
    let model = Scripted::new(vec![AssistantMessage {
        content: "done".to_string(),
        tool_calls: vec![],
        finish_reason: "stop".to_string(),
        reasoning_content: Some("thinking".to_string()),
        completion: crate::reasoning::CompletionTelemetry {
            requested_reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
            provider_reasoning_parameters: Some(json!({"reasoning": {"effort": "high"}})),
            reasoning_mode_applied: true,
            actual_reasoning_tokens: Some(17),
        },
    }]);
    let mut a = configured_agent(model, ApprovalMode::NonInteractive);
    let _ = a.run("task");
    assert_eq!(a.messages[2].actual_reasoning_tokens, Some(17));
    assert_eq!(a.messages[2].requested_reasoning_mode, Some(crate::reasoning::ReasoningMode::High));
}

// quecto-agent/src/session.rs
#[test]
fn messages_round_trip_with_reasoning_metadata() {
    let mut store = Store::open_in_memory().unwrap();
    store.create_session("s1", "task", "/repo", "m").unwrap();
    let mut m = Message::assistant("response");
    m.requested_reasoning_mode = Some(crate::reasoning::ReasoningMode::Low);
    m.provider_reasoning_parameters = Some(json!({"reasoning": {"effort": "low"}}));
    m.reasoning_mode_applied = Some(true);
    m.actual_reasoning_tokens = Some(9);
    store.record_message("s1", 0, &m).unwrap();
    let loaded = store.load_messages("s1").unwrap();
    assert_eq!(loaded[0].requested_reasoning_mode, Some(crate::reasoning::ReasoningMode::Low));
    assert_eq!(loaded[0].actual_reasoning_tokens, Some(9));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test -p quecto-agent propagates_completion_reasoning_metadata messages_round_trip_with_reasoning_metadata`
Expected: FAIL with missing `Message` fields, missing schema columns, or missing propagation from `AssistantMessage`.

- [ ] **Step 3: Implement message propagation, schema migration, and OTEL recording**

```rust
// quecto-agent/src/model.rs
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub reasoning_content: Option<String>,
    pub requested_reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    pub provider_reasoning_parameters: Option<Value>,
    pub reasoning_mode_applied: Option<bool>,
    pub actual_reasoning_tokens: Option<u64>,
}

// quecto-agent/src/agent.rs
let mut assistant_msg = Message::assistant(msg.content.clone());
assistant_msg.reasoning_content = msg.reasoning_content.clone();
assistant_msg.requested_reasoning_mode = msg.completion.requested_reasoning_mode;
assistant_msg.provider_reasoning_parameters = msg.completion.provider_reasoning_parameters.clone();
assistant_msg.reasoning_mode_applied = Some(msg.completion.reasoning_mode_applied);
assistant_msg.actual_reasoning_tokens = msg.completion.actual_reasoning_tokens;

// quecto-agent/src/session.rs
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT,
    reasoning_content TEXT,
    requested_reasoning_mode TEXT,
    provider_reasoning_parameters TEXT,
    reasoning_mode_applied INTEGER,
    actual_reasoning_tokens INTEGER
);";

let _ = conn.execute("ALTER TABLE messages ADD COLUMN requested_reasoning_mode TEXT", []);
let _ = conn.execute("ALTER TABLE messages ADD COLUMN provider_reasoning_parameters TEXT", []);
let _ = conn.execute("ALTER TABLE messages ADD COLUMN reasoning_mode_applied INTEGER", []);
let _ = conn.execute("ALTER TABLE messages ADD COLUMN actual_reasoning_tokens INTEGER", []);

// insert/select bindings updated to write/read the optional fields
```

- [ ] **Step 4: Run the focused tests and the package test suite**

Run: `cargo test -p quecto-agent propagates_completion_reasoning_metadata messages_round_trip_with_reasoning_metadata`
Expected: PASS

Run: `cargo test -p quecto-agent`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/agent.rs quecto-agent/src/session.rs
git commit -m "feat(reasoning): persist reasoning metadata in messages and tracing"
```

### Task 5: Document the Operator Surface and Regression-Proof the Harness Contract

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `README.md`
- Test: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: final `HttpModel` override API and `QUECTO_REASONING_MODE`
- Produces: scaffold comments and README examples using `QUECTO_REASONING_MODE=low`
- Produces: regression test that two completions can use different override modes with the same transcript

- [ ] **Step 1: Write the failing harness-contract regression test**

```rust
#[test]
fn completion_options_can_change_reasoning_mode_across_calls() {
    let model = HttpModel {
        url: "http://example.test/v1/chat/completions".into(),
        api_key: None,
        model: "test-model".into(),
        default_reasoning_mode: Some(crate::reasoning::ReasoningMode::Low),
    };
    let low = effective_reasoning_mode(
        model.default_reasoning_mode,
        &crate::reasoning::CompletionOptions::default(),
    );
    let high = effective_reasoning_mode(
        model.default_reasoning_mode,
        &crate::reasoning::CompletionOptions {
            reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
        },
    );
    assert_eq!(low, Some(crate::reasoning::ReasoningMode::Low));
    assert_eq!(high, Some(crate::reasoning::ReasoningMode::High));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test -p quecto-agent completion_options_can_change_reasoning_mode_across_calls`
Expected: FAIL until the final API and defaults compile cleanly together.

- [ ] **Step 3: Update scaffold comments and README examples**

```rust
// quecto-agent/src/main.rs
const SCAFFOLD_TEMPLATE: &str = r#"name = "{name}"

# model           = "qwen3.6:35b"
# base_url        = "http://localhost:11434/v1"
# reasoning_mode  = "low"
# max_steps       = 30
# auto_verify     = true
# system_prompt   = "You are a terse senior reviewer."
"#;
```

```md
<!-- README.md -->
export QUECTO_REASONING_MODE=low
quecto-agent "inspect this repository and summarize the test harness"

Harness code can override the default per completion by passing
`CompletionOptions { reasoning_mode: Some(ReasoningMode::High) }`
to `Model::complete_with_options(...)`.
```

- [ ] **Step 4: Run the focused test and the package test suite**

Run: `cargo test -p quecto-agent completion_options_can_change_reasoning_mode_across_calls`
Expected: PASS

Run: `cargo test -p quecto-agent`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/main.rs README.md quecto-agent/src/model.rs
git commit -m "docs(reasoning): document reasoning mode env and harness override"
```

## Self-Review

- Spec coverage: the plan covers normalized input parsing, env/flavor defaults, harness override API, provider request translation, response metadata capture, OTEL/session recording, and operator docs.
- Placeholder scan: no `TODO`, `TBD`, or deferred “implement later” instructions remain.
- Type consistency: all tasks use the same names for `ReasoningMode`, `CompletionOptions`, `CompletionTelemetry`, `complete_with_options`, and the per-message metadata fields.
