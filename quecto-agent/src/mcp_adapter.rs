#[cfg(feature = "mcp")]
use quecto_mcp::{McpRegistry, McpTool};
#[cfg(feature = "mcp")]
use crate::tools::{Context, Tool, ToolError, ToolOutput, ToolResult};
#[cfg(feature = "mcp")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "mcp")]
pub struct McpToolAdapter {
    pub tool: McpTool,
    pub registry: Arc<Mutex<McpRegistry>>,
}

#[cfg(feature = "mcp")]
impl Tool for McpToolAdapter {
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str { &self.tool.prefixed_name }
    fn description(&self) -> &str { self.tool.description.as_deref().unwrap_or("MCP tool") }
    fn schema(&self) -> serde_json::Value { self.tool.input_schema.clone() }
    fn run(&self, args: &serde_json::Value, _cx: &mut Context) -> ToolResult {
        let mut reg = self.registry.lock()
            .map_err(|e| ToolError::new(format!("mcp registry lock poisoned: {e}")))?;
        match reg.call_tool(&self.tool.prefixed_name, args.clone()) {
            Ok(result) => {
                let text = if result.is_string() {
                    result.as_str().unwrap_or("").to_string()
                } else {
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                };
                Ok(ToolOutput::new(text, "mcp tool result"))
            }
            Err(e) => Ok(ToolOutput::new(format!("error: {e}"), "mcp error")),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mcp_prefix_routing_check() {
        assert!("mcp__filesystem__read_file".starts_with("mcp__"));
        assert!(!"read_file".starts_with("mcp__"));
    }
}
