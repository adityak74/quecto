//! quecto — the smallest harness of all time.
//! Core: quecto_raw / quecto_stream / quecto_to / quecto, plus small pub helpers
//! (build_body, join_url, env_config, extract_content, init_exports) reused by the
//! binary and future companion crates.

use serde_json::{json, Value};

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
}
