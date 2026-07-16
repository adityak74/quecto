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
