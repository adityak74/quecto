use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;

pub struct StreamableHttpTransport;

impl StreamableHttpTransport {
    pub fn new(_url: String, _headers: HashMap<String, String>, _timeout: u64) -> Self {
        Self
    }
}

impl Transport for StreamableHttpTransport {
    fn send(&mut self, _req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        Err(McpError::Transport("streamable_http transport not yet implemented".into()))
    }
}
