use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}
impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        JsonRpcRequest { jsonrpc: "2.0", id, method: method.into(), params }
    }
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError { pub code: i64, pub message: String }

#[derive(Debug, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String, pub method: String, pub params: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub prefixed_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

pub fn mcp_prefix(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn request_serialises_correctly() {
        let req = JsonRpcRequest::new(1, "tools/list", None);
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "tools/list");
    }
    #[test]
    fn response_result_parsed() {
        let raw = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
        let resp: JsonRpcResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }
    #[test]
    fn mcp_prefix_format() {
        assert_eq!(mcp_prefix("filesystem", "read_file"), "mcp__filesystem__read_file");
    }
    #[test]
    fn mcp_tool_prefixed_name_correct() {
        let t = McpTool {
            server: "fs".into(), name: "read_file".into(),
            prefixed_name: mcp_prefix("fs", "read_file"),
            description: None, input_schema: json!({}),
        };
        assert_eq!(t.prefixed_name, "mcp__fs__read_file");
    }
}
