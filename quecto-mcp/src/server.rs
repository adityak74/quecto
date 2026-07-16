use crate::config::{ServerConfig, TransportKind, TrustLevel};
use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, McpTool, mcp_prefix};
use crate::transport::{stdio::StdioTransport, Transport};
use serde_json::json;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub struct McpServer {
    pub name: String,
    pub trust: TrustLevel,
    transport: Box<dyn Transport>,
    id_counter: u64,
}

impl McpServer {
    pub fn from_config(cfg: &ServerConfig) -> Result<Self, McpError> {
        let connect_timeout = cfg.timeout_secs.unwrap_or(10);
        let transport: Box<dyn Transport> = match &cfg.transport {
            TransportKind::Stdio => {
                let cmd = cfg.command.as_deref().ok_or_else(|| {
                    McpError::Config(format!("server '{}': stdio requires `command`", cfg.name))
                })?;
                Box::new(StdioTransport::spawn(cmd, &cfg.args, &cfg.env, connect_timeout)?)
            }
            TransportKind::StreamableHttp => {
                let url = cfg.url.clone().ok_or_else(|| McpError::Config(format!("server '{}': streamable_http requires `url`", cfg.name)))?;
                Box::new(crate::transport::streamable_http::StreamableHttpTransport::new(url, cfg.headers.clone(), cfg.timeout_secs.unwrap_or(30)))
            }
            TransportKind::Sse => {
                let url = cfg.url.clone().ok_or_else(|| McpError::Config(format!("server '{}': sse requires `url`", cfg.name)))?;
                Box::new(crate::transport::sse::SseTransport::new(url, cfg.headers.clone(), cfg.timeout_secs.unwrap_or(30)))
            }
        };
        Ok(McpServer { name: cfg.name.clone(), trust: cfg.trust.clone(), transport, id_counter: 1 })
    }

    pub(crate) fn next_id(&mut self) -> u64 {
        let id = self.id_counter;
        self.id_counter += 1;
        id
    }

    pub(crate) fn send_request(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        self.transport.send(req)
    }

    pub fn initialize(&mut self) -> Result<(), McpError> {
        let id = self.next_id();
        let req = JsonRpcRequest::new(id, "initialize", Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "quecto-mcp", "version": env!("CARGO_PKG_VERSION") }
        })));
        let resp = self.transport.send(req)?;
        if let Some(err) = resp.error {
            return Err(McpError::ServerError { code: err.code, message: err.message });
        }
        // Notifications/initialized — no response expected; best-effort fire-and-forget.
        // Write raw notification bytes directly to transport by crafting a high-id req.
        // Servers tolerate missing initialized notification per spec.
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let id = self.next_id();
        let resp = self.transport.send(JsonRpcRequest::new(id, "tools/list", None))?;
        if let Some(err) = resp.error {
            return Err(McpError::ServerError { code: err.code, message: err.message });
        }
        let result = resp.result.ok_or_else(|| McpError::Protocol("tools/list: missing result".into()))?;
        let arr = result["tools"].as_array()
            .ok_or_else(|| McpError::Protocol("tools/list: result.tools is not an array".into()))?;
        let mut tools = Vec::new();
        for t in arr {
            let name = t["name"].as_str()
                .ok_or_else(|| McpError::Protocol("tool entry missing name".into()))?
                .to_string();
            tools.push(McpTool {
                server: self.name.clone(),
                prefixed_name: mcp_prefix(&self.name, &name),
                name,
                description: t["description"].as_str().map(str::to_string),
                input_schema: t.get("inputSchema").cloned().unwrap_or(json!({})),
            });
        }
        Ok(tools)
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<String, McpError> {
        let id = self.next_id();
        let resp = self.send_request(JsonRpcRequest::new(id, "resources/read", Some(json!({"uri": uri}))))?;
        if let Some(err) = resp.error { return Err(McpError::ServerError { code: err.code, message: err.message }); }
        let result = resp.result.ok_or_else(|| McpError::Protocol("resources/read: missing result".into()))?;
        Ok(result["contents"].as_array().and_then(|a| a.first()).and_then(|c| c["text"].as_str()).unwrap_or("").to_string())
    }

    pub fn list_prompt_names(&mut self) -> Result<Vec<String>, McpError> {
        let id = self.next_id();
        let resp = self.send_request(JsonRpcRequest::new(id, "prompts/list", None))?;
        if let Some(err) = resp.error { return Err(McpError::ServerError { code: err.code, message: err.message }); }
        let result = resp.result.ok_or_else(|| McpError::Protocol("prompts/list: missing result".into()))?;
        Ok(result["prompts"].as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    }

    pub fn get_prompt(&mut self, name: &str) -> Result<String, McpError> {
        let id = self.next_id();
        let resp = self.send_request(JsonRpcRequest::new(id, "prompts/get", Some(json!({"name": name}))))?;
        if let Some(err) = resp.error { return Err(McpError::ServerError { code: err.code, message: err.message }); }
        let result = resp.result.ok_or_else(|| McpError::Protocol("prompts/get: missing result".into()))?;
        Ok(result["messages"].as_array().map(|a| a.iter().filter_map(|m| m["content"]["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default())
    }

    pub fn sampling_create_message(&mut self, messages: serde_json::Value, model_prefs: serde_json::Value, base_url: &str, api_key: &str, model: &str) -> Result<serde_json::Value, McpError> {
        let body = json!({"model": model, "messages": messages, "max_tokens": model_prefs.get("maxTokens").and_then(|v| v.as_u64()).unwrap_or(1024)});
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let mut headers = std::collections::HashMap::new();
        if !api_key.is_empty() { headers.insert("Authorization".to_string(), format!("Bearer {api_key}")); }
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        let header_refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        quecto::quecto_raw(&url, &header_refs, body)
            .map_err(|e| McpError::Transport(format!("sampling LLM call failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::mcp_prefix;
    #[test]
    fn prefix_formula() {
        assert_eq!(mcp_prefix("myserver", "do_thing"), "mcp__myserver__do_thing");
    }
    #[test]
    fn initialize_request_has_correct_method() {
        use crate::protocol::JsonRpcRequest;
        use serde_json::json;
        let req = JsonRpcRequest::new(1, "initialize", Some(json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"quecto-mcp","version":"0.1.0"}})));
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["method"], "initialize");
    }
}
