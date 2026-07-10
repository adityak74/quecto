//! quecto — the smallest harness of all time.
//! Core: quecto_raw / quecto_stream / quecto_to / quecto, plus small pub helpers
//! (build_body, join_url, env_config, extract_content, init_exports) reused by the
//! binary and future companion crates.

use serde_json::{json, Value};
use std::time::Duration;

/// Shared boxed error: every fallible fn returns this. Both ureq::Error and
/// serde_json::Error satisfy it, so `?` composes and errors cross into async tasks.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// Build an OpenAI-style chat body: optional system message + one user message.
pub fn build_body(system: Option<&str>, prompt: &str, model: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(s) = system {
        messages.push(json!({"role": "system", "content": s}));
    }
    messages.push(json!({"role": "user", "content": prompt}));
    json!({"model": model, "messages": messages})
}

/// Join a base URL and a path with exactly one slash, tolerating trailing/leading
/// slashes on either side (so `…/v1` and `…/v1/` both work).
pub fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// Extract assistant text from a buffered chat response. Errors only when there
/// are no choices; a present-but-null/absent content yields "" (tool-call turns).
pub fn extract_content(resp: &Value) -> Result<String, BoxErr> {
    let choices = resp
        .get("choices")
        .and_then(|c| c.as_array())
        .filter(|a| !a.is_empty())
        .ok_or("no choices in response")?;
    Ok(choices[0]["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Parse one SSE `data:` payload into its `choices[0].delta` object.
/// Returns None for `[DONE]`, unparseable JSON, or a chunk without a delta.
pub(crate) fn parse_sse_delta(data: &str) -> Option<Value> {
    if data == "[DONE]" {
        return None;
    }
    let chunk: Value = serde_json::from_str(data).ok()?;
    chunk.get("choices")?.get(0)?.get("delta").cloned()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(60))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// Buffered primitive: POST an arbitrary JSON body to an arbitrary URL with
/// arbitrary headers; return the full parsed response. No path/auth/shape opinions.
/// `ureq` returns `Err` on non-2xx status, so no explicit status check is needed.
pub fn quecto_raw(url: &str, headers: &[(&str, &str)], body: Value) -> Result<Value, BoxErr> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body)?;
    let value: Value = resp.into_json()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_user_only() {
        let b = build_body(None, "hi", "m");
        assert_eq!(b["model"], "m");
        assert_eq!(b["messages"].as_array().unwrap().len(), 1);
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"], "hi");
    }

    #[test]
    fn build_body_with_system() {
        let b = build_body(Some("sys"), "hi", "m");
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "sys");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn join_url_variants() {
        assert_eq!(join_url("http://x/v1", "chat/completions"), "http://x/v1/chat/completions");
        assert_eq!(join_url("http://x/v1/", "chat/completions"), "http://x/v1/chat/completions");
        assert_eq!(join_url("http://x/v1", "/chat/completions"), "http://x/v1/chat/completions");
    }

    #[test]
    fn extract_content_ok() {
        let r = json!({"choices":[{"message":{"content":"hello"}}]});
        assert_eq!(extract_content(&r).unwrap(), "hello");
    }

    #[test]
    fn extract_content_null_is_empty() {
        let r = json!({"choices":[{"message":{"tool_calls":[]}}]});
        assert_eq!(extract_content(&r).unwrap(), "");
    }

    #[test]
    fn extract_content_no_choices_errs() {
        let r = json!({"error":"x"});
        assert!(extract_content(&r).is_err());
    }

    #[test]
    fn parse_sse_delta_content() {
        let d = parse_sse_delta(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        assert_eq!(d["content"], "hi");
    }

    #[test]
    fn parse_sse_delta_done_none() {
        assert!(parse_sse_delta("[DONE]").is_none());
    }

    #[test]
    fn parse_sse_delta_bad_json_none() {
        assert!(parse_sse_delta("not json").is_none());
    }

    #[test]
    fn parse_sse_delta_no_delta_none() {
        assert!(parse_sse_delta(r#"{"choices":[{}]}"#).is_none());
    }
}
