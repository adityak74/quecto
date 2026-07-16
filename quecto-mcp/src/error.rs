use std::fmt;

#[non_exhaustive]
#[derive(Debug)]
pub enum McpError {
    Connect(String),
    Protocol(String),
    Transport(String),
    ToolNotFound { server: String, name: String },
    ServerError { code: i64, message: String },
    Timeout { server: String, elapsed_secs: u64 },
    Config(String),
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::Connect(msg)    => write!(f, "mcp connect error: {msg}"),
            McpError::Protocol(msg)   => write!(f, "mcp protocol error: {msg}"),
            McpError::Transport(msg)  => write!(f, "mcp transport error: {msg}"),
            McpError::ToolNotFound { server, name } => write!(f, "mcp tool not found: {server}/{name}"),
            McpError::ServerError { code, message } => write!(f, "mcp server error {code}: {message}"),
            McpError::Timeout { server, elapsed_secs } => write!(f, "mcp timeout after {elapsed_secs}s on server '{server}'"),
            McpError::Config(msg)     => write!(f, "mcp config error: {msg}"),
        }
    }
}

impl std::error::Error for McpError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_connect() {
        let e = McpError::Connect("refused".into());
        assert!(e.to_string().contains("connect"));
        assert!(e.to_string().contains("refused"));
    }
    #[test]
    fn display_tool_not_found() {
        let e = McpError::ToolNotFound { server: "fs".into(), name: "read".into() };
        assert!(e.to_string().contains("fs"));
        assert!(e.to_string().contains("read"));
    }
    #[test]
    fn display_timeout() {
        let e = McpError::Timeout { server: "s".into(), elapsed_secs: 30 };
        assert!(e.to_string().contains("30"));
    }
}
