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
