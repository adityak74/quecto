mod common;

use common::{mock, mock_capture};
use quecto_agent::{
    parse_assistant_completion, CompletionOptions, HttpModel, Message, Model, ReasoningMode,
};
use serde_json::{json, Value};

#[cfg(feature = "otel")]
use std::collections::HashMap;
#[cfg(feature = "otel")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "otel")]
use tracing::field::{Field, Visit};
#[cfg(feature = "otel")]
use tracing::span::{Attributes, Id, Record};
#[cfg(feature = "otel")]
use tracing::Subscriber;
#[cfg(feature = "otel")]
use tracing_subscriber::layer::{Context, SubscriberExt};
#[cfg(feature = "otel")]
use tracing_subscriber::registry::LookupSpan;
#[cfg(feature = "otel")]
use tracing_subscriber::Layer;

#[cfg(feature = "otel")]
#[derive(Clone)]
struct CaptureLayer {
    fields: Arc<Mutex<HashMap<String, String>>>,
}

#[cfg(feature = "otel")]
impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        attrs.record(&mut FieldVisitor(self.fields.clone()));
    }

    fn on_record(&self, _id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        values.record(&mut FieldVisitor(self.fields.clone()));
    }
}

#[cfg(feature = "otel")]
struct FieldVisitor(Arc<Mutex<HashMap<String, String>>>);

#[cfg(feature = "otel")]
impl Visit for FieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().into(), value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().into(), value.into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().into(), format!("{value:?}"));
    }
}

#[test]
fn http_model_completes_against_mock() {
    let base = mock(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#,
    );
    let m = HttpModel {
        url: format!("{base}/chat/completions"),
        api_key: None,
        model: "m".to_string(),
    };
    let msg = m.complete(&[Message::user("hey")], &[]).unwrap();
    assert_eq!(msg.content, "hi");
    assert!(msg.tool_calls.is_empty());
}

#[test]
fn chat_completions_sends_top_level_reasoning_effort() {
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"hi","reasoning_content":"work"},"finish_reason":"stop"}]}"#,
    );
    let model = HttpModel {
        url: format!("{base}/v1/chat/completions"),
        api_key: None,
        model: "reasoning-model".into(),
    };

    let completion = model
        .complete_with_options(
            &[Message::user("hey")],
            &[],
            &CompletionOptions {
                reasoning_mode: Some(ReasoningMode::High),
            },
        )
        .unwrap();
    let request = request.recv().unwrap();
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();

    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("reasoning").is_none());
    assert_eq!(
        completion.telemetry.provider_reasoning_parameters,
        Some(json!({"reasoning_effort": "high"}))
    );
    assert!(completion.telemetry.reasoning_parameters_sent);
    assert!(completion.telemetry.reasoning_content_available);
}

#[test]
fn unsupported_endpoint_omits_reasoning_parameters() {
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#,
    );
    let model = HttpModel {
        url: format!("{base}/v1/responses"),
        api_key: None,
        model: "reasoning-model".into(),
    };

    let completion = model
        .complete_with_options(
            &[Message::user("hey")],
            &[],
            &CompletionOptions {
                reasoning_mode: Some(ReasoningMode::Low),
            },
        )
        .unwrap();
    let request = request.recv().unwrap();
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();

    assert!(body.get("reasoning_effort").is_none());
    assert!(completion
        .telemetry
        .provider_reasoning_parameters
        .is_none());
    assert!(!completion.telemetry.reasoning_parameters_sent);
}

#[test]
fn parse_path_populates_response_only_telemetry() {
    let response = json!({
        "choices": [{"message": {"content": "hi"}, "finish_reason": "stop"}],
        "usage": {"completion_tokens_details": {"reasoning_tokens": 42}}
    });

    let completion = parse_assistant_completion(&response).unwrap();

    assert_eq!(completion.telemetry.actual_reasoning_tokens, Some(42));
    assert!(!completion.telemetry.reasoning_content_available);
}

#[cfg(feature = "otel")]
fn failed_completion_fields(status: u16, body: &str) -> HashMap<String, String> {
    let (base, request) = mock_capture(status, "application/json", body);
    let model = HttpModel {
        url: format!("{base}/v1/chat/completions"),
        api_key: None,
        model: "reasoning-model".into(),
    };
    let fields = Arc::new(Mutex::new(HashMap::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer {
        fields: fields.clone(),
    });

    let result = tracing::subscriber::with_default(subscriber, || {
        model.complete_with_options(
            &[Message::user("hey")],
            &[],
            &CompletionOptions {
                reasoning_mode: Some(ReasoningMode::High),
            },
        )
    });

    assert!(result.is_err());
    request.recv().unwrap();
    let captured = fields.lock().unwrap().clone();
    captured
}

#[cfg(feature = "otel")]
fn assert_request_reasoning_fields(fields: &HashMap<String, String>) {
    assert_eq!(fields["quecto.requested_reasoning_mode"], "high");
    assert_eq!(
        fields["quecto.provider_reasoning_parameters"],
        r#"{"reasoning_effort":"high"}"#
    );
    assert_eq!(fields["quecto.reasoning_parameters_sent"], "true");
}

#[cfg(feature = "otel")]
#[test]
fn http_error_preserves_request_reasoning_span_fields() {
    let fields = failed_completion_fields(500, r#"{"error":"provider failure"}"#);

    assert_request_reasoning_fields(&fields);
}

#[cfg(feature = "otel")]
#[test]
fn malformed_response_preserves_request_reasoning_span_fields() {
    let fields = failed_completion_fields(200, r#"{"choices":[]}"#);

    assert_request_reasoning_fields(&fields);
}
