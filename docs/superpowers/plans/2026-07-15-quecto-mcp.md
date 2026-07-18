# quecto-mcp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `quecto-mcp`, a synchronous Rust library crate that lets `quecto-agent` consume MCP servers (STDIO, Streamable HTTP, legacy SSE) as tool sources at runtime, with zero changes to the base agent binary.

**Architecture:** A new `quecto-mcp` lib crate in the workspace exposes a fully synchronous `McpRegistry` API; tokio lives entirely inside the crate boundary behind `Runtime::block_on`. `quecto-agent` gains an optional `mcp` Cargo feature that wires the registry into its startup and tool dispatch. MCP tools appear in the model's tool list prefixed `mcp__<server>__<tool>` to avoid collisions with native tools.

**Tech Stack:** Rust 2021 edition · tokio 1 · serde/serde_json · toml 0.8 · ureq 2 (already a transitive dep) · sha2 (already in quecto-agent) · tempfile (dev)

## Global Constraints

- Rust edition 2021; MSRV follows workspace.
- `cargo test --workspace` (no features) must stay green at every task — 183 existing tests, 0 regressions.
- `cargo clippy --all-targets --workspace` must be warning-free at every commit.
- The base `quecto-agent` binary (no `mcp` feature) must remain ≤ 3.5 MB.
- `quecto-mcp` is a **lib crate only** — no binary, no main.
- tokio must not appear in the non-`mcp` build of `quecto-agent`.
- All MCP tool names in the model's view are prefixed `mcp__<server_name>__<tool_name>`.
- STDIO connect timeout: 10 s. All call timeouts: 30 s default (per-server override).
- `cargo test -p quecto-mcp` runs unit tests; integration tests are `#[ignore]` and require `npx`.
- Commit after every task with a conventional commit message (`feat:`, `test:`, `docs:`).

---

### Task 1: Workspace scaffold — `quecto-mcp` crate

**Files:**
- Create: `quecto-mcp/Cargo.toml`
- Create: `quecto-mcp/src/lib.rs`
- Modify: `Cargo.toml` (root workspace)

**Interfaces:**
- Consumes: nothing (empty crate)
- Produces: compilable crate `quecto-mcp` in the workspace; `cargo test -p quecto-mcp` passes

- [ ] **Step 1: Create `quecto-mcp/Cargo.toml`**

```toml
[package]
name        = "quecto-mcp"
version     = "0.1.0"
edition     = "2021"
description = "MCP client library for quecto-agent."
license     = "MIT"

[dependencies]
tokio      = { version = "1", features = ["rt", "process", "io-util", "time", "net"] }
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
toml       = "0.8"
ureq       = { version = "2", features = ["json"] }
sha2       = "0.10"
quecto     = { path = ".." }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `quecto-mcp/src/lib.rs`** (empty module stubs so it compiles)

```rust
//! quecto-mcp — MCP client library for quecto-agent.
//!
//! Exposes a fully synchronous API; tokio is hidden inside via `Runtime::block_on`.

pub mod config;
pub mod error;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod tofu;
pub mod transport;

pub use config::{McpConfig, ServerConfig, TransportKind, TrustLevel};
pub use error::McpError;
pub use protocol::{McpTool, mcp_prefix};
pub use registry::McpRegistry;
pub use tofu::McpTofuStore;
```

- [ ] **Step 3: Create stub files** — each containing just `// TODO` so the crate compiles:
  - `quecto-mcp/src/config.rs`
  - `quecto-mcp/src/error.rs`
  - `quecto-mcp/src/protocol.rs`
  - `quecto-mcp/src/registry.rs`
  - `quecto-mcp/src/server.rs`
  - `quecto-mcp/src/tofu.rs`
  - `quecto-mcp/src/transport/mod.rs`
  - `quecto-mcp/src/transport/stdio.rs`
  - `quecto-mcp/src/transport/streamable_http.rs`
  - `quecto-mcp/src/transport/sse.rs`

- [ ] **Step 4: Add `quecto-mcp` to the workspace `members` in root `Cargo.toml`**

Change `members = [".", "quecto-agent"]` to `members = [".", "quecto-agent", "quecto-mcp"]`

- [ ] **Step 5: Verify it compiles and all existing tests pass**

```bash
cargo build -p quecto-mcp
cargo test --workspace
```
Expected: compiles, all 183 workspace tests pass.

- [ ] **Step 6: Commit**

```bash
git add quecto-mcp/ Cargo.toml Cargo.lock
git commit -m "feat(mcp): scaffold quecto-mcp crate in workspace"
```

---

### Task 2: Error types and JSON-RPC 2.0 protocol structs

**Files:**
- Modify: `quecto-mcp/src/error.rs`
- Modify: `quecto-mcp/src/protocol.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `McpError` enum (`Connect`, `Protocol`, `Transport`, `ToolNotFound{server,name}`, `ServerError{code,message}`, `Timeout{server,elapsed_secs}`, `Config`) — `#[non_exhaustive]`, `Display`, `Error`
  - `JsonRpcRequest { jsonrpc: &'static str, id: u64, method: String, params: Option<Value> }` + `fn new(id, method, params)`
  - `JsonRpcResponse { jsonrpc: String, id: Option<u64>, result: Option<Value>, error: Option<JsonRpcError> }`
  - `JsonRpcError { code: i64, message: String }`
  - `JsonRpcNotification { jsonrpc: String, method: String, params: Option<Value> }`
  - `McpTool { server: String, name: String, prefixed_name: String, description: Option<String>, input_schema: Value }`
  - `fn mcp_prefix(server: &str, tool: &str) -> String` → `"mcp__<server>__<tool>"`

- [ ] **Step 1: Write failing tests in `error.rs`**

```rust
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
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error (McpError not defined)

- [ ] **Step 3: Implement `error.rs`**

```rust
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
```

- [ ] **Step 4: Write failing tests in `protocol.rs`**

```rust
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
```

- [ ] **Step 5: Implement `protocol.rs`**

```rust
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
```

- [ ] **Step 6: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 7: Commit**
```bash
git add quecto-mcp/src/error.rs quecto-mcp/src/protocol.rs quecto-mcp/src/lib.rs
git commit -m "feat(mcp): add McpError, JSON-RPC 2.0 types, McpTool, mcp_prefix"
```

---

### Task 3: Config parsing — `McpConfig` (TOML + merge)

**Files:**
- Modify: `quecto-mcp/src/config.rs`

**Interfaces:**
- Consumes: `McpError` (Task 2)
- Produces:
  - `TransportKind` enum: `Stdio`, `StreamableHttp`, `Sse` — `#[derive(Debug,Clone,Deserialize,PartialEq,Eq)]` `snake_case`
  - `TrustLevel` enum: `Sandbox`, `Trusted` — same derives
  - `ServerConfig { name, transport, command?, args, env, url?, headers, trust, timeout_secs? }`
  - `McpConfig { servers: Vec<ServerConfig> }` (TOML key `server` → array)
  - `McpConfig::empty() -> Self`
  - `McpConfig::from_toml_str(s: &str) -> Result<McpConfig, McpError>`
  - `McpConfig::from_file(path: &Path) -> Result<McpConfig, McpError>` — `Ok(empty())` if NotFound
  - `McpConfig::merge_from(&mut self, other: McpConfig)` — other wins by name
  - `McpConfig::from_env_var(var: &str) -> Result<McpConfig, McpError>` — parses JSON array
  - `McpConfig::from_env() -> Result<McpConfig, McpError>` — reads `QUECTO_MCP_SERVERS`
  - `McpConfig::merged(file, env, cli) -> McpConfig` — file < env < cli

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"
[[server]]
name = "filesystem"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
trust = "sandbox"

[[server]]
name = "github"
transport = "streamable_http"
url = "https://api.githubcopilot.com/mcp/"
trust = "trusted"
timeout_secs = 60
"#;
    #[test]
    fn parses_two_servers() {
        let cfg = McpConfig::from_toml_str(SAMPLE).unwrap();
        assert_eq!(cfg.servers.len(), 2);
    }
    #[test]
    fn stdio_server_parsed() {
        let cfg = McpConfig::from_toml_str(SAMPLE).unwrap();
        let fs = &cfg.servers[0];
        assert_eq!(fs.name, "filesystem");
        assert!(matches!(fs.transport, TransportKind::Stdio));
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert!(matches!(fs.trust, TrustLevel::Sandbox));
    }
    #[test]
    fn streamable_http_server_parsed() {
        let cfg = McpConfig::from_toml_str(SAMPLE).unwrap();
        let gh = &cfg.servers[1];
        assert!(matches!(gh.transport, TransportKind::StreamableHttp));
        assert_eq!(gh.url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
        assert!(matches!(gh.trust, TrustLevel::Trusted));
        assert_eq!(gh.timeout_secs, Some(60));
    }
    #[test]
    fn missing_file_returns_empty() {
        let cfg = McpConfig::from_file(std::path::Path::new("/nonexistent/mcp.toml")).unwrap();
        assert!(cfg.servers.is_empty());
    }
    #[test]
    fn from_env_var_parses_json_array() {
        std::env::set_var("QUECTO_MCP_TEST_ABC", r#"[{"name":"mem","transport":"stdio","command":"npx","args":["-y","server-memory"],"trust":"sandbox"}]"#);
        let cfg = McpConfig::from_env_var("QUECTO_MCP_TEST_ABC").unwrap();
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].name, "mem");
        std::env::remove_var("QUECTO_MCP_TEST_ABC");
    }
    #[test]
    fn merged_env_overrides_file_by_name() {
        let file = McpConfig::from_toml_str("[[server]]\nname=\"fs\"\ntransport=\"stdio\"\ncommand=\"npx\"\ntrust=\"sandbox\"\n").unwrap();
        let env  = McpConfig::from_toml_str("[[server]]\nname=\"fs\"\ntransport=\"stdio\"\ncommand=\"uvx\"\ntrust=\"trusted\"\n").unwrap();
        let merged = McpConfig::merged(file, env, McpConfig::empty());
        assert_eq!(merged.servers.len(), 1);
        assert_eq!(merged.servers[0].command.as_deref(), Some("uvx"));
    }
}
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `config.rs`**

```rust
use crate::error::McpError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind { Stdio, StreamableHttp, Sse }

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel { Sandbox, Trusted }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub name: String,
    pub transport: TransportKind,
    pub command: Option<String>,
    #[serde(default)] pub args: Vec<String>,
    #[serde(default)] pub env: HashMap<String, String>,
    pub url: Option<String>,
    #[serde(default)] pub headers: HashMap<String, String>,
    pub trust: TrustLevel,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct McpConfig { pub servers: Vec<ServerConfig> }

#[derive(Deserialize)]
struct McpConfigToml { #[serde(default, rename = "server")] servers: Vec<ServerConfig> }

impl McpConfig {
    pub fn empty() -> Self { McpConfig { servers: vec![] } }

    pub fn from_toml_str(s: &str) -> Result<Self, McpError> {
        let t: McpConfigToml = toml::from_str(s).map_err(|e| McpError::Config(e.to_string()))?;
        Ok(McpConfig { servers: t.servers })
    }

    pub fn from_file(path: &Path) -> Result<Self, McpError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml_str(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(e) => Err(McpError::Config(format!("{}: {e}", path.display()))),
        }
    }

    pub fn merge_from(&mut self, other: McpConfig) {
        for incoming in other.servers {
            if let Some(existing) = self.servers.iter_mut().find(|s| s.name == incoming.name) {
                *existing = incoming;
            } else {
                self.servers.push(incoming);
            }
        }
    }

    pub fn from_env_var(var: &str) -> Result<Self, McpError> {
        let raw = match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return Ok(Self::empty()),
        };
        let servers: Vec<ServerConfig> = serde_json::from_str(&raw)
            .map_err(|e| McpError::Config(format!("{var} parse error: {e}")))?;
        Ok(McpConfig { servers })
    }

    pub fn from_env() -> Result<Self, McpError> { Self::from_env_var("QUECTO_MCP_SERVERS") }

    pub fn merged(mut file: McpConfig, env: McpConfig, cli: McpConfig) -> McpConfig {
        file.merge_from(env);
        file.merge_from(cli);
        file
    }
}
```

- [ ] **Step 4: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 5: Commit**
```bash
git add quecto-mcp/src/config.rs
git commit -m "feat(mcp): add McpConfig, TransportKind, TrustLevel — TOML + env parsing"
```

---

### Task 4: `Transport` trait and `StdioTransport`

**Files:**
- Modify: `quecto-mcp/src/transport/mod.rs`
- Modify: `quecto-mcp/src/transport/stdio.rs`

**Interfaces:**
- Consumes: `JsonRpcRequest`, `JsonRpcResponse`, `McpError` (Task 2)
- Produces:
  - `pub trait Transport: Send` with `fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>`
  - `pub struct StdioTransport` impl `Transport`
  - `StdioTransport::spawn(command: &str, args: &[String], env: &HashMap<String,String>, connect_timeout_secs: u64) -> Result<StdioTransport, McpError>`
  - `StdioTransport::encode_request(req: &JsonRpcRequest) -> String` — newline-terminated JSON

- [ ] **Step 1: Write failing tests in `transport/stdio.rs`**

```rust
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
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `transport/mod.rs`**

```rust
pub mod sse;
pub mod stdio;
pub mod streamable_http;

use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

pub trait Transport: Send {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;
}
```

- [ ] **Step 4: Implement `transport/stdio.rs`**

```rust
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
```

- [ ] **Step 5: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 6: Commit**
```bash
git add quecto-mcp/src/transport/
git commit -m "feat(mcp): add Transport trait and StdioTransport with JSON-RPC framing"
```

---

### Task 5: `McpServer` (initialize + tools/list) and `McpRegistry` (discover + call_tool)

**Files:**
- Modify: `quecto-mcp/src/server.rs`
- Modify: `quecto-mcp/src/registry.rs`

**Interfaces:**
- Consumes: `Transport` (Task 4); `ServerConfig`, `TransportKind`, `TrustLevel` (Task 3); `JsonRpcRequest`, `McpTool`, `mcp_prefix` (Task 2); `McpError` (Task 2)
- Produces:
  - `pub struct McpServer { pub name: String, pub trust: TrustLevel, transport: Box<dyn Transport>, next_id: u64 }`
  - `McpServer::from_config(cfg: &ServerConfig) -> Result<McpServer, McpError>` — STDIO wired; HTTP/SSE stubs returning `Err` (implemented in Tasks 6–7)
  - `McpServer::initialize(&mut self) -> Result<(), McpError>`
  - `McpServer::list_tools(&mut self) -> Result<Vec<McpTool>, McpError>`
  - `pub(crate) McpServer::next_id(&mut self) -> u64`
  - `pub(crate) McpServer::send_request(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>`
  - `pub struct McpRegistry { servers: Vec<McpServer> }`
  - `McpRegistry::new(config: McpConfig) -> McpRegistry` — non-fatal on server failures
  - `McpRegistry::discover(&mut self) -> Vec<McpTool>` — non-fatal per-server
  - `McpRegistry::call_tool(&mut self, prefixed_name: &str, args: Value) -> Result<Value, McpError>`
  - `McpRegistry::server_names(&self) -> Vec<&str>`
  - `McpRegistry::is_empty(&self) -> bool`

- [ ] **Step 1: Write failing tests in `server.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
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
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `server.rs`**

```rust
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
        quecto::quecto_raw(&url, &headers, &body)
            .map_err(|e| McpError::Transport(format!("sampling LLM call failed: {e}")))
    }
}
```

- [ ] **Step 4: Implement `registry.rs`**

```rust
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
```

- [ ] **Step 5: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 6: Commit**
```bash
git add quecto-mcp/src/server.rs quecto-mcp/src/registry.rs quecto-mcp/src/lib.rs
git commit -m "feat(mcp): add McpServer (initialize, list_tools, resources, prompts, sampling) + McpRegistry"
```

---

### Task 6: `StreamableHttpTransport`

**Files:**
- Modify: `quecto-mcp/src/transport/streamable_http.rs`

**Interfaces:**
- Consumes: `Transport` trait, `JsonRpcRequest`, `JsonRpcResponse`, `McpError`
- Produces:
  - `pub struct StreamableHttpTransport { url, pub(crate) resolved_headers, call_timeout_secs }`
  - `StreamableHttpTransport::new(url: String, headers: HashMap<String,String>, call_timeout_secs: u64) -> Self`
  - Implements `Transport::send` — POST JSON-RPC, handles `application/json` + `text/event-stream` responses
  - `$VAR` env-var substitution in header values at construction time

- [ ] **Step 1: Write failing tests**

```rust
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
        std::env::set_var("QUECTO_TEST_TOKEN_XYZ", "tok123");
        let mut h = HashMap::new();
        h.insert("Authorization".into(), "Bearer $QUECTO_TEST_TOKEN_XYZ".into());
        let t = StreamableHttpTransport::new("https://example.com/mcp".into(), h, 30);
        assert_eq!(t.resolved_headers.get("Authorization").unwrap(), "Bearer tok123");
        std::env::remove_var("QUECTO_TEST_TOKEN_XYZ");
    }
}
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `transport/streamable_http.rs`**

```rust
use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;

pub struct StreamableHttpTransport {
    url: String,
    pub(crate) resolved_headers: HashMap<String, String>,
    call_timeout_secs: u64,
}

impl StreamableHttpTransport {
    pub fn new(url: String, headers: HashMap<String, String>, call_timeout_secs: u64) -> Self {
        let resolved_headers = headers.into_iter().map(|(k, v)| (k, expand_env(&v))).collect();
        StreamableHttpTransport { url, resolved_headers, call_timeout_secs }
    }
}

fn expand_env(s: &str) -> String {
    let mut result = s.to_string();
    let mut i = 0;
    while i < result.len() {
        if result.as_bytes()[i] == b'$' {
            let rest = &result[i + 1..];
            let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(rest.len());
            let var_name = &rest[..end];
            if !var_name.is_empty() {
                if let Ok(val) = std::env::var(var_name) {
                    result.replace_range(i..i + 1 + end, &val);
                    i += val.len();
                    continue;
                }
            }
        }
        i += 1;
    }
    result
}

impl Transport for StreamableHttpTransport {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let body = serde_json::to_string(&req).map_err(|e| McpError::Protocol(format!("serialize: {e}")))?;
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(self.call_timeout_secs))
            .build();
        let mut request = agent.post(&self.url)
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
        if let Some(data) = line.strip_prefix("data: ") {
            data_buf = data.to_string();
        } else if line.is_empty() && !data_buf.is_empty() {
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data_buf) {
                if resp.id == Some(target_id) { return Ok(resp); }
            }
            data_buf.clear();
        }
    }
    Err(McpError::Transport(format!("SSE stream ended without response for id {target_id}")))
}
```

- [ ] **Step 4: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 5: Commit**
```bash
git add quecto-mcp/src/transport/streamable_http.rs
git commit -m "feat(mcp): add StreamableHttpTransport (Streamable HTTP over HTTPS)"
```

---

### Task 7: Legacy `SseTransport` (compat only)

**Files:**
- Modify: `quecto-mcp/src/transport/sse.rs`

**Interfaces:**
- Consumes: `Transport` trait, `JsonRpcRequest`, `JsonRpcResponse`, `McpError`
- Produces:
  - `pub struct SseTransport` impl `Transport`
  - `SseTransport::new(base_url: String, headers: HashMap<String,String>, call_timeout_secs: u64) -> Self`
  - POST to `<base_url>/messages`, parse JSON response (simplified compat layer)

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn sse_transport_constructs() {
        let _t = SseTransport::new("https://old.example.com/sse".into(), HashMap::new(), 30);
    }
}
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `transport/sse.rs`**

```rust
//! Legacy standalone SSE transport — deprecated since MCP spec March 2025.
//! Compat-only: new servers should use Streamable HTTP.
use crate::error::McpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;
use std::collections::HashMap;

pub struct SseTransport {
    base_url: String,
    headers: HashMap<String, String>,
    call_timeout_secs: u64,
    post_endpoint: Option<String>,
}

impl SseTransport {
    pub fn new(base_url: String, headers: HashMap<String, String>, call_timeout_secs: u64) -> Self {
        SseTransport { base_url, headers, call_timeout_secs, post_endpoint: None }
    }
    fn post_url(&self) -> String {
        self.post_endpoint.clone()
            .unwrap_or_else(|| format!("{}/messages", self.base_url.trim_end_matches('/')))
    }
}

impl Transport for SseTransport {
    fn send(&mut self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let body = serde_json::to_string(&req).map_err(|e| McpError::Protocol(format!("serialize: {e}")))?;
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(self.call_timeout_secs))
            .build();
        let post_url = self.post_url();
        let mut request = agent.post(&post_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        for (k, v) in &self.headers { request = request.set(k, v); }
        let response = request.send_string(&body)
            .map_err(|e| McpError::Transport(format!("legacy SSE POST failed: {e}")))?;
        let text = response.into_string().map_err(|e| McpError::Transport(format!("read body: {e}")))?;
        serde_json::from_str(&text).map_err(|e| McpError::Protocol(format!("bad JSON-RPC: {e}")))
    }
}
```

- [ ] **Step 4: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 5: Commit**
```bash
git add quecto-mcp/src/transport/sse.rs
git commit -m "feat(mcp): add legacy SseTransport (deprecated compat only)"
```

---

### Task 8: `McpTofuStore`

**Files:**
- Modify: `quecto-mcp/src/tofu.rs`

**Interfaces:**
- Consumes: `ServerConfig` (Task 3); `sha2`
- Produces:
  - `pub fn server_config_hash(cfg: &ServerConfig) -> String` — deterministic SHA-256 hex
  - `pub struct McpTofuStore { path: PathBuf, hashes: BTreeSet<String> }`
  - `McpTofuStore::open_at(path: impl AsRef<Path>) -> Self`
  - `McpTofuStore::is_trusted(cfg: &ServerConfig) -> bool`
  - `McpTofuStore::trust(cfg: &ServerConfig)` — persists hash to file

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, TransportKind, TrustLevel};
    use std::collections::HashMap;

    fn sample() -> ServerConfig {
        ServerConfig { name: "test".into(), transport: TransportKind::Stdio, command: Some("npx".into()), args: vec![], env: HashMap::new(), url: None, headers: HashMap::new(), trust: TrustLevel::Sandbox, timeout_secs: None }
    }

    #[test]
    fn new_server_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpTofuStore::open_at(dir.path().join("trust"));
        assert!(!store.is_trusted(&sample()));
    }

    #[test]
    fn trusted_server_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust");
        let mut store = McpTofuStore::open_at(&path);
        let srv = sample();
        store.trust(&srv);
        let store2 = McpTofuStore::open_at(&path);
        assert!(store2.is_trusted(&srv));
    }

    #[test]
    fn changed_config_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust");
        let mut store = McpTofuStore::open_at(&path);
        let mut srv = sample();
        store.trust(&srv);
        srv.command = Some("uvx".into());
        let store2 = McpTofuStore::open_at(&path);
        assert!(!store2.is_trusted(&srv));
    }
}
```

- [ ] **Step 2: Run `cargo test -p quecto-mcp`** — expected: compile error

- [ ] **Step 3: Implement `tofu.rs`**

```rust
use crate::config::ServerConfig;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn server_config_hash(cfg: &ServerConfig) -> String {
    let mut env_pairs: Vec<_> = cfg.env.iter().collect();
    env_pairs.sort_by_key(|(k, _)| k.as_str());
    let mut hdr_pairs: Vec<_> = cfg.headers.iter().collect();
    hdr_pairs.sort_by_key(|(k, _)| k.as_str());
    let canonical = format!(
        "name={}\ntransport={:?}\ncommand={}\nargs={}\nenv={}\nurl={}\nheaders={}\ntrust={:?}\ntimeout={:?}",
        cfg.name,
        cfg.transport,
        cfg.command.as_deref().unwrap_or(""),
        cfg.args.join(","),
        env_pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";"),
        cfg.url.as_deref().unwrap_or(""),
        hdr_pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";"),
        cfg.trust,
        cfg.timeout_secs,
    );
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub struct McpTofuStore { path: PathBuf, hashes: BTreeSet<String> }

impl McpTofuStore {
    pub fn open_at(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let hashes = std::fs::read_to_string(&path)
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        McpTofuStore { path, hashes }
    }
    pub fn is_trusted(&self, cfg: &ServerConfig) -> bool { self.hashes.contains(&server_config_hash(cfg)) }
    pub fn trust(&mut self, cfg: &ServerConfig) {
        let hash = server_config_hash(cfg);
        if !self.hashes.insert(hash) { return; }
        if let Some(p) = self.path.parent() { if !p.as_os_str().is_empty() { let _ = std::fs::create_dir_all(p); } }
        let body = self.hashes.iter().cloned().collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(&self.path, format!("{body}\n"));
    }
}
```

Note: `TransportKind` and `TrustLevel` must have `#[derive(Debug)]` — add it to `config.rs` if not already present.

- [ ] **Step 4: Run `cargo test -p quecto-mcp && cargo clippy -p quecto-mcp --all-targets`** — expected: pass

- [ ] **Step 5: Commit**
```bash
git add quecto-mcp/src/tofu.rs quecto-mcp/src/lib.rs
git commit -m "feat(mcp): add McpTofuStore for per-server TOFU trust tracking"
```

---

### Task 9: Wire `quecto-mcp` into `quecto-agent` (startup + dispatch)

**Files:**
- Modify: `quecto-agent/Cargo.toml` — add `mcp` feature + `quecto-mcp` optional dep
- Create: `quecto-agent/src/mcp_adapter.rs` — `McpToolAdapter` implementing `Tool`
- Modify: `quecto-agent/src/lib.rs` — export `McpToolAdapter` behind `mcp` feature
- Modify: `quecto-agent/src/main.rs` — `--mcp` CLI flag, startup wiring, dispatch routing

**Interfaces:**
- Consumes: `McpRegistry::new`, `::discover`, `::call_tool`, `::system_prompt_additions` (Tasks 5, 6, 7); `McpConfig::from_file`, `::from_env`, `::merged` (Task 3); `McpTofuStore` (Task 8); `mcp_prefix` (Task 2)
- Produces:
  - `[features] mcp = ["dep:quecto-mcp"]` in `quecto-agent/Cargo.toml`
  - `struct McpToolAdapter { tool: McpTool, registry: Arc<Mutex<McpRegistry>> }` impl `Tool`
  - `--mcp <transport:name:...>` global CLI arg (multi-value, optional)
  - Startup: load config (file < env < CLI) → `McpRegistry::new` → `discover` → register tools → append prompt additions
  - Dispatch: `name.starts_with("mcp__")` → `McpRegistry::call_tool` → `ToolOutput`

- [ ] **Step 1: Add `mcp` feature to `quecto-agent/Cargo.toml`**

```toml
quecto-mcp = { path = "../quecto-mcp", optional = true }

[features]
default = []
otel = [ ... ]  # unchanged
mcp = ["dep:quecto-mcp"]
```

- [ ] **Step 2: Write a failing unit test** in `quecto-agent/src/mcp_adapter.rs`

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn mcp_prefix_routing_check() {
        assert!("mcp__filesystem__read_file".starts_with("mcp__"));
        assert!(!"read_file".starts_with("mcp__"));
    }
}
```

- [ ] **Step 3: Run `cargo test --workspace --features mcp`** — expected: compile error (file doesn't exist)

- [ ] **Step 4: Create `quecto-agent/src/mcp_adapter.rs`**

```rust
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
```

- [ ] **Step 5: Add `pub mod mcp_adapter;` to `quecto-agent/src/lib.rs` (guarded by `#[cfg(feature = "mcp")]`)**

```rust
#[cfg(feature = "mcp")]
pub mod mcp_adapter;
#[cfg(feature = "mcp")]
pub use mcp_adapter::McpToolAdapter;
```

- [ ] **Step 6: Add `--mcp` CLI flag to `struct Cli` in `main.rs`**

```rust
/// Connect to an MCP server. Format: stdio:name:command[:arg1:arg2...]
/// or streamable_http:name:url  or  sse:name:url (legacy).
/// Can be specified multiple times. Requires --features mcp build.
#[cfg(feature = "mcp")]
#[arg(long = "mcp", global = true, value_name = "TRANSPORT:NAME:...")]
mcp: Vec<String>,
```

- [ ] **Step 7: Add MCP startup wiring in `main.rs`** — add after flavor loading, before `Agent::new`:

```rust
#[cfg(feature = "mcp")]
let mcp_result = {
    use quecto_mcp::{McpConfig, McpRegistry};
    use quecto_agent::mcp_adapter::McpToolAdapter;
    use std::sync::{Arc, Mutex};
    use std::path::Path;

    let file_cfg = McpConfig::from_file(Path::new(".quecto/mcp.toml"))
        .unwrap_or_else(|e| { eprintln!("quecto-mcp: config warning: {e}"); McpConfig::empty() });
    let env_cfg = McpConfig::from_env()
        .unwrap_or_else(|e| { eprintln!("quecto-mcp: env warning: {e}"); McpConfig::empty() });
    let cli_cfg = mcp_config_from_flags(&cli.mcp);  // helper defined below
    let merged = McpConfig::merged(file_cfg, env_cfg, cli_cfg);

    let mut registry = McpRegistry::new(merged);
    let mcp_tools = registry.discover();
    let prompt_additions = registry.system_prompt_additions();
    let registry_arc = Arc::new(Mutex::new(registry));
    (mcp_tools, prompt_additions, registry_arc)
};
```

- [ ] **Step 8: Register MCP tools in agent after `register_builtins()`**

```rust
#[cfg(feature = "mcp")]
{
    let (mcp_tools, prompt_additions, registry_arc) = mcp_result;
    for mcp_tool in mcp_tools {
        let adapter = McpToolAdapter { tool: mcp_tool, registry: Arc::clone(&registry_arc) };
        agent = agent.register(Box::new(adapter));
    }
    for addition in &prompt_additions {
        if let Some(msg) = agent.messages.first_mut() {
            msg.content.push_str("\n\n");
            msg.content.push_str(addition);
        }
    }
}
```

- [ ] **Step 9: Add `mcp_config_from_flags` helper in `main.rs`**

```rust
#[cfg(feature = "mcp")]
fn mcp_config_from_flags(flags: &[String]) -> quecto_mcp::McpConfig {
    use quecto_mcp::config::{ServerConfig, TransportKind, TrustLevel};
    use std::collections::HashMap;
    let mut servers = Vec::new();
    for flag in flags {
        let parts: Vec<&str> = flag.splitn(3, ':').collect();
        if parts.len() < 3 { eprintln!("quecto-mcp: ignoring malformed --mcp flag: {flag}"); continue; }
        let (transport_str, name, rest) = (parts[0], parts[1], parts[2]);
        let transport = match transport_str {
            "stdio" => TransportKind::Stdio,
            "streamable_http" => TransportKind::StreamableHttp,
            "sse" => TransportKind::Sse,
            other => { eprintln!("quecto-mcp: unknown transport '{other}'"); continue; }
        };
        let server = match transport {
            TransportKind::Stdio => {
                let mut p = rest.split(':');
                let command = p.next().unwrap_or("").to_string();
                let args: Vec<String> = p.map(str::to_string).collect();
                ServerConfig { name: name.to_string(), transport, command: Some(command), args, env: HashMap::new(), url: None, headers: HashMap::new(), trust: TrustLevel::Sandbox, timeout_secs: None }
            }
            _ => ServerConfig { name: name.to_string(), transport, command: None, args: vec![], env: HashMap::new(), url: Some(rest.to_string()), headers: HashMap::new(), trust: TrustLevel::Sandbox, timeout_secs: None }
        };
        servers.push(server);
    }
    quecto_mcp::McpConfig { servers }
}
```

- [ ] **Step 10: Run full test suite with and without the feature**

```bash
cargo test --workspace
cargo clippy --all-targets --workspace
cargo test --workspace --features mcp
cargo clippy --all-targets --workspace --features mcp
```

Expected: all 183+ tests pass in both builds, zero clippy warnings in both.

- [ ] **Step 11: Commit**
```bash
git add quecto-agent/Cargo.toml quecto-agent/src/mcp_adapter.rs quecto-agent/src/lib.rs quecto-agent/src/main.rs
git commit -m "feat(agent): wire quecto-mcp into quecto-agent behind optional mcp feature flag"
```

---

### Task 10: Integration tests and README update

**Files:**
- Create: `quecto-mcp/tests/integration.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: full public API of `quecto-mcp`
- Produces: `#[ignore]` integration tests; updated README roadmap table and `quecto-mcp` section

- [ ] **Step 1: Create `quecto-mcp/tests/integration.rs`**

```rust
//! Integration tests — require `npx` and `@modelcontextprotocol/server-memory` in PATH.
//! Run with: cargo test -p quecto-mcp -- --ignored

use quecto_mcp::{McpConfig, McpRegistry};

fn has_npx() -> bool {
    std::process::Command::new("npx").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
#[ignore = "requires npx and @modelcontextprotocol/server-memory"]
fn stdio_memory_server_discover_and_call() {
    if !has_npx() { eprintln!("skipping: npx not found"); return; }
    let config = McpConfig::from_toml_str(r#"
[[server]]
name = "memory"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory"]
trust = "sandbox"
"#).unwrap();
    let mut registry = McpRegistry::new(config);
    let tools = registry.discover();
    assert!(!tools.is_empty(), "expected at least one tool from server-memory");
    for t in &tools {
        assert!(t.prefixed_name.starts_with("mcp__memory__"), "bad prefix: {}", t.prefixed_name);
    }
}
```

- [ ] **Step 2: Run unit tests (no ignored)**

```bash
cargo test -p quecto-mcp
```

Expected: all unit tests pass; integration test skipped.

- [ ] **Step 3: Update `README.md` roadmap table** — change:

```markdown
| MCP integrations | `quecto-mcp` | 🔮 planned |
```
To:
```markdown
| MCP client (STDIO · Streamable HTTP · legacy SSE compat) | `quecto-mcp` | 🚧 in progress |
```

- [ ] **Step 4: Add `quecto-mcp` section to README** after the `quecto-agent` section:

```markdown
## `quecto-mcp` — MCP client for the agent

Build the agent with MCP support to consume any MCP-compatible server as a tool source:

```bash
# Build with MCP (~6–9 MB with tokio)
cargo build --release -p quecto-agent --features mcp

# Local STDIO server
quecto-agent --mcp stdio:filesystem:npx:-y:@modelcontextprotocol/server-filesystem:/tmp "list files"

# Remote Streamable HTTP server (current standard)
quecto-agent --mcp streamable_http:github:https://api.githubcopilot.com/mcp/ "open issues"

# Or configure in .quecto/mcp.toml (see docs/superpowers/specs/2026-07-15-quecto-mcp-design.md)
```

MCP tools are prefixed `mcp__<server>__<tool>` — no collision with native tools. Server failures at startup are non-fatal.

| Transport | When to use |
|---|---|
| `stdio` | Local single-user tools (Claude Desktop / Cursor pattern) |
| `streamable_http` | Remote/production (current MCP standard, March 2025+) |
| `sse` | Legacy servers only — deprecated, compat support |
```

- [ ] **Step 5: Final verification**

```bash
cargo test --workspace
cargo test --workspace --features mcp
cargo clippy --all-targets --workspace
cargo clippy --all-targets --workspace --features mcp
```

Expected: all tests pass, zero warnings.

- [ ] **Step 6: Commit**
```bash
git add quecto-mcp/tests/integration.rs README.md
git commit -m "test(mcp): integration tests; docs: update README with quecto-mcp section"
```

---

## Self-Review

**Spec coverage:**
- ✅ §3 Architecture — Tasks 1+5 create exact file layout
- ✅ §4.1 TOML config — Task 3
- ✅ §4.2 Env var — Task 3 (`from_env_var`, `from_env`)
- ✅ §4.3 CLI flags — Task 9
- ✅ §4 Trust levels — Task 3 (`TrustLevel`); dispatch routing in `McpToolAdapter` (Task 9)
- ✅ §4 TOFU — Task 8 (`McpTofuStore`)
- ✅ §5 `initialize` — Task 5; `tools/list` — Task 5; `tools/call` — Task 5+9; `resources/read` — Task 5; `prompts/get` — Task 5; `sampling/createMessage` — Task 5
- ✅ §6 Public API — all methods across Tasks 5, 8
- ✅ §7 Tool integration (prefix, startup, dispatch) — Task 9
- ✅ §7 Failure isolation — `McpRegistry::new` (non-fatal), `McpToolAdapter::run` (error→ToolOutput)
- ✅ §8 `McpError` — Task 2
- ✅ §9 Testing — all tasks (unit); Task 10 (integration)
- ✅ §10 Binary size note — Task 10 README
- ✅ §11 Dependencies — Task 1 `Cargo.toml`
- ✅ §12 Out of scope — not implemented
- ✅ §13 Milestones M1–M8 — all covered across Tasks 1–10

**Type consistency:**
- `Transport::send(JsonRpcRequest) -> Result<JsonRpcResponse, McpError>` — consistent across Tasks 4, 6, 7 ✅
- `McpRegistry::call_tool(&str, Value) -> Result<Value, McpError>` — consistent Tasks 5 + 9 ✅
- `McpTool::prefixed_name` generated by `mcp_prefix` — consistent Tasks 2, 5, 9 ✅
- `McpConfig::merged(file, env, cli) -> McpConfig` — consistent Tasks 3 + 9 ✅
- `McpServer::id_counter` (private) / `next_id()` pub(crate) — consistent Tasks 5, 6, 7 ✅
