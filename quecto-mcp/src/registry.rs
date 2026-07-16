use crate::config::McpConfig;
use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, McpTool};
use crate::server::McpServer;
use serde_json::json;

pub struct McpRegistry { servers: Vec<McpServer> }

impl McpRegistry {
    pub fn new(config: McpConfig) -> Self {
        let mut servers = Vec::new();
        for cfg in &config.servers {
            match McpServer::from_config(cfg) {
                Ok(mut srv) => match srv.initialize() {
                    Ok(()) => servers.push(srv),
                    Err(e) => eprintln!("quecto-mcp: warning: server '{}' initialize failed: {e}", cfg.name),
                },
                Err(e) => eprintln!("quecto-mcp: warning: could not connect to '{}': {e}", cfg.name),
            }
        }
        McpRegistry { servers }
    }

    pub fn discover(&mut self) -> Vec<McpTool> {
        let mut all = Vec::new();
        for srv in &mut self.servers {
            match srv.list_tools() {
                Ok(tools) => all.extend(tools),
                Err(e) => eprintln!("quecto-mcp: warning: tools/list on '{}': {e}", srv.name),
            }
        }
        all
    }

    pub fn call_tool(&mut self, prefixed_name: &str, args: serde_json::Value) -> Result<serde_json::Value, McpError> {
        let rest = prefixed_name.strip_prefix("mcp__")
            .ok_or_else(|| McpError::ToolNotFound { server: "?".into(), name: prefixed_name.into() })?;
        let (server_name, tool_name) = rest.split_once("__")
            .ok_or_else(|| McpError::ToolNotFound { server: rest.into(), name: "?".into() })?;
        let srv = self.servers.iter_mut().find(|s| s.name == server_name)
            .ok_or_else(|| McpError::ToolNotFound { server: server_name.into(), name: tool_name.into() })?;
        let id = srv.next_id();
        let req = JsonRpcRequest::new(id, "tools/call", Some(json!({"name": tool_name, "arguments": args})));
        let resp = srv.send_request(req)?;
        if let Some(err) = resp.error { return Err(McpError::ServerError { code: err.code, message: err.message }); }
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    pub fn read_resource(&mut self, server: &str, uri: &str) -> Result<String, McpError> {
        let srv = self.servers.iter_mut().find(|s| s.name == server)
            .ok_or_else(|| McpError::ToolNotFound { server: server.into(), name: uri.into() })?;
        srv.read_resource(uri)
    }

    pub fn system_prompt_additions(&mut self) -> Vec<String> {
        let mut additions = Vec::new();
        for srv in &mut self.servers {
            match srv.list_prompt_names() {
                Ok(names) => for name in names {
                    match srv.get_prompt(&name) {
                        Ok(text) if !text.is_empty() => additions.push(text),
                        Ok(_) => {}
                        Err(e) => eprintln!("quecto-mcp: prompts/get '{name}': {e}"),
                    }
                },
                Err(e) => eprintln!("quecto-mcp: prompts/list on '{}': {e}", srv.name),
            }
        }
        additions
    }

    pub fn sampling_create_message(&mut self, server: &str, messages: serde_json::Value, model_prefs: serde_json::Value, base_url: &str, api_key: &str, model: &str) -> Result<serde_json::Value, McpError> {
        let srv = self.servers.iter_mut().find(|s| s.name == server)
            .ok_or_else(|| McpError::ToolNotFound { server: server.into(), name: "sampling".into() })?;
        srv.sampling_create_message(messages, model_prefs, base_url, api_key, model)
    }

    pub fn server_names(&self) -> Vec<&str> { self.servers.iter().map(|s| s.name.as_str()).collect() }
    pub fn is_empty(&self) -> bool { self.servers.is_empty() }
}
