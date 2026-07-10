mod common;
use common::mock;
use serde_json::json;

#[test]
fn raw_returns_full_value() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"hi"}}]}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let resp = quecto::quecto_raw(&url, &[], json!({"model":"m","messages":[]})).unwrap();
    assert_eq!(resp["choices"][0]["message"]["content"], "hi");
}

#[test]
fn raw_non_2xx_is_err() {
    let base = mock(400, "application/json", r#"{"error":"bad"}"#);
    let url = quecto::join_url(&base, "chat/completions");
    let r = quecto::quecto_raw(&url, &[], json!({}));
    assert!(r.is_err());
}
