pub mod sse;
pub mod stdio;
pub mod streamable_http;

use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

pub trait Transport: Send {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;
}
