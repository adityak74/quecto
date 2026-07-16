# quecto-mcp Design Spec

**Date:** 2026-07-15  
**Status:** Approved — pending implementation plan  
**Author:** Aditya (via brainstorming session)

> **Transport note (2026-07-15 correction):** The MCP ecosystem currently uses two active transports:
> - **STDIO** — for local, single-user servers (subprocess stdin/stdout). Used by Claude Desktop, Cursor, VS Code.
> - **Streamable HTTP over HTTPS** — for remote/production servers; the current standard since the March 2025 protocol revision. The endpoint returns `application/json` or `text/event-stream` (SSE *inside* Streamable HTTP) depending on whether streaming is needed.
> - **Legacy standalone SSE** — deprecated since March 2025; supported here for compatibility with older servers only. Do not build new servers around it.
>
> HTTPS is not a separate MCP transport — it is Streamable HTTP carried over TLS.

---

## 1. Overview

`quecto-mcp` is a new Rust **library crate** that gives `quecto-agent` the ability to consume MCP (Model Context Protocol) servers as tool sources at runtime. The agent discovers and connects to MCP servers on startup, merges their tools into its native tool registry, and routes model `tool_call`s to the appropriate server transparently.

**What does not change:**
- The `quecto` core crate — untouched, zero-async, two dependencies.
- The `quecto-agent` loop, session model, sandbox, and flavors — unchanged in the non-`mcp` build.
- The existing 183 passing tests — `cargo test --workspace` (without `mcp` feature) stays green.

---

## 2. Goals

1. Let developers plug any MCP-compatible server (local or remote) into `quecto-agent` without forking.
2. Support both STDIO (subprocess) and Streamable HTTP over HTTPS transports. Provide legacy standalone SSE support for compatibility with older servers only.
3. Keep the agent loop **synchronous** — async is isolated inside `quecto-mcp`.
4. Reuse existing patterns: TOFU trust, config-file-primary with env/CLI overrides, denylist sandbox.
5. The base agent binary (no `mcp` feature) must stay at ≤ 3.5 MB.

---

## 3. Architecture

```
workspace/
├── src/                        # quecto core (unchanged)
├── quecto-agent/               # agent (gains optional `mcp` feature)
│   └── Cargo.toml              # quecto-mcp = { path = "../quecto-mcp", optional = true }
└── quecto-mcp/                 # NEW: MCP client library crate
    ├── Cargo.toml
    └── src/
        ├── lib.rs              # public API re-exports
        ├── transport/
        │   ├── mod.rs          # Transport trait
        │   ├── stdio.rs        # spawn child process, communicate via stdin/stdout
        │   ├── streamable_http.rs  # Streamable HTTP over HTTPS (current standard)
        │   └── sse.rs          # legacy standalone SSE (deprecated, compat only)
        ├── protocol.rs         # JSON-RPC 2.0 types + MCP schema structs
        ├── registry.rs         # McpRegistry: connect, discover, dispatch
        └── config.rs           # parse .quecto/mcp.toml, env, CLI overrides
```

### Key constraints

- `quecto-mcp` is a **lib crate only** — no binary.
- tokio lives entirely inside `quecto-mcp`. The public API surface is **fully synchronous**: every method blocks via `tokio::runtime::Runtime::block_on`.
- `quecto-agent` enables the feature with `--features mcp`; without it, zero MCP code compiles in.

---

## 4. Configuration

### 4.1 Primary: `.quecto/mcp.toml`

Lives alongside `.quecto/flavors/`. Same directory, same TOFU trust model.

```toml
# Local server — STDIO (subprocess). Preferred for single-user/local tools.
[[server]]
name        = "filesystem"
transport   = "stdio"
command     = "npx"
args        = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
env         = {}
trust       = "sandbox"        # "sandbox" | "trusted"
timeout_secs = 30              # optional, default: 30 for calls, 10 for connect

# Remote/production server — Streamable HTTP over HTTPS (current standard).
# Returns application/json or text/event-stream depending on whether streaming is needed.
[[server]]
name        = "github"
transport   = "streamable_http"  # Streamable HTTP over HTTPS
url         = "https://api.githubcopilot.com/mcp/"
headers     = { Authorization = "Bearer $GITHUB_TOKEN" }  # $VAR env substitution
trust       = "trusted"
timeout_secs = 30

# Legacy-only: standalone SSE (deprecated March 2025 — use only for older servers).
# [[server]]
# name        = "legacy-server"
# transport   = "sse"           # legacy, compat only — not recommended for new servers
# url         = "https://old-server.example.com/sse"
# trust       = "sandbox"
```

**Trust levels:**
- `sandbox` — MCP tool calls are routed through quecto-agent's existing hard-denylist before execution.
- `trusted` — MCP tool calls bypass the denylist. Operator explicitly opts in per server.

**Trust-on-first-use (TOFU):** The first time a server's config block is loaded, its content-hash is recorded in `$QUECTO_TRUST_FILE` (same file as flavors). Any config change triggers a re-confirmation prompt before the server is used.

### 4.2 Override: `QUECTO_MCP_SERVERS` env var

JSON array of server spec objects (same shape as TOML). Merges with the config file by `name`; env entries win on conflict.

```bash
export QUECTO_MCP_SERVERS='[{"name":"memory","transport":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-memory"],"trust":"sandbox"}]'
```

### 4.3 Override: CLI flags on `quecto-agent`

```bash
# STDIO server (local)
quecto-agent --mcp stdio:filesystem:npx:-y:@modelcontextprotocol/server-filesystem:/tmp

# Streamable HTTP over HTTPS (remote/production — current standard)
quecto-agent --mcp streamable_http:github:https://api.githubcopilot.com/mcp/

# Legacy standalone SSE (deprecated — compatibility only)
quecto-agent --mcp sse:legacy-server:https://old-server.example.com/sse
```

CLI flags are the highest-priority override. Multiple `--mcp` flags are allowed.

---

## 5. MCP Protocol Coverage

| MCP Method | Purpose | When |
|---|---|---|
| `initialize` | Handshake, negotiate capabilities | On connect |
| `tools/list` | Discover tools → `Vec<McpTool>` | Startup |
| `tools/call` | Invoke tool by name + JSON args | On model `tool_call` |
| `resources/read` | Fetch resource URI → content string | On model `tool_call` (resource tools) |
| `prompts/get` | Fetch named prompt text | Startup → appended to system prompt |
| `sampling/createMessage` | MCP server calls back into LLM | Handled by `quecto_raw` call inside the registry |

All messages are JSON-RPC 2.0. Framing details by transport:

| Transport | Framing | Notes |
|---|---|---|
| `stdio` | Newline-delimited JSON on stdin/stdout | Local subprocess; most common for developer tools |
| `streamable_http` | POST to MCP endpoint; response is `application/json` or `text/event-stream` | Current standard for remote servers (March 2025+) |
| `sse` (legacy) | GET to SSE endpoint; `text/event-stream` | Deprecated; retained for compatibility with older servers |

---

## 6. Public API (`quecto-mcp`)

All methods are **synchronous** from the caller's perspective.

```rust
/// A discovered MCP tool, ready to be merged into the agent's tool list.
pub struct McpTool {
    pub server:      String,          // server name from config
    pub name:        String,          // tool name as reported by the server
    pub prefixed_name: String,        // "mcp__<server>__<name>" — used in the registry
    pub description: Option<String>,
    pub input_schema: serde_json::Value, // JSON Schema for args
}

/// Top-level registry: owns all server connections for a session.
pub struct McpRegistry { /* ... */ }

impl McpRegistry {
    /// Load config, connect to servers, perform MCP initialize handshake.
    pub fn new(config: McpConfig) -> Result<Self, McpError>;

    /// Call tools/list on all servers; returns merged, prefixed tool list.
    pub fn discover(&mut self) -> Result<Vec<McpTool>, McpError>;

    /// Route a tool call to the correct server; return the JSON result.
    pub fn call_tool(&self, prefixed_name: &str, args: serde_json::Value) -> Result<serde_json::Value, McpError>;

    /// Return system-prompt additions from prompts/get on all servers.
    pub fn system_prompt_additions(&self) -> Vec<String>;

    /// Return resource content for a given URI.
    pub fn read_resource(&self, server: &str, uri: &str) -> Result<String, McpError>;
}
```

---

## 7. Tool Integration in `quecto-agent`

### Namespace

MCP tools are prefixed `mcp__<server_name>__<tool_name>` in the registry the model sees:

- Model sees: `mcp__filesystem__read_file`
- Registry strips prefix when routing to `McpRegistry::call_tool`

This prevents collisions with native agent tools (`read_file`, `write_file`, etc.).

### Startup sequence (with `mcp` feature)

1. Parse config from `.quecto/mcp.toml` + env + CLI flags.
2. `McpRegistry::new()` — connect to servers, run `initialize`.
3. `registry.discover()` — call `tools/list` on all servers; merge results into the agent's native tool list.
4. `registry.system_prompt_additions()` — append any server prompts to the base system prompt (after repo rules, same as flavors).
5. Agent loop runs as normal — the model sees native + MCP tools together.

### Tool call routing (per agent step)

```
model returns tool_call(name="mcp__filesystem__read_file", args={...})
  → agent loop checks: does name start with "mcp__"?
    → yes: route to registry.call_tool(name, args)
    → no:  route to native tool handler
```

### Failure isolation

- If a server fails to connect at startup: print warning, skip that server's tools, continue. Non-fatal.
- If `call_tool` fails for an MCP tool: return `{"error": "..."}` as the tool result so the model can recover. Non-fatal to the session.

---

## 8. Error Handling

```rust
#[non_exhaustive]
pub enum McpError {
    Connect(String),
    Protocol(String),
    Transport(String),
    ToolNotFound { server: String, name: String },
    ServerError { code: i64, message: String },
    Timeout { server: String, elapsed_secs: u64 },
    Config(String),
}
```

**Timeouts:**
- Stdio connect: 10 s (configurable via `timeout_secs` for `connect_timeout_secs` in future)
- All calls (stdio + HTTP): 30 s default, per-server override via `timeout_secs` in config

---

## 9. Testing Strategy

### Unit tests (no real MCP server required)

- Mock `Transport` trait implementations for JSON-RPC round-trips
- Config parsing: TOML, env var JSON, CLI flag precedence
- TOFU hash recording and change-detection
- Namespace prefixing and routing logic
- `McpError` display/formatting

### Integration tests (real MCP server, `#[ignore]` by default)

File: `quecto-mcp/tests/integration.rs`

Run with:
```bash
cargo test --features mcp -p quecto-mcp -- --ignored
```

Requires `npx` and `@modelcontextprotocol/server-memory` (stdio) available in PATH.

### Existing test suite

```bash
cargo test --workspace        # 183 tests, mcp feature OFF — must stay green
cargo clippy --all-targets --workspace
```

The `mcp` feature is additive. No existing test must change.

---

## 10. Binary Size Impact

| Build | Approximate size |
|---|---|
| `quecto-agent` (no `mcp`) | ~3.5 MB (unchanged) |
| `quecto-agent --features mcp` | ~6–9 MB (tokio added) |
| `quecto-mcp` (lib only) | n/a |

The larger size with `mcp` is expected and acceptable. It is documented. The default build stays at 3.5 MB.

---

## 11. Dependencies (`quecto-mcp/Cargo.toml`)

```toml
[dependencies]
tokio       = { version = "1", features = ["rt", "process", "io-util", "time", "net"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
toml        = "0.8"
# ureq for Streamable HTTP baseline request; tokio handles the SSE streaming leg
ureq        = { version = "2", features = ["json"] }

[dev-dependencies]
tempfile    = "3"
```

**Transport-to-crate mapping:**
- `stdio` → tokio `process` (spawn + async I/O)
- `streamable_http` → `ureq` for the POST, tokio for parsing the `text/event-stream` response body when streaming
- `sse` (legacy) → tokio `net` + manual SSE parser (no new dep needed)

`ureq` is already a transitive dep (from the core). Tokio is the only significant addition.

---

## 12. Out of Scope (explicitly)

- `quecto-mcp` acting as an **MCP server** (exposing quecto-agent's tools to external hosts) — not in this milestone.
- Dynamic reconnection / retry loops — connect once at startup; reconnect is a future milestone.
- MCP roots — not in the initial protocol surface.
- GUI or web dashboard for server status.

---

## 13. Milestones

| Milestone | Scope |
|---|---|
| M1 — Crate scaffold | `quecto-mcp` crate, `Transport` trait, STDIO transport, JSON-RPC framing, `initialize` handshake, unit tests |
| M2 — Tool discovery | `tools/list`, `McpRegistry::discover`, namespace prefixing, agent integration (feature flag), startup wiring |
| M3 — Tool invocation | `tools/call` routing in agent loop, sandbox trust routing, error recovery |
| M4 — Streamable HTTP transport | Streamable HTTP over HTTPS client (`streamable_http.rs`), handles `application/json` + `text/event-stream` responses, integration tests |
| M5 — Legacy SSE compat | `sse.rs` for old standalone SSE servers (compat only — no new server should target this) |
| M6 — Resources & prompts | `resources/read`, `prompts/get`, system prompt injection |
| M7 — Sampling | `sampling/createMessage` callback into `quecto_raw` |
| M8 — Config + TOFU | Full config surface (TOML + env + CLI), TOFU hash for servers, `timeout_secs`, README update |
