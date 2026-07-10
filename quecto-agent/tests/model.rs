mod common;

use common::mock;
use quecto_agent::{HttpModel, Message, Model};

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
    let msg = m.complete(&[Message::user("hey")]).unwrap();
    assert_eq!(msg.content, "hi");
    assert!(msg.tool_calls.is_empty());
}
