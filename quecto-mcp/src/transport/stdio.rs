use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct StdioTransport {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    pub fn spawn(
        command: &str, args: &[String], env: &HashMap<String, String>, _connect_timeout_secs: u64,
    ) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args).envs(env)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()
            .map_err(|e| McpError::Connect(format!("failed to spawn '{command}': {e}")))?;
        let stdin = child.stdin.take()
            .ok_or_else(|| McpError::Connect("could not open stdin pipe".into()))?;
        let stdout_raw = child.stdout.take()
            .ok_or_else(|| McpError::Connect("could not open stdout pipe".into()))?;
        Ok(StdioTransport { _child: child, stdin, stdout: BufReader::new(stdout_raw) })
    }

    pub fn encode_request(req: &JsonRpcRequest) -> String {
        let mut s = serde_json::to_string(req).expect("serialize JsonRpcRequest");
        s.push('\n');
        s
    }
}

impl Transport for StdioTransport {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let encoded = Self::encode_request(&req);
        self.stdin.write_all(encoded.as_bytes())
            .map_err(|e| McpError::Transport(format!("stdin write: {e}")))?;
        self.stdin.flush()
            .map_err(|e| McpError::Transport(format!("stdin flush: {e}")))?;
        let target_id = req.id;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line)
                .map_err(|e| McpError::Transport(format!("stdout read: {e}")))?;
            if line.is_empty() {
                return Err(McpError::Transport("server closed stdout".into()));
            }
            let resp: JsonRpcResponse = serde_json::from_str(line.trim())
                .map_err(|e| McpError::Protocol(format!("bad JSON-RPC: {e}")))?;
            if resp.id == Some(target_id) { return Ok(resp); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcRequest;
    use std::collections::HashMap;

    #[test]
    fn encode_request_is_newline_terminated() {
        let req = JsonRpcRequest::new(42, "tools/list", None);
        let enc = StdioTransport::encode_request(&req);
        assert!(enc.ends_with('\n'));
        let _: serde_json::Value = serde_json::from_str(enc.trim_end()).unwrap();
    }

    #[test]
    fn spawn_invalid_command_returns_err() {
        let result = StdioTransport::spawn(
            "quecto_mcp_definitely_no_such_binary_xyz",
            &[],
            &HashMap::new(),
            5,
        );
        assert!(result.is_err());
    }
}
