//! Legacy standalone SSE transport — deprecated since MCP spec March 2025.
//! Compat-only: new servers should use Streamable HTTP.
use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;

pub struct SseTransport {
    base_url: String,
    headers: HashMap<String, String>,
    post_endpoint: Option<String>,
    agent: ureq::Agent,
}

impl SseTransport {
    pub fn new(base_url: String, headers: HashMap<String, String>, call_timeout_secs: u64) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(call_timeout_secs))
            .build();
            
        SseTransport { 
            base_url, 
            headers, 
            post_endpoint: None,
            agent,
        }
    }
    
    fn post_url(&self) -> String {
        self.post_endpoint.clone()
            .unwrap_or_else(|| format!("{}/messages", self.base_url.trim_end_matches('/')))
    }
}

impl Transport for SseTransport {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let body = serde_json::to_string(&req).map_err(|e| McpError::Protocol(format!("serialize: {e}")))?;
        
        let post_url = self.post_url();
        let mut request = self.agent.post(&post_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
            
        for (k, v) in &self.headers { 
            request = request.set(k, v); 
        }
        
        let response = request.send_string(&body)
            .map_err(|e| McpError::Transport(format!("legacy SSE POST failed: {e}")))?;
            
        let text = response.into_string().map_err(|e| McpError::Transport(format!("read body: {e}")))?;
        serde_json::from_str(&text).map_err(|e| McpError::Protocol(format!("bad JSON-RPC: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn sse_transport_constructs() {
        let _t = SseTransport::new("https://old.example.com/sse".into(), HashMap::new(), 30);
    }
}
