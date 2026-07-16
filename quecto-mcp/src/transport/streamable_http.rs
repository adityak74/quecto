use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;

pub struct StreamableHttpTransport {
    url: String,
    pub(crate) resolved_headers: HashMap<String, String>,
    agent: ureq::Agent,
}

impl StreamableHttpTransport {
    pub fn new(url: String, headers: HashMap<String, String>, call_timeout_secs: u64) -> Self {
        let resolved_headers = headers.into_iter().map(|(k, v)| (k, expand_env(&v, |k| std::env::var(k).ok()))).collect();
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(call_timeout_secs))
            .build();
        StreamableHttpTransport { url, resolved_headers, agent }
    }
}

fn expand_env<F>(s: &str, mut get_env: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let mut result = String::new();
    let mut remainder = s;
    while let Some(idx) = remainder.find('$') {
        result.push_str(&remainder[..idx]);
        let rest = &remainder[idx + 1..];
        let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(rest.len());
        let var_name = &rest[..end];
        if !var_name.is_empty() {
            if let Some(val) = get_env(var_name) {
                result.push_str(&val);
            } else {
                result.push('$');
                result.push_str(var_name);
            }
        } else {
            result.push('$');
        }
        remainder = &rest[end..];
    }
    result.push_str(remainder);
    result
}

impl Transport for StreamableHttpTransport {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let body = serde_json::to_string(&req).map_err(|e| McpError::Protocol(format!("serialize: {e}")))?;
        let mut request = self.agent.post(&self.url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        for (k, v) in &self.resolved_headers { request = request.set(k, v); }
        let response = request.send_string(&body)
            .map_err(|e| McpError::Transport(format!("HTTP POST failed: {e}")))?;
        let ct = response.header("content-type").unwrap_or("application/json").to_string();
        if ct.contains("text/event-stream") {
            parse_sse_response(response, req.id)
        } else {
            let text = response.into_string().map_err(|e| McpError::Transport(format!("read body: {e}")))?;
            serde_json::from_str(&text).map_err(|e| McpError::Protocol(format!("bad JSON-RPC: {e}")))
        }
    }
}

fn parse_sse_response(response: ureq::Response, target_id: u64) -> Result<JsonRpcResponse, McpError> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(response.into_reader());
    let mut data_buf = String::new();
    for line in reader.lines() {
        let line = line.map_err(|e| McpError::Transport(format!("SSE read: {e}")))?;
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            data_buf.push_str(data);
            data_buf.push('\n');
        } else if line.is_empty() && !data_buf.is_empty() {
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data_buf) {
                if resp.id == Some(target_id) { return Ok(resp); }
            }
            data_buf.clear();
        }
    }
    Err(McpError::Transport(format!("SSE stream ended without response for id {target_id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn constructor_does_not_panic() {
        let _t = StreamableHttpTransport::new("https://example.com/mcp".into(), HashMap::new(), 30);
    }
    #[test]
    fn env_var_substitution_in_header_value() {
        let resolved = expand_env("Bearer $QUECTO_TEST_TOKEN_XYZ", |k| {
            if k == "QUECTO_TEST_TOKEN_XYZ" {
                Some("tok123".into())
            } else {
                None
            }
        });
        assert_eq!(resolved, "Bearer tok123");
    }
}
