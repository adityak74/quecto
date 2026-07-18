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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Message, ToolCall};
    use serde_json::json;

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
}
