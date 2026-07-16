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
