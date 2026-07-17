mod common;

use common::{mock, mock_capture};
use quecto_agent::{CompletionOptions, HttpModel, Message, Model, ReasoningMode};
use serde_json::{json, Value};

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
        default_reasoning_mode: None,
    };
    let msg = m.complete(&[Message::user("hey")], &[]).unwrap();
    assert_eq!(msg.content, "hi");
    assert!(msg.tool_calls.is_empty());
    assert!(!msg.completion.reasoning_parameters_sent);
    assert!(!msg.completion.reasoning_content_available);
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
        default_reasoning_mode: None,
    };

    let message = model
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
        message.completion.provider_reasoning_parameters,
        Some(json!({"reasoning_effort": "high"}))
    );
    assert!(message.completion.reasoning_parameters_sent);
    assert!(message.completion.reasoning_content_available);
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
        default_reasoning_mode: Some(ReasoningMode::Low),
    };

    let message = model.complete(&[Message::user("hey")], &[]).unwrap();
    let request = request.recv().unwrap();
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();

    assert!(body.get("reasoning_effort").is_none());
    assert!(message.completion.provider_reasoning_parameters.is_none());
    assert!(!message.completion.reasoning_parameters_sent);
}
