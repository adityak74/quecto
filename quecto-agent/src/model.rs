use crate::BoxErr;
use serde_json::{json, Value};

/// A single chat message in the running transcript.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub reasoning_content: Option<String>,
}

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

    pub fn system(c: impl Into<String>) -> Self {
        Message::plain("system", c)
    }

    pub fn user(c: impl Into<String>) -> Self {
        Message::plain("user", c)
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
    pub reasoning_content: Option<String>,
}

pub fn extract_think_tags(content: &str) -> (Option<String>, String) {
    if let Some(start) = content.find("<think>") {
        if let Some(end) = content.find("</think>") {
            if start < end {
                let reasoning = content[start + 7..end].trim().to_string();
                let cleaned_content = format!("{}{}", &content[..start], &content[end + 8..]).trim().to_string();
                return (Some(reasoning), cleaned_content);
            }
        } else {
            let reasoning = content[start + 7..].trim().to_string();
            let cleaned_content = content[..start].trim().to_string();
            return (Some(reasoning), cleaned_content);
        }
    }
    (None, content.to_string())
}

/// Parse an OpenAI-compatible buffered chat response (native tool-call protocol)
/// into a normalized AssistantMessage. Content absent/null -> ""; tool_calls absent -> [].
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
        .or_else(|| message.get("reasoning"))
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
            // Native protocol encodes arguments as a JSON string; tolerate an object too.
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

/// Serialize the transcript into an OpenAI-compatible request body.
pub fn messages_to_body(model: &str, messages: &[Message]) -> Value {
    let msgs: Vec<Value> = messages.iter().map(message_to_json).collect();
    json!({"model": model, "messages": msgs})
}

fn message_to_json(m: &Message) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".into(), json!(m.role));
    let content = if let Some(reasoning) = &m.reasoning_content {
        format!("<think>\n{}\n</think>\n{}", reasoning, m.content)
    } else {
        m.content.clone()
    };
    obj.insert("content".into(), json!(content));
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

/// Abstraction over "take the transcript, return the assistant's next message."
/// The real impl calls the model over HTTP; tests inject a scripted fake.
pub trait Model: Send + Sync {
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr>;
}

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
    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "model_complete",
            quecto.model = self.model.as_str(),
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
                let redacted_reasoning = crate::sandbox::redact_secrets(reasoning);
                tracing::event!(tracing::Level::INFO, name = "model_thinking", content = %redacted_reasoning);
            }
            if !msg.content.is_empty() {
                let redacted_content = crate::sandbox::redact_secrets(&msg.content);
                tracing::event!(tracing::Level::INFO, name = "model_response", content = %redacted_content);
            }
        }

        parsed
    }
}

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
        assert!(body["messages"][0].get("tool_calls").is_none());
        assert!(body["messages"][1].get("tool_call_id").is_none());
    }

    #[test]
    fn assistant_tool_call_serializes_native_shape() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: json!({"path":"a.rs"}),
        };
        let body = messages_to_body("m", &[Message::assistant_with_calls("", vec![call])]);
        let tc = &body["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "c1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        assert_eq!(tc["function"]["arguments"], "{\"path\":\"a.rs\"}");
    }

    #[test]
    fn tool_result_serializes_with_id() {
        let body = messages_to_body("m", &[Message::tool_result("c1", "file contents")]);
        assert_eq!(body["messages"][0]["role"], "tool");
        assert_eq!(body["messages"][0]["tool_call_id"], "c1");
        assert_eq!(body["messages"][0]["content"], "file contents");
    }

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

    #[test]
    fn serializes_reasoning_content_with_think_tags() {
        let mut m = Message::assistant("hello");
        m.reasoning_content = Some("thinking 123".to_string());
        let body = messages_to_body("m", &[m]);
        assert_eq!(
            body["messages"][0]["content"],
            "<think>\nthinking 123\n</think>\nhello"
        );
    }

    #[test]
    fn parses_think_tags_missing_closing_tag() {
        let r = json!({"choices":[{"message":{"content":"<think>\nthinking 789\n"},"finish_reason":"length"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "");
        assert_eq!(m.reasoning_content, Some("thinking 789".to_string()));
    }
}
