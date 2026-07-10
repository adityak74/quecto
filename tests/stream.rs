mod common;
use common::mock;
use serde_json::json;

#[test]
fn stream_accumulates_sse() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n";
    let base = mock(200, "text/event-stream", sse);
    let url = quecto::join_url(&base, "chat/completions");
    let mut seen = 0;
    let out = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |_d| seen += 1).unwrap();
    assert_eq!(out, "Hello");
    assert_eq!(seen, 2);
}

#[test]
fn stream_non_sse_fallback() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"whole"}}]}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let mut calls = 0;
    let out = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |d| {
        calls += 1;
        assert_eq!(d["content"], "whole");
    }).unwrap();
    assert_eq!(out, "whole");
    assert_eq!(calls, 1);
}

#[test]
fn stream_skips_leading_comment_line() {
    // Proxy emits an SSE comment/heartbeat before the first data frame.
    let sse = ": keep-alive\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
    let base = mock(200, "text/event-stream", sse);
    let url = quecto::join_url(&base, "chat/completions");
    let out = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |_d| {}).unwrap();
    assert_eq!(out, "ok");
}

#[test]
fn stream_empty_body_errors() {
    let base = mock(200, "text/event-stream", "");
    let url = quecto::join_url(&base, "chat/completions");
    let r = quecto::quecto_stream(&url, &[], json!({"model":"m","messages":[]}), |_d| {});
    assert!(r.is_err());
}
