# quecto-agent Anthropic Claude API Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `quecto-agent` a native `Provider::Anthropic` request/response path alongside the existing OpenAI-compatible one, selectable via a new `provider` flavor/CLI field, so it can talk to Claude models directly through the Anthropic Messages API (`POST /v1/messages`).

**Architecture:** A new `Provider` enum (`quecto-agent/src/provider.rs`) carries pure, unit-testable functions that translate the crate's existing `Message`/`ToolCall` transcript types to and from Anthropic's wire format (system extraction, `tool_use`/`tool_result` content blocks, `thinking` budget mapping). `HttpModel` gains a `provider: Provider` field and branches its one `complete_with_options` method on it — everything upstream of that method (the `Model` trait, `Agent`, `Flavor`, session persistence) is unaffected. `HttpModel.url` keeps its existing meaning (the exact endpoint to `POST`); callers pick the right path suffix (`chat/completions` vs `messages`) via `Provider::path_suffix()`.

**Tech Stack:** Rust, `serde_json::Value` for wire-format construction/parsing (no new struct types for request/response bodies, matching the existing `model.rs` style), `ureq` via the existing `quecto::quecto_raw` primitive (no new HTTP dependency), `toml`/`serde` for flavor config (matching `flavor.rs`/`reasoning.rs`).

## Global Constraints

- No streaming in this plan. Both providers use the existing buffered `quecto::quecto_raw` primitive; `quecto_stream` is out of scope.
- `QUECTO_API_KEY` is the only API key env var for both providers (no new `ANTHROPIC_API_KEY`).
- Anthropic auth is `x-api-key: <key>` + `anthropic-version: 2023-06-01`; **not** `Authorization: Bearer`.
- Anthropic's `max_tokens` request field is required; default `4096` when unset via config.
- `finish_reason` / Anthropic's `stop_reason` is informational only — the agent loop branches on `tool_calls.is_empty()`, not on this string (confirmed in `agent.rs`) — no cross-provider normalization needed.
- Every task must leave `cargo test -p quecto-agent` (plus `--features otel` where a task touches otel-gated code) passing before moving to the next task, because `HttpModel`'s struct literal is used at 9 call sites across the crate and any task that changes its field set must fix all of them in the same task.

---

### Task 1: `Provider` enum

**Files:**
- Create: `quecto-agent/src/provider.rs`
- Modify: `quecto-agent/src/lib.rs` (add `mod provider;` and export `Provider`)

**Interfaces:**
- Produces: `pub enum Provider { OpenAiCompatible, Anthropic }` — `Default` (`OpenAiCompatible`), `Copy + Clone + Debug + Eq + PartialEq`, `impl FromStr` (accepts `"openai"`/`"openai-compatible"`/`"openai_compatible"` → `OpenAiCompatible`, `"anthropic"`/`"claude"` → `Anthropic`, case-insensitive), `impl Deserialize`/`Serialize` via `#[serde(rename_all = "lowercase")]` with `OpenAiCompatible` renamed to `"openai"`, and a method `pub fn path_suffix(&self) -> &'static str` (`"chat/completions"` / `"messages"`).

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/provider.rs`:

```rust
use crate::BoxErr;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Which wire format `HttpModel` speaks to the configured endpoint.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    #[serde(rename = "openai")]
    OpenAiCompatible,
    Anthropic,
}

impl Provider {
    /// The path segment to append to `base_url` for this provider's completion endpoint.
    pub fn path_suffix(&self) -> &'static str {
        match self {
            Provider::OpenAiCompatible => "chat/completions",
            Provider::Anthropic => "messages",
        }
    }
}

impl FromStr for Provider {
    type Err = BoxErr;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "openai_compatible" => Ok(Self::OpenAiCompatible),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            other => Err(format!("unknown provider: {other}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_is_openai_compatible() {
        assert_eq!(Provider::default(), Provider::OpenAiCompatible);
    }

    #[test]
    fn path_suffix_differs_per_provider() {
        assert_eq!(Provider::OpenAiCompatible.path_suffix(), "chat/completions");
        assert_eq!(Provider::Anthropic.path_suffix(), "messages");
    }

    #[test]
    fn parses_known_aliases_case_insensitively() {
        for alias in ["openai", "OpenAI", "openai-compatible", "openai_compatible"] {
            assert_eq!(alias.parse::<Provider>().unwrap(), Provider::OpenAiCompatible);
        }
        for alias in ["anthropic", "Anthropic", "claude", "CLAUDE"] {
            assert_eq!(alias.parse::<Provider>().unwrap(), Provider::Anthropic);
        }
    }

    #[test]
    fn rejects_unknown_providers() {
        assert!("bedrock".parse::<Provider>().is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they compile and pass**

Run: `cargo test -p quecto-agent provider::tests -- --nocapture`
Expected: 4 tests pass (the module didn't exist before this step, so there's no "before" failure to check — this crate doesn't yet declare `mod provider;`, so verify by running the full build first: `cargo build -p quecto-agent` fails with "file not found for module `provider`" until Step 3 is done. Do Step 3 first if `cargo test` doesn't find the module.)

- [ ] **Step 3: Wire the module into `lib.rs`**

In `quecto-agent/src/lib.rs`, add the module declaration next to the other `mod` lines (after `mod policy;`):

```rust
mod policy;
mod provider;
```

Add the export next to `pub use policy::{...}`:

```rust
pub use policy::{Decision, Policy, Preset};
pub use provider::Provider;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent provider:: -- --nocapture`
Expected: `test provider::tests::default_provider_is_openai_compatible ... ok`, `path_suffix_differs_per_provider ... ok`, `parses_known_aliases_case_insensitively ... ok`, `rejects_unknown_providers ... ok`

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/provider.rs quecto-agent/src/lib.rs
git commit -m "feat(quecto-agent): add Provider enum for OpenAI-compatible vs Anthropic dispatch"
```

---

### Task 2: Anthropic request body builder

**Files:**
- Modify: `quecto-agent/src/provider.rs`

**Interfaces:**
- Consumes: `crate::model::Message` (fields: `role: String`, `content: String`, `tool_calls: Vec<ToolCall>`, `tool_call_id: Option<String>`), `crate::model::ToolCall` (fields: `id: String`, `name: String`, `arguments: Value`).
- Produces: `pub const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 4096;`, `pub fn messages_to_anthropic_body(model: &str, messages: &[crate::model::Message], max_tokens: u32) -> Value`, `pub fn tools_to_anthropic(tools: &[Value]) -> Vec<Value>`.

- [ ] **Step 1: Write the failing tests**

Add to `quecto-agent/src/provider.rs`, inside the existing `#[cfg(test)] mod tests` block (add these alongside the Task 1 tests, keep one `use super::*;` and one `use serde_json::json;` at the top of the module):

```rust
    use crate::model::{Message, ToolCall};
    use serde_json::json;

    #[test]
    fn extracts_system_message_to_top_level_field() {
        let messages = [Message::system("be terse"), Message::user("hi")];
        let body = messages_to_anthropic_body("claude-x", &messages, 4096);

        assert_eq!(body["system"], "be terse");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hi");
    }

    #[test]
    fn omits_system_field_when_no_system_message() {
        let messages = [Message::user("hi")];
        let body = messages_to_anthropic_body("claude-x", &messages, 4096);

        assert!(body.get("system").is_none());
    }

    #[test]
    fn always_includes_model_and_max_tokens() {
        let body = messages_to_anthropic_body("claude-x", &[Message::user("hi")], 2048);

        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn assistant_tool_calls_become_tool_use_blocks() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "a.rs"}),
        };
        let messages = [Message::assistant_with_calls("checking", vec![call])];
        let body = messages_to_anthropic_body("claude-x", &messages, 4096);

        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0], json!({"type": "text", "text": "checking"}));
        assert_eq!(
            content[1],
            json!({"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a.rs"}})
        );
    }

    #[test]
    fn assistant_tool_calls_with_empty_content_omit_text_block() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: json!({}),
        };
        let messages = [Message::assistant_with_calls("", vec![call])];
        let body = messages_to_anthropic_body("claude-x", &messages, 4096);

        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
    }

    #[test]
    fn tool_result_message_reroled_to_user_with_tool_result_block() {
        let messages = [Message::tool_result("call_1", "file contents")];
        let body = messages_to_anthropic_body("claude-x", &messages, 4096);

        assert_eq!(body["messages"][0]["role"], "user");
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0],
            json!({"type": "tool_result", "tool_use_id": "call_1", "content": "file contents"})
        );
    }

    #[test]
    fn converts_openai_function_tools_to_anthropic_shape() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
            }
        })];

        let converted = tools_to_anthropic(&tools);

        assert_eq!(
            converted[0],
            json!({
                "name": "read_file",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            })
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent provider:: -- --nocapture`
Expected: FAIL with `cannot find function messages_to_anthropic_body`/`tools_to_anthropic` in this scope

- [ ] **Step 3: Implement**

Add above the `#[cfg(test)]` line in `quecto-agent/src/provider.rs`:

```rust
use serde_json::{json, Value};

/// Anthropic requires `max_tokens` on every request; this is the default when
/// no flavor/CLI value is configured.
pub const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 4096;

/// Serialize the transcript into an Anthropic Messages API request body.
/// `system`-role messages are pulled out into the top-level `system` field
/// (Anthropic has no `system` role inside `messages`); tool calls become
/// `tool_use` content blocks; tool results are re-roled to `user` messages
/// carrying a `tool_result` content block.
pub fn messages_to_anthropic_body(
    model: &str,
    messages: &[crate::model::Message],
    max_tokens: u32,
) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut anthropic_messages: Vec<Value> = Vec::new();

    for m in messages {
        match m.role.as_str() {
            "system" => system_parts.push(m.content.clone()),
            "tool" => {
                let tool_use_id = m.tool_call_id.clone().unwrap_or_default();
                anthropic_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": m.content,
                    }]
                }));
            }
            "assistant" if !m.tool_calls.is_empty() => {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": m.content}));
                }
                for call in &m.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                anthropic_messages.push(json!({"role": "assistant", "content": blocks}));
            }
            _ => {
                anthropic_messages.push(json!({"role": m.role, "content": m.content}));
            }
        }
    }

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": anthropic_messages,
    });
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n\n"));
    }
    body
}

/// Convert OpenAI-shaped function tool defs
/// (`{"type":"function","function":{name,description,parameters}}`) to
/// Anthropic's flat shape (`{"name","description","input_schema"}`). Tool
/// defs that don't match the expected shape are dropped.
pub fn tools_to_anthropic(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|t| {
            let func = t.get("function")?;
            Some(json!({
                "name": func.get("name")?.clone(),
                "description": func.get("description").cloned().unwrap_or(Value::Null),
                "input_schema": func
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            }))
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent provider:: -- --nocapture`
Expected: all `provider::tests::*` tests pass, including the 7 new ones

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/provider.rs
git commit -m "feat(quecto-agent): build Anthropic Messages API request bodies"
```

---

### Task 3: Anthropic response parser

**Files:**
- Modify: `quecto-agent/src/provider.rs`

**Interfaces:**
- Consumes: `crate::model::{AssistantMessage, ModelCompletion, ToolCall}`, `crate::reasoning::CompletionTelemetry` (all defined in `model.rs`/`reasoning.rs`, unchanged).
- Produces: `pub fn parse_anthropic_completion(resp: &Value) -> Result<crate::model::ModelCompletion, crate::BoxErr>`.

- [ ] **Step 1: Write the failing tests**

Add to `quecto-agent/src/provider.rs`'s test module:

```rust
    #[test]
    fn parses_text_only_response() {
        let resp = json!({
            "content": [{"type": "text", "text": "hello there"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let completion = parse_anthropic_completion(&resp).unwrap();

        assert_eq!(completion.message.content, "hello there");
        assert!(completion.message.tool_calls.is_empty());
        assert_eq!(completion.message.finish_reason, "end_turn");
        assert!(completion.message.reasoning_content.is_none());
    }

    #[test]
    fn parses_tool_use_blocks_into_tool_calls() {
        let resp = json!({
            "content": [
                {"type": "text", "text": "checking"},
                {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.rs"}}
            ],
            "stop_reason": "tool_use"
        });

        let completion = parse_anthropic_completion(&resp).unwrap();

        assert_eq!(completion.message.content, "checking");
        assert_eq!(completion.message.finish_reason, "tool_use");
        assert_eq!(completion.message.tool_calls.len(), 1);
        assert_eq!(completion.message.tool_calls[0].id, "toolu_1");
        assert_eq!(completion.message.tool_calls[0].name, "read_file");
        assert_eq!(completion.message.tool_calls[0].arguments, json!({"path": "a.rs"}));
    }

    #[test]
    fn parses_thinking_block_into_reasoning_content() {
        let resp = json!({
            "content": [
                {"type": "thinking", "thinking": "let me think"},
                {"type": "text", "text": "answer"}
            ],
            "stop_reason": "end_turn",
            "usage": {"output_tokens": 123}
        });

        let completion = parse_anthropic_completion(&resp).unwrap();

        assert_eq!(completion.message.reasoning_content.as_deref(), Some("let me think"));
        assert_eq!(completion.message.content, "answer");
        assert!(completion.telemetry.reasoning_content_available);
        assert_eq!(completion.telemetry.actual_reasoning_tokens, Some(123));
    }

    #[test]
    fn blank_thinking_block_does_not_mark_reasoning_available() {
        let resp = json!({
            "content": [{"type": "thinking", "thinking": "  \n "}, {"type": "text", "text": "answer"}],
            "stop_reason": "end_turn"
        });

        let completion = parse_anthropic_completion(&resp).unwrap();

        assert!(completion.message.reasoning_content.is_none());
        assert!(!completion.telemetry.reasoning_content_available);
        assert!(completion.telemetry.actual_reasoning_tokens.is_none());
    }

    #[test]
    fn missing_content_array_is_an_error() {
        let resp = json!({"stop_reason": "end_turn"});

        assert!(parse_anthropic_completion(&resp).is_err());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent provider:: -- --nocapture`
Expected: FAIL with `cannot find function parse_anthropic_completion`

- [ ] **Step 3: Implement**

Add to `quecto-agent/src/provider.rs`, above the `#[cfg(test)]` line:

```rust
/// Parse an Anthropic Messages API response into a normalized `ModelCompletion`.
/// `content` blocks of type `text` are concatenated into the assistant text;
/// `tool_use` blocks become `ToolCall`s; a non-blank `thinking` block becomes
/// `reasoning_content`. `usage.output_tokens` is recorded as a best-effort
/// approximation of reasoning-token spend only when a thinking block was
/// actually present in the response.
pub fn parse_anthropic_completion(resp: &Value) -> Result<crate::model::ModelCompletion, crate::BoxErr> {
    let content = resp
        .get("content")
        .and_then(Value::as_array)
        .ok_or("no content in response")?;

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_content: Option<String> = None;

    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(Value::as_str) {
                    if !t.trim().is_empty() {
                        reasoning_content = Some(t.to_string());
                    }
                }
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let name = block.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let arguments = block.get("input").cloned().unwrap_or(Value::Null);
                tool_calls.push(crate::model::ToolCall { id, name, arguments });
            }
            _ => {}
        }
    }

    let finish_reason = resp
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let reasoning_content_available = reasoning_content.is_some();
    let actual_reasoning_tokens = reasoning_content_available
        .then(|| resp.get("usage").and_then(|u| u.get("output_tokens")).and_then(Value::as_u64))
        .flatten();

    Ok(crate::model::ModelCompletion {
        message: crate::model::AssistantMessage {
            content: text,
            tool_calls,
            finish_reason,
            reasoning_content,
        },
        telemetry: crate::reasoning::CompletionTelemetry {
            reasoning_content_available,
            actual_reasoning_tokens,
            ..Default::default()
        },
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent provider:: -- --nocapture`
Expected: all `provider::tests::*` tests pass, including the 5 new ones

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/provider.rs
git commit -m "feat(quecto-agent): parse Anthropic Messages API responses"
```

---

### Task 4: Anthropic `thinking` reasoning-mode mapping

**Files:**
- Modify: `quecto-agent/src/reasoning.rs`

**Interfaces:**
- Consumes: `crate::reasoning::ReasoningMode` (existing enum, unchanged).
- Produces: `pub fn anthropic_thinking_budget(mode: ReasoningMode) -> Option<u64>`, `pub fn apply_anthropic_thinking(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value>`. Additive only — the existing `apply_reasoning_mode` (OpenAI-compatible path) is untouched, so existing tests referencing it keep passing unmodified.

- [ ] **Step 1: Write the failing tests**

Add to `quecto-agent/src/reasoning.rs`'s existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn anthropic_budget_ladder_covers_every_mode() {
        assert_eq!(anthropic_thinking_budget(ReasoningMode::None), None);
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Minimal), Some(1024));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Low), Some(4000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Medium), Some(10000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::High), Some(24000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::XHigh), Some(32000));
    }

    #[test]
    fn applies_thinking_payload_to_body() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, Some(ReasoningMode::High)).unwrap();

        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 24000})
        );
        assert_eq!(payload, json!({"thinking": {"type": "enabled", "budget_tokens": 24000}}));
    }

    #[test]
    fn none_mode_omits_thinking_entirely() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, Some(ReasoningMode::None));

        assert!(payload.is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn no_mode_omits_thinking_entirely() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, None);

        assert!(payload.is_none());
        assert!(body.get("thinking").is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent reasoning:: -- --nocapture`
Expected: FAIL with `cannot find function anthropic_thinking_budget`/`apply_anthropic_thinking`

- [ ] **Step 3: Implement**

Add to `quecto-agent/src/reasoning.rs`, immediately after `apply_reasoning_mode` and before `parse_reasoning_tokens`:

```rust
/// Anthropic's `thinking.budget_tokens` for each `ReasoningMode`.
/// `None` omits the `thinking` parameter entirely (thinking disabled).
/// Anthropic's minimum `budget_tokens` is 1024, hence `Minimal` maps there
/// rather than to 0.
pub fn anthropic_thinking_budget(mode: ReasoningMode) -> Option<u64> {
    match mode {
        ReasoningMode::None => None,
        ReasoningMode::Minimal => Some(1024),
        ReasoningMode::Low => Some(4000),
        ReasoningMode::Medium => Some(10000),
        ReasoningMode::High => Some(24000),
        ReasoningMode::XHigh => Some(32000),
    }
}

/// Inject Anthropic's `thinking: {"type":"enabled","budget_tokens":N}` into
/// the request body for the given mode, if any. Returns the injected
/// payload (for telemetry), or `None` if no mode was requested or the mode
/// maps to no budget (`ReasoningMode::None`).
pub fn apply_anthropic_thinking(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value> {
    let budget = anthropic_thinking_budget(mode?)?;
    let payload = json!({"type": "enabled", "budget_tokens": budget});
    body.as_object_mut()?.insert("thinking".to_string(), payload.clone());
    Some(json!({"thinking": payload}))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent reasoning:: -- --nocapture`
Expected: all `reasoning::tests::*` tests pass, including the 4 new ones; the pre-existing `parses_all_reasoning_modes_case_insensitively` and `rejects_unknown_reasoning_modes` still pass unmodified

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/reasoning.rs
git commit -m "feat(quecto-agent): map ReasoningMode to Anthropic thinking budgets"
```

---

### Task 5: Wire `Provider` into `HttpModel`

This is the task that changes `HttpModel`'s field set, so every construction
site in the crate must be updated in this same task to keep the crate
compiling. There are 9 sites total; 4 are covered here (the ones inside
`quecto-agent/src/`), the remaining 5 (in `quecto-agent/src/main.rs` and the
`quecto-agent/tests/` integration tests) are covered in Tasks 6 and 7.
Because Rust requires the whole crate to type-check, **this task alone will
not compile quecto-agent's binary or its `tests/` crate** — that's expected;
verify with `cargo test -p quecto-agent --lib` (library unit tests only)
after this task, and the full `cargo test -p quecto-agent` only after Task 7.

**Files:**
- Modify: `quecto-agent/src/model.rs`

**Interfaces:**
- Consumes: `crate::provider::{Provider, DEFAULT_ANTHROPIC_MAX_TOKENS, messages_to_anthropic_body, tools_to_anthropic, parse_anthropic_completion}` (Tasks 1–3), `crate::reasoning::apply_anthropic_thinking` (Task 4).
- Produces: `HttpModel` gains two new public fields: `pub provider: Provider` and `pub max_tokens: Option<u32>`. `HttpModel::from_env()` sets `provider: Provider::OpenAiCompatible, max_tokens: None` (the legacy env-only constructor stays OpenAI-only, matching its doc comment "Build from the legacy core env config only").

- [ ] **Step 1: Write the failing test**

Add to `quecto-agent/src/model.rs`'s `#[cfg(test)] mod tests` block, right after the existing `completion_options_can_change_reasoning_mode_across_calls` test (the raw `TcpListener` fake-server test around line 755):

```rust
    #[test]
    fn anthropic_provider_completes_against_mock_with_tool_use_and_thinking() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = std::sync::mpsc::channel::<(Value, String)>();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let header_end = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "request ended before body");
                request.extend_from_slice(&buffer[..read]);
            }
            let body: Value = serde_json::from_slice(&request[header_end..]).unwrap();
            request_tx.send((body, headers)).unwrap();

            let resp_body = r#"{"content":[{"type":"thinking","thinking":"reasoning here"},{"type":"text","text":"done"},{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"a.rs"}}],"stop_reason":"tool_use","usage":{"output_tokens":50}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            )
            .unwrap();
        });

        let model = HttpModel {
            url: format!("http://{address}/v1/messages"),
            api_key: Some("test-key".into()),
            model: "claude-x".into(),
            provider: crate::provider::Provider::Anthropic,
            max_tokens: Some(1234),
        };
        let completion = model
            .complete_with_options(
                &[Message::user("read a.rs")],
                &[json!({
                    "type": "function",
                    "function": {"name": "read_file", "description": "read", "parameters": {"type": "object"}}
                })],
                &crate::reasoning::CompletionOptions {
                    reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
                },
            )
            .unwrap();
        server.join().unwrap();
        let (sent_body, sent_headers) = request_rx.recv().unwrap();

        assert_eq!(sent_body["model"], "claude-x");
        assert_eq!(sent_body["max_tokens"], 1234);
        assert_eq!(sent_body["thinking"], json!({"type": "enabled", "budget_tokens": 24000}));
        assert_eq!(sent_body["tools"][0]["name"], "read_file");
        // Header casing on the wire is a ureq implementation detail — compare
        // lowercased to avoid coupling the test to it.
        let sent_headers_lower = sent_headers.to_ascii_lowercase();
        assert!(sent_headers_lower.contains("x-api-key: test-key"));
        assert!(sent_headers_lower.contains("anthropic-version: 2023-06-01"));
        assert!(!sent_headers_lower.contains("authorization:"));

        assert_eq!(completion.message.content, "done");
        assert_eq!(completion.message.finish_reason, "tool_use");
        assert_eq!(completion.message.tool_calls[0].name, "read_file");
        assert_eq!(completion.message.reasoning_content.as_deref(), Some("reasoning here"));
        assert!(completion.telemetry.reasoning_parameters_sent);
        assert_eq!(completion.telemetry.actual_reasoning_tokens, Some(50));
    }

    #[test]
    fn anthropic_provider_defaults_max_tokens_when_unset() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = std::sync::mpsc::channel::<Value>();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let header_end = loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            request_tx
                .send(serde_json::from_slice(&request[header_end..]).unwrap())
                .unwrap();

            let resp_body = r#"{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            )
            .unwrap();
        });

        let model = HttpModel {
            url: format!("http://{address}/v1/messages"),
            api_key: None,
            model: "claude-x".into(),
            provider: crate::provider::Provider::Anthropic,
            max_tokens: None,
        };
        model
            .complete_with_options(
                &[Message::user("hi")],
                &[],
                &crate::reasoning::CompletionOptions::default(),
            )
            .unwrap();
        server.join().unwrap();
        let sent_body = request_rx.recv().unwrap();

        assert_eq!(sent_body["max_tokens"], crate::provider::DEFAULT_ANTHROPIC_MAX_TOKENS);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib model:: -- --nocapture`
Expected: FAIL to compile — `HttpModel` struct literal is missing fields `provider` and `max_tokens` (they don't exist yet)

- [ ] **Step 3: Add the fields and branch `complete_with_options`**

In `quecto-agent/src/model.rs`, change the `HttpModel` struct definition:

```rust
/// The real model client: buffered `quecto_raw` against either an
/// OpenAI-compatible or Anthropic Messages API endpoint, selected by `provider`.
#[derive(Clone)]
pub struct HttpModel {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub provider: crate::provider::Provider,
    /// Anthropic-only: forwarded as the request's `max_tokens`. Ignored for
    /// `Provider::OpenAiCompatible`. `None` uses `DEFAULT_ANTHROPIC_MAX_TOKENS`.
    pub max_tokens: Option<u32>,
}
```

Update `HttpModel::from_env()` (the legacy constructor) to set both new fields:

```rust
    pub fn from_env() -> Self {
        let (base, key, model, _system) = quecto::env_config();
        HttpModel {
            url: quecto::join_url(&base, "chat/completions"),
            api_key: key,
            model,
            provider: crate::provider::Provider::OpenAiCompatible,
            max_tokens: None,
        }
    }
```

**Important:** the original code records `quecto.requested_reasoning_mode` / `quecto.provider_reasoning_parameters` / `quecto.reasoning_parameters_sent` onto the otel span *before* the HTTP request goes out, specifically so that a failed request (network error, non-2xx, malformed response) still has those fields on its span — this is asserted by the existing `http_error_preserves_request_reasoning_span_fields` and `malformed_response_preserves_request_reasoning_span_fields` tests in `quecto-agent/tests/model.rs`. The provider branch below must record the same fields at the same point (right after computing them, before calling `quecto_raw`) in **both** match arms, not once after the whole match — otherwise a failed request short-circuits via `?` before the fields are ever recorded, silently breaking those two tests. Factor the recording into a small helper to avoid duplicating it per arm.

Add this helper above `impl Model for HttpModel`:

```rust
#[cfg(feature = "otel")]
fn record_reasoning_request_fields(
    span: &tracing::Span,
    reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    provider_reasoning_parameters: &Option<Value>,
    reasoning_parameters_sent: bool,
) {
    if let Some(mode) = reasoning_mode {
        span.record("quecto.requested_reasoning_mode", mode.effort_str());
    }
    if let Some(parameters) = provider_reasoning_parameters {
        span.record("quecto.provider_reasoning_parameters", parameters.to_string());
    }
    span.record("quecto.reasoning_parameters_sent", reasoning_parameters_sent);
}
```

Replace the body of `impl Model for HttpModel { fn complete_with_options(...) }` (everything between the `#[cfg(feature = "otel")] let _guard = span.enter();` line and the final `#[cfg(feature = "otel")]` telemetry-recording block) with:

```rust
        let reasoning_mode = options.reasoning_mode;

        let (mut completion, provider_reasoning_parameters, reasoning_parameters_sent) =
            match self.provider {
                crate::provider::Provider::OpenAiCompatible => {
                    let mut body = messages_to_body(&self.model, messages);
                    if !tools.is_empty() {
                        body["tools"] = Value::Array(tools.to_vec());
                    }
                    let provider_reasoning_parameters =
                        crate::reasoning::apply_reasoning_mode(&mut body, &self.url, reasoning_mode);
                    let reasoning_parameters_sent = provider_reasoning_parameters.is_some();
                    #[cfg(feature = "otel")]
                    record_reasoning_request_fields(
                        &span,
                        reasoning_mode,
                        &provider_reasoning_parameters,
                        reasoning_parameters_sent,
                    );
                    let auth = self.api_key.as_ref().map(|k| format!("Bearer {k}"));
                    let mut headers: Vec<(&str, &str)> = Vec::new();
                    if let Some(a) = &auth {
                        headers.push(("Authorization", a.as_str()));
                    }
                    let resp = quecto::quecto_raw(&self.url, &headers, body)?;
                    (
                        parse_assistant_completion(&resp)?,
                        provider_reasoning_parameters,
                        reasoning_parameters_sent,
                    )
                }
                crate::provider::Provider::Anthropic => {
                    let max_tokens = self
                        .max_tokens
                        .unwrap_or(crate::provider::DEFAULT_ANTHROPIC_MAX_TOKENS);
                    let mut body =
                        crate::provider::messages_to_anthropic_body(&self.model, messages, max_tokens);
                    if !tools.is_empty() {
                        body["tools"] = Value::Array(crate::provider::tools_to_anthropic(tools));
                    }
                    let provider_reasoning_parameters =
                        crate::reasoning::apply_anthropic_thinking(&mut body, reasoning_mode);
                    let reasoning_parameters_sent = provider_reasoning_parameters.is_some();
                    #[cfg(feature = "otel")]
                    record_reasoning_request_fields(
                        &span,
                        reasoning_mode,
                        &provider_reasoning_parameters,
                        reasoning_parameters_sent,
                    );
                    let mut headers: Vec<(&str, &str)> = vec![("anthropic-version", "2023-06-01")];
                    if let Some(k) = self.api_key.as_deref() {
                        headers.push(("x-api-key", k));
                    }
                    let resp = quecto::quecto_raw(&self.url, &headers, body)?;
                    (
                        crate::provider::parse_anthropic_completion(&resp)?,
                        provider_reasoning_parameters,
                        reasoning_parameters_sent,
                    )
                }
            };
        completion.telemetry.requested_reasoning_mode = reasoning_mode;
        completion.telemetry.provider_reasoning_parameters = provider_reasoning_parameters;
        completion.telemetry.reasoning_parameters_sent = reasoning_parameters_sent;
```

(The `#[cfg(feature = "otel")]` span-setup block before this, and the final telemetry-recording `#[cfg(feature = "otel")]` block plus `Ok(completion)` after it, stay exactly as they were — only the middle section that built the request/parsed the response changes.)

Now fix the two other `HttpModel { ... }` struct literals already inside `quecto-agent/src/model.rs`'s test module (search for `let mut model = HttpModel {` and `let model = HttpModel {` — do **not** touch the new ones you just added in Step 1). For each, add the two new fields:

```rust
        let mut model = HttpModel {
            url: "http://example.test/v1/chat/completions".into(),
            api_key: None,
            model: "test-model".into(),
            provider: crate::provider::Provider::OpenAiCompatible,
            max_tokens: None,
        }
```

and

```rust
        let model = HttpModel {
            url: format!("http://{address}/v1/chat/completions"),
            api_key: None,
            model: "test-model".into(),
            provider: crate::provider::Provider::OpenAiCompatible,
            max_tokens: None,
        }
```

- [ ] **Step 4: Fix the two remaining crate-internal call sites (not in `model.rs`)**

In `quecto-agent/src/agent.rs`, in `agent_session_reasoning_mode_round_trips_on_configured_model`, update the `HttpModel` literal:

```rust
        let model = crate::model::HttpModel {
            url: "http://example.test/v1/chat/completions".into(),
            api_key: None,
            model: "test-model".into(),
            provider: crate::provider::Provider::OpenAiCompatible,
            max_tokens: None,
        }
        .with_default_reasoning_mode(Some(crate::reasoning::ReasoningMode::Low));
```

Run `cargo build -p quecto-agent --lib` now — it will still fail on `main.rs`'s three production sites and one test site, and on `quecto-agent/tests/*.rs`. That's expected; those are fixed in Tasks 6 and 7. Confirm the *library* target alone type-checks by running:

Run: `cargo check -p quecto-agent --lib`
Expected: succeeds (this checks `src/lib.rs` and everything it declares as modules, i.e. `agent.rs`, `model.rs`, `provider.rs`, `reasoning.rs`, etc. — but not `main.rs`, which is a separate binary target, nor `tests/`)

- [ ] **Step 5: Run the library tests to verify they pass**

Run: `cargo test -p quecto-agent --lib -- --nocapture`
Expected: all tests pass, including the 2 new ones from Step 1 (`anthropic_provider_completes_against_mock_with_tool_use_and_thinking`, `anthropic_provider_defaults_max_tokens_when_unset`)

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/agent.rs
git commit -m "feat(quecto-agent): dispatch HttpModel completion by Provider"
```

---

### Task 6: Flavor config (`provider`, `max_tokens`)

**Files:**
- Modify: `quecto-agent/src/flavor.rs`
- Modify: `quecto-agent/src/main.rs` (scaffold template comment only — no logic yet, that's Task 7)
- Modify: `quecto-agent/tests/public_api_compat.rs` (fix the `Flavor` struct literal)

**Interfaces:**
- Consumes: `crate::provider::Provider` (Task 1).
- Produces: `Flavor.provider: Option<Provider>`, `Flavor.max_tokens: Option<u32>` (and the matching fields on `ConfiguredFlavorDocument`, mapped through by `ConfiguredFlavor::parse`). `Flavor::merge` carries both through with the existing `or()` (override-wins) semantics.

- [ ] **Step 1: Write the failing test**

Add to `quecto-agent/src/flavor.rs`'s `#[cfg(test)] mod tests` block (find it near the bottom of the file; if you're unsure of its exact location, run `grep -n "mod tests" quecto-agent/src/flavor.rs` first):

```rust
    #[test]
    fn parses_provider_and_max_tokens() {
        let flavor = Flavor::parse(
            r#"
            provider = "anthropic"
            max_tokens = 8192
            "#,
        )
        .unwrap();

        assert_eq!(flavor.provider, Some(crate::provider::Provider::Anthropic));
        assert_eq!(flavor.max_tokens, Some(8192));
    }

    #[test]
    fn merge_lets_override_win_for_provider_and_max_tokens() {
        let base = Flavor {
            provider: Some(crate::provider::Provider::OpenAiCompatible),
            max_tokens: Some(1000),
            ..Flavor::default()
        };
        let over = Flavor {
            provider: Some(crate::provider::Provider::Anthropic),
            max_tokens: None,
            ..Flavor::default()
        };

        let merged = base.merge(over);

        assert_eq!(merged.provider, Some(crate::provider::Provider::Anthropic));
        assert_eq!(merged.max_tokens, Some(1000));
    }

    #[test]
    fn configured_flavor_parses_provider_alongside_reasoning_mode() {
        let configured = ConfiguredFlavor::parse(
            r#"
            provider = "anthropic"
            max_tokens = 4096
            reasoning_mode = "high"
            "#,
        )
        .unwrap();

        assert_eq!(configured.flavor.provider, Some(crate::provider::Provider::Anthropic));
        assert_eq!(configured.flavor.max_tokens, Some(4096));
        assert_eq!(configured.reasoning_mode, Some(crate::reasoning::ReasoningMode::High));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib flavor:: -- --nocapture`
Expected: FAIL to compile — `Flavor` has no field `provider`/`max_tokens`

- [ ] **Step 3: Implement**

In `quecto-agent/src/flavor.rs`, add the two fields to `Flavor` (right after `base_url`):

```rust
pub struct Flavor {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub provider: Option<crate::provider::Provider>,
    pub max_tokens: Option<u32>,
    pub max_steps: Option<usize>,
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
```

Add the same two fields to `ConfiguredFlavorDocument` (also right after `base_url`):

```rust
struct ConfiguredFlavorDocument {
    name: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    provider: Option<crate::provider::Provider>,
    max_tokens: Option<u32>,
    max_steps: Option<usize>,
    reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    auto_verify: Option<bool>,
    auto_approve: Option<bool>,
    system_prompt: Option<String>,
    system_prompt_file: Option<String>,
    #[serde(default)]
    tools: ToolsSection,
    #[serde(default)]
    approval: ApprovalSection,
    #[serde(default)]
    verify: VerifySection,
}
```

In `Flavor::merge`, add both fields to the constructed `Flavor` (right after `base_url: or(self.base_url, over.base_url),`):

```rust
            base_url: or(self.base_url, over.base_url),
            provider: or(self.provider, over.provider),
            max_tokens: or(self.max_tokens, over.max_tokens),
```

In `ConfiguredFlavor::parse`, add both fields to the constructed inner `Flavor` (right after `base_url: document.base_url,`):

```rust
                base_url: document.base_url,
                provider: document.provider,
                max_tokens: document.max_tokens,
```

- [ ] **Step 4: Fix `public_api_compat.rs`'s `Flavor` literal**

In `quecto-agent/tests/public_api_compat.rs`, add `Provider` to the import list:

```rust
use quecto_agent::{
    ApprovalSection, AssistantMessage, CompletionTelemetry, Flavor, HttpModel, Message, Model,
    Provider, ToolCall, ToolsSection, VerifySection,
};
```

In the `Flavor { ... }` literal inside `legacy_public_struct_literals_still_compile_and_work`, add the two new fields right after `base_url: None,`:

```rust
    let flavor = Flavor {
        name: Some("legacy".into()),
        model: Some("legacy-model".into()),
        base_url: None,
        provider: None,
        max_tokens: None,
        max_steps: Some(10),
        auto_verify: None,
        auto_approve: None,
        system_prompt: None,
        system_prompt_file: None,
        tools: ToolsSection::default(),
        approval: ApprovalSection::default(),
        verify: VerifySection::default(),
    };
```

(`Provider` is imported but only used implicitly via `Option<Provider>` type inference at `None` — if `cargo check` reports it as an unused import because `None`'s type is inferred without ever naming `Provider`, remove the `Provider` import again; keep whichever the compiler accepts. Run Step 5 to find out.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib flavor:: -- --nocapture`
Expected: all `flavor::tests::*` pass, including the 3 new ones

Run: `cargo test -p quecto-agent --test public_api_compat`
Expected: this will still fail to compile until Task 7 fixes the `HttpModel` literal in the same file — that's expected. Confirm only that the `Flavor` literal itself no longer errors by checking the compiler output mentions only `HttpModel`, not `Flavor`, as missing fields.

- [ ] **Step 6: Update the scaffold template documentation**

In `quecto-agent/src/main.rs`, in the `SCAFFOLD_TEMPLATE` constant, add commented-out example lines for the new fields (insert after the `# base_url` line, before `# reasoning_mode`):

```rust
# base_url      = "http://localhost:11434/v1"
# provider      = "openai"  # openai | anthropic
# max_tokens    = 4096      # required by Anthropic; ignored for openai
# reasoning_mode  = "low"
```

- [ ] **Step 7: Commit**

```bash
git add quecto-agent/src/flavor.rs quecto-agent/src/main.rs quecto-agent/tests/public_api_compat.rs
git commit -m "feat(quecto-agent): add provider/max_tokens flavor config fields"
```

---

### Task 7: CLI flags and production wiring

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/model.rs`
- Modify: `quecto-agent/tests/public_api_compat.rs` (the `HttpModel` literal, deferred from Task 6)

**Interfaces:**
- Consumes: `crate::provider::Provider` (Task 1), `Flavor.provider`/`Flavor.max_tokens` (Task 6).
- Produces: `--provider <openai|anthropic>` and `--max-tokens <N>` global CLI flags; `fn resolve_provider(overrides: &Overrides, merged: &Flavor) -> Result<Provider, quecto_agent::BoxErr>`; `fn resolve_max_tokens(overrides: &Overrides, merged: &Flavor) -> Option<u32>`. All `HttpModel` construction sites in the crate now compile and the crate's provider selection is fully wired end to end.

- [ ] **Step 1: Add the CLI flags and `Overrides` fields**

In `quecto-agent/src/main.rs`, add `Provider` to the top-of-file import:

```rust
use quecto_agent::{
    cancel_token, chat_spinner_renderer, content_hash, join_url, load_instructions, new_session_id,
    parse_command, parse_spinner_verbs, project_raw, render_assistant_text, render_change_summary,
    resolve_scoped_configured, seed_context, Agent, ApprovalMode, ChatCommand, ConfiguredFlavor,
    Flavor, HttpModel, LineRenderer, Outcome, Policy, Preset, Provider, ReasoningCommand,
    ReasoningMode, Renderer, SqliteRecorder, Store, TrustStore, Verifier,
};
```

Add two new fields to the `Cli` struct, right after `base_url`:

```rust
    /// Override the OpenAI-compatible base URL.
    #[arg(long, global = true)]
    base_url: Option<String>,
    /// Select the provider wire format: "openai" (default) or "anthropic".
    #[arg(long, global = true)]
    provider: Option<String>,
    /// Override the max_tokens sent to Anthropic requests (ignored for openai).
    #[arg(long, global = true)]
    max_tokens: Option<u32>,
```

Add the same two fields to `Overrides`, right after `base_url`:

```rust
struct Overrides {
    flavor: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    provider: Option<String>,
    max_tokens: Option<u32>,
    max_steps: Option<usize>,
    approval: Option<String>,
    #[cfg(feature = "mcp")]
    mcp: Vec<String>,
}
```

In `main()`, add the two fields to the `Overrides` construction, right after `base_url: cli.base_url.clone(),`:

```rust
    let overrides = Overrides {
        flavor: cli.flavor.clone(),
        model: cli.model.clone(),
        base_url: cli.base_url.clone(),
        provider: cli.provider.clone(),
        max_tokens: cli.max_tokens,
        max_steps: cli.max_steps,
        approval: cli.approval.clone(),
        #[cfg(feature = "mcp")]
        mcp: cli.mcp,
    };
```

- [ ] **Step 2: Add the resolver helpers**

In `quecto-agent/src/main.rs`, immediately after `fn resolve_host_and_model(...)` (which ends with `(base_url, model_name)`), add:

```rust
fn resolve_provider(overrides: &Overrides, merged: &Flavor) -> Result<Provider, quecto_agent::BoxErr> {
    if let Some(flag) = &overrides.provider {
        return flag.parse();
    }
    if let Ok(env) = std::env::var("QUECTO_PROVIDER") {
        if !env.is_empty() {
            return env.parse();
        }
    }
    Ok(merged.provider.unwrap_or_default())
}

fn resolve_max_tokens(overrides: &Overrides, merged: &Flavor) -> Option<u32> {
    overrides
        .max_tokens
        .or_else(|| {
            std::env::var("QUECTO_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or(merged.max_tokens)
}
```

- [ ] **Step 3: Wire the resolvers into the three production `HttpModel` construction sites**

There are three near-identical sites: `fn run(...)` (~line 469), `fn chat(...)` (~line 556), `fn resume(...)` (~line 1002). Each currently does:

```rust
    let (base_url, model_name) = resolve_host_and_model(overrides, &merged);
    let api_key = std::env::var("QUECTO_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let model = HttpModel {
        url: join_url(&base_url, "chat/completions"),
        api_key,
        model: model_name,
    }
```

(`run` binds `api_key` to a local first; `chat` and `resume` inline `std::env::var(...)` directly in the literal — keep whichever shape each site already uses.) In **all three**, replace with:

```rust
    let (base_url, model_name) = resolve_host_and_model(overrides, &merged);
    let provider = resolve_provider(overrides, &merged).unwrap_or_else(|e| {
        eprintln!("quecto-agent: {e}");
        std::process::exit(2);
    });
    let api_key = std::env::var("QUECTO_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let model = HttpModel {
        url: join_url(&base_url, provider.path_suffix()),
        api_key,
        model: model_name,
        provider,
        max_tokens: resolve_max_tokens(overrides, &merged),
    }
```

(For the two sites that inline `api_key` directly into the struct literal instead of binding it first, keep them inlined — just add `provider,` and `max_tokens: resolve_max_tokens(overrides, &merged),` as two more fields in the literal, and insert the `let provider = ...` line before the literal.)

- [ ] **Step 4: Fix the remaining `HttpModel` literals**

In `quecto-agent/src/main.rs`'s test module, in `test_agent`, update:

```rust
    fn test_agent(mode: Option<ReasoningMode>) -> Agent {
        let model = HttpModel {
            url: "http://example.test/v1/chat/completions".into(),
            api_key: None,
            model: "test-model".into(),
            provider: Provider::OpenAiCompatible,
            max_tokens: None,
        }
        .with_default_reasoning_mode(mode);
```

In `quecto-agent/tests/model.rs`, add `Provider` to the import:

```rust
use quecto_agent::{
    parse_assistant_completion, CompletionOptions, HttpModel, Message, Model, Provider,
    ReasoningMode,
};
```

Then update all 4 `HttpModel { ... }` literals in that file (`http_model_completes_against_mock`, `chat_completions_sends_top_level_reasoning_effort`, `unsupported_endpoint_omits_reasoning_parameters`, and the `#[cfg(feature = "otel")]`-gated `failed_completion_fields`) by adding two fields to each, e.g.:

```rust
    let m = HttpModel {
        url: format!("{base}/chat/completions"),
        api_key: None,
        model: "m".to_string(),
        provider: Provider::OpenAiCompatible,
        max_tokens: None,
    };
```

In `quecto-agent/tests/public_api_compat.rs`, update the `HttpModel { ... }` literal (import already fixed in Task 6):

```rust
    let model = HttpModel {
        url: "http://127.0.0.1:1/v1/chat/completions".into(),
        api_key: None,
        model: "legacy-model".into(),
        provider: Provider::OpenAiCompatible,
        max_tokens: None,
    };
```

- [ ] **Step 5: Run the full test suite to verify everything passes**

Run: `cargo test -p quecto-agent`
Expected: all tests pass across the library, `main.rs`'s internal tests, and every file under `quecto-agent/tests/`

Run: `cargo test -p quecto-agent --features otel`
Expected: all tests pass, including the otel-gated ones in `tests/model.rs` — in
particular `http_error_preserves_request_reasoning_span_fields` and
`malformed_response_preserves_request_reasoning_span_fields`, which are the
regression check for the `record_reasoning_request_fields` placement from
Task 5 (they confirm reasoning span fields are still recorded even when the
request fails, which requires recording them *before* `quecto_raw` runs in
each provider branch, not after the whole `match`)

Run: `cargo build -p quecto-agent --bin quecto-agent`
Expected: builds cleanly

- [ ] **Step 6: Manual smoke check of the new flags**

Run: `cargo run -p quecto-agent -- --provider bogus "hello"`
Expected: prints `quecto-agent: unknown provider: bogus` to stderr and exits with status 2 (no panic)

Run: `cargo run -p quecto-agent -- --provider anthropic --base-url https://api.anthropic.com/v1 --model claude-opus-4-8 --max-tokens 1024 "say hi"`
Expected: fails with a network/auth error from Anthropic (no `QUECTO_API_KEY` set in this smoke check) rather than a panic or a client-side format error — confirms the request reaches Anthropic's API in the right shape. If you have a real `QUECTO_API_KEY` for testing, set it and confirm you get an actual assistant reply.

- [ ] **Step 7: Commit**

```bash
git add quecto-agent/src/main.rs quecto-agent/tests/model.rs quecto-agent/tests/public_api_compat.rs
git commit -m "feat(quecto-agent): add --provider/--max-tokens CLI flags and wire production call sites"
```

---

## Post-implementation

After Task 7, quecto-agent supports `provider = "anthropic"` (flavor file), `--provider anthropic` (CLI), or `QUECTO_PROVIDER=anthropic` (env) to talk to Claude models natively via the Messages API, with `max_tokens`/`--max-tokens`/`QUECTO_MAX_TOKENS` controlling the required `max_tokens` field, and `reasoning_mode`/`--reasoning`/`QUECTO_REASONING_MODE` mapped to Anthropic's `thinking.budget_tokens`.

Streaming, vision/PDF input, prompt caching, and other Anthropic-specific features remain out of scope per the design doc's non-goals — file a follow-up spec if/when those are needed.
