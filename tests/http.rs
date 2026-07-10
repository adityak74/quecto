mod common;
use common::mock;
use serde_json::json;
use std::sync::Mutex;

// Serializes the env-mutating test(s); other tests take explicit args and need no lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

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

#[test]
fn to_extracts_content() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"pong"}}]}"#);
    let out = quecto::quecto_to("ping", &base, None, "m").unwrap();
    assert_eq!(out, "pong");
}

#[test]
fn quecto_reads_env() {
    let _g = ENV_LOCK.lock().unwrap();
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"envd"}}]}"#);
    std::env::set_var("QUECTO_BASE_URL", &base);
    std::env::set_var("QUECTO_MODEL", "m");
    std::env::remove_var("QUECTO_API_KEY");
    std::env::remove_var("QUECTO_SYSTEM");
    let out = quecto::quecto("hi").unwrap();
    assert_eq!(out, "envd");
    std::env::remove_var("QUECTO_BASE_URL");
    std::env::remove_var("QUECTO_MODEL");
}
