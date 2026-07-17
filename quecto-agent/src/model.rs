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
    pub requested_reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    pub provider_reasoning_parameters: Option<Value>,
    pub reasoning_parameters_sent: Option<bool>,
    pub reasoning_content_available: Option<bool>,
    pub actual_reasoning_tokens: Option<u64>,
}

impl Message {
    fn plain(role: &str, content: impl Into<String>) -> Self {
        Message {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
            requested_reasoning_mode: None,
            provider_reasoning_parameters: None,
            reasoning_parameters_sent: None,
            reasoning_content_available: None,
            actual_reasoning_tokens: None,
        }
    }

    pub fn system(c: impl Into<String>) -> Self {
        Message::plain("system", c)
    }

    pub fn user(c: impl Into<String>) -> Self {
        Message::plain("user", c)
    }

    pub fn assistant(c: impl Into<String>) -> Self {
        Message::plain("assistant", c)
    }

    pub fn assistant_with_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message {
            role: "assistant".into(),
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            reasoning_content: None,
            requested_reasoning_mode: None,
            provider_reasoning_parameters: None,
            reasoning_parameters_sent: None,
            reasoning_content_available: None,
            actual_reasoning_tokens: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message {
            role: "tool".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
            requested_reasoning_mode: None,
            provider_reasoning_parameters: None,
            reasoning_parameters_sent: None,
            reasoning_content_available: None,
            actual_reasoning_tokens: None,
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
    pub completion: crate::reasoning::CompletionTelemetry,
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
    let reasoning_content_available = reasoning_content.is_some();

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
        completion: crate::reasoning::CompletionTelemetry {
            reasoning_content_available,
            ..crate::reasoning::CompletionTelemetry::default()
        },
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

    fn complete_with_options(
        &self,
        messages: &[Message],
        tools: &[Value],
        _options: &crate::reasoning::CompletionOptions,
    ) -> Result<AssistantMessage, BoxErr> {
        self.complete(messages, tools)
    }

    fn clone_box(&self) -> Box<dyn Model>;
}

impl Clone for Box<dyn Model> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// The real model client: buffered `quecto_raw` against an OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct HttpModel {
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub default_reasoning_mode: Option<crate::reasoning::ReasoningMode>,
}

impl HttpModel {
    /// Build from environment, panicking when `QUECTO_REASONING_MODE` is invalid.
    /// Prefer [`HttpModel::try_from_env`] when the caller can surface configuration errors.
    pub fn from_env() -> Self {
        Self::try_from_env().expect("invalid HttpModel environment configuration")
    }

    /// Build from the core env config plus the optional reasoning-mode setting.
    pub fn try_from_env() -> Result<Self, BoxErr> {
        let (base, key, model, _system) = quecto::env_config();
        Ok(HttpModel {
            url: quecto::join_url(&base, "chat/completions"),
            api_key: key,
            model,
            default_reasoning_mode: crate::reasoning::parse_env_reasoning_mode()?,
        })
    }
}

fn effective_reasoning_mode(
    default_mode: Option<crate::reasoning::ReasoningMode>,
    options: &crate::reasoning::CompletionOptions,
) -> Option<crate::reasoning::ReasoningMode> {
    options.reasoning_mode.or(default_mode)
}

impl Model for HttpModel {
    fn clone_box(&self) -> Box<dyn Model> {
        Box::new(self.clone())
    }

    fn complete(&self, messages: &[Message], tools: &[Value]) -> Result<AssistantMessage, BoxErr> {
        self.complete_with_options(
            messages,
            tools,
            &crate::reasoning::CompletionOptions::default(),
        )
    }

    fn complete_with_options(
        &self,
        messages: &[Message],
        tools: &[Value],
        options: &crate::reasoning::CompletionOptions,
    ) -> Result<AssistantMessage, BoxErr> {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "model_complete",
            quecto.model = self.model.as_str(),
            quecto.messages_sent = messages.len(),
            quecto.tools_provided = tools.len(),
            quecto.requested_reasoning_mode = tracing::field::Empty,
            quecto.provider_reasoning_parameters = tracing::field::Empty,
            quecto.reasoning_parameters_sent = tracing::field::Empty,
            quecto.reasoning_content_available = tracing::field::Empty,
            quecto.actual_reasoning_tokens = tracing::field::Empty
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

        let reasoning_mode = effective_reasoning_mode(self.default_reasoning_mode, options);
        let mut body = messages_to_body(&self.model, messages);
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        let provider_reasoning_parameters =
            crate::reasoning::apply_reasoning_mode(&mut body, &self.url, reasoning_mode);
        let reasoning_parameters_sent = provider_reasoning_parameters.is_some();
        let auth = self.api_key.as_ref().map(|k| format!("Bearer {k}"));
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(a) = &auth {
            headers.push(("Authorization", a.as_str()));
        }
        let resp = quecto::quecto_raw(&self.url, &headers, body)?;
        let mut parsed = parse_assistant(&resp)?;
        let reasoning_content_available = parsed.reasoning_content.is_some();
        parsed.completion = crate::reasoning::CompletionTelemetry {
            requested_reasoning_mode: reasoning_mode,
            provider_reasoning_parameters,
            reasoning_parameters_sent,
            reasoning_content_available,
            actual_reasoning_tokens: crate::reasoning::parse_reasoning_tokens(&resp),
        };

        #[cfg(feature = "otel")]
        {
            if let Some(mode) = parsed.completion.requested_reasoning_mode {
                span.record("quecto.requested_reasoning_mode", mode.effort_str());
            }
            if let Some(parameters) = &parsed.completion.provider_reasoning_parameters {
                span.record("quecto.provider_reasoning_parameters", parameters.to_string());
            }
            span.record(
                "quecto.reasoning_parameters_sent",
                parsed.completion.reasoning_parameters_sent,
            );
            span.record(
                "quecto.reasoning_content_available",
                parsed.completion.reasoning_content_available,
            );
            if let Some(tokens) = parsed.completion.actual_reasoning_tokens {
                span.record("quecto.actual_reasoning_tokens", tokens);
            }
            if let Some(reasoning) = &parsed.reasoning_content {
                let redacted_reasoning = crate::sandbox::redact_secrets(reasoning);
                tracing::event!(tracing::Level::INFO, name = "model_thinking", content = %redacted_reasoning);
            }
            if !parsed.content.is_empty() {
                let redacted_content = crate::sandbox::redact_secrets(&parsed.content);
                tracing::event!(tracing::Level::INFO, name = "model_response", content = %redacted_content);
            }
        }

        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard(Vec<(String, Option<String>)>);

    impl EnvGuard {
        fn set(values: &[(&str, Option<&str>)]) -> Self {
            let previous = values
                .iter()
                .map(|(name, _)| ((*name).to_string(), std::env::var(name).ok()))
                .collect();
            for (name, value) in values {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            Self(previous)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[derive(Clone)]
    struct LegacyModel;

    impl Model for LegacyModel {
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<AssistantMessage, BoxErr> {
            Ok(AssistantMessage {
                content: "legacy".into(),
                tool_calls: Vec::new(),
                finish_reason: "stop".into(),
                reasoning_content: None,
                completion: crate::reasoning::CompletionTelemetry::default(),
            })
        }

        fn clone_box(&self) -> Box<dyn Model> {
            Box::new(self.clone())
        }
    }

    #[test]
    fn completion_options_default_to_legacy_complete_implementation() {
        let result = LegacyModel
            .complete_with_options(
                &[Message::user("hello")],
                &[],
                &crate::reasoning::CompletionOptions::default(),
            )
            .unwrap();
        assert_eq!(result.content, "legacy");
    }

    #[test]
    fn from_env_reads_reasoning_mode() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::set(&[
            ("QUECTO_BASE_URL", Some("http://localhost:1234/v1")),
            ("QUECTO_MODEL", Some("reasoning-model")),
            ("QUECTO_REASONING_MODE", Some("high")),
        ]);

        let model = HttpModel::from_env();

        assert_eq!(model.url, "http://localhost:1234/v1/chat/completions");
        assert_eq!(
            model.default_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::High)
        );
    }

    #[test]
    fn try_from_env_rejects_invalid_reasoning_mode() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::set(&[("QUECTO_REASONING_MODE", Some("turbo"))]);

        let error = HttpModel::try_from_env().err().unwrap();

        assert!(error.to_string().contains("unknown reasoning mode: turbo"));
    }

    #[test]
    fn completion_options_override_model_default() {
        let options = crate::reasoning::CompletionOptions {
            reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
        };
        let effective =
            effective_reasoning_mode(Some(crate::reasoning::ReasoningMode::Low), &options);
        assert_eq!(effective, Some(crate::reasoning::ReasoningMode::High));
    }

    #[test]
    fn completion_options_fall_back_to_model_default() {
        let options = crate::reasoning::CompletionOptions::default();
        let effective =
            effective_reasoning_mode(Some(crate::reasoning::ReasoningMode::Medium), &options);
        assert_eq!(effective, Some(crate::reasoning::ReasoningMode::Medium));
    }

    #[test]
    fn completion_options_can_change_reasoning_mode_across_calls() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = std::sync::mpsc::channel::<Value>();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                let header_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0, "request ended before headers");
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
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
                    assert!(read > 0, "request ended before body");
                    request.extend_from_slice(&buffer[..read]);
                }
                request_tx
                    .send(serde_json::from_slice(&request[header_end..]).unwrap())
                    .unwrap();

                let body = r#"{"choices":[{"message":{"content":"ok","reasoning_content":"thinking"},"finish_reason":"stop"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });

        let model = HttpModel {
            url: format!("http://{address}/v1/chat/completions"),
            api_key: None,
            model: "test-model".into(),
            default_reasoning_mode: Some(crate::reasoning::ReasoningMode::Low),
        };
        let transcript = [Message::user("same transcript")];
        let low = model
            .complete_with_options(
                &transcript,
                &[],
                &crate::reasoning::CompletionOptions::default(),
            )
            .unwrap();
        let high = model
            .complete_with_options(
                &transcript,
                &[],
                &crate::reasoning::CompletionOptions {
                    reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
                },
            )
            .unwrap();

        assert_eq!(
            low.completion.requested_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::Low)
        );
        assert_eq!(
            high.completion.requested_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::High)
        );
        assert!(low.completion.reasoning_parameters_sent);
        assert!(high.completion.reasoning_parameters_sent);
        assert!(low.completion.reasoning_content_available);
        assert!(high.completion.reasoning_content_available);
        let low_request = request_rx.recv().unwrap();
        assert_eq!(low_request["reasoning_effort"], "low");
        assert!(low_request.get("reasoning").is_none());
        let high_request = request_rx.recv().unwrap();
        assert_eq!(high_request["reasoning_effort"], "high");
        assert!(high_request.get("reasoning").is_none());
        server.join().unwrap();
    }

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
    fn injects_reasoning_payload_into_request_body() {
        let mut body = messages_to_body("m", &[Message::user("u")]);
        let payload = crate::reasoning::apply_reasoning_mode(
            &mut body,
            "https://api.openai.com/v1/chat/completions",
            Some(crate::reasoning::ReasoningMode::Low),
        )
        .unwrap();
        assert_eq!(payload, json!({"reasoning_effort": "low"}));
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn leaves_body_unchanged_when_reasoning_mode_is_absent() {
        let mut body = messages_to_body("m", &[Message::user("u")]);
        let payload = crate::reasoning::apply_reasoning_mode(
            &mut body,
            "https://api.openai.com/v1/chat/completions",
            None,
        );
        assert!(payload.is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn does_not_report_parameters_sent_when_body_cannot_be_modified() {
        let mut body = Value::Null;
        let payload = crate::reasoning::apply_reasoning_mode(
            &mut body,
            "https://api.openai.com/v1/chat/completions",
            Some(crate::reasoning::ReasoningMode::Low),
        );
        assert!(payload.is_none());
        assert_eq!(body, Value::Null);
    }

    #[test]
    fn parses_reasoning_tokens_from_usage_metadata() {
        let resp = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens_details": {"reasoning_tokens": 42}}
        });
        assert_eq!(crate::reasoning::parse_reasoning_tokens(&resp), Some(42));
    }

    #[test]
    fn parses_reasoning_tokens_from_output_token_details() {
        let resp = json!({
            "usage": {"output_tokens_details": {"reasoning_tokens": 24}}
        });
        assert_eq!(crate::reasoning::parse_reasoning_tokens(&resp), Some(24));
    }

    #[test]
    fn parses_reasoning_tokens_from_provider_usage_field() {
        let resp = json!({"usage": {"reasoning_tokens": 12}});
        assert_eq!(crate::reasoning::parse_reasoning_tokens(&resp), Some(12));
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
        assert!(m.completion.reasoning_content_available);
    }

    #[test]
    fn parses_think_tags_in_content() {
        let r = json!({"choices":[{"message":{"content":"<think>\nthinking 456\n</think>\nhello"},"finish_reason":"stop"}]});
        let m = parse_assistant(&r).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.reasoning_content, Some("thinking 456".to_string()));
        assert!(m.completion.reasoning_content_available);
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
