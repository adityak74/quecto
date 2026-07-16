use std::collections::BTreeSet;
use std::path::PathBuf;

/// Remembers approved project-flavor content hashes, one lowercase-hex hash per
/// line in a small state file. Best-effort: I/O errors degrade to "not trusted"
/// and are never fatal.
pub struct TrustStore {
    path: PathBuf,
    hashes: BTreeSet<String>,
}

/// Resolve a quecto state file path: `$<env_var>` if set (and non-empty),
/// otherwise `$XDG_STATE_HOME/quecto/<leaf>` falling back to
/// `$HOME/.local/state/quecto/<leaf>`, and finally `.quecto-state/quecto/<leaf>`
/// if neither is set. Shared by `TrustStore::default_path` and
/// `Store::default_path`, which differ only in env var and leaf name.
pub(crate) fn state_path(env_var: &str, leaf: &str) -> PathBuf {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/state"))
        })
        .unwrap_or_else(|| PathBuf::from(".quecto-state"));
    base.join("quecto").join(leaf)
}

impl TrustStore {
    pub fn default_path() -> PathBuf {
        state_path("QUECTO_TRUST_FILE", "trust")
    }

    pub fn open() -> TrustStore {
        TrustStore::open_at(TrustStore::default_path())
    }

    pub fn open_at(path: PathBuf) -> TrustStore {
        let hashes = std::fs::read_to_string(&path)
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        TrustStore { path, hashes }
    }

    pub fn is_trusted(&self, hash: &str) -> bool {
        self.hashes.contains(hash)
    }

    pub fn trust(&mut self, hash: &str) {
        if !self.hashes.insert(hash.to_string()) {
            return;
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let body: String = self.hashes.iter().cloned().collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(&self.path, format!("{body}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust");
        let mut store = TrustStore::open_at(path.clone());
        assert!(!store.is_trusted("abc"));
        store.trust("abc");
        assert!(store.is_trusted("abc"));
        // Reopen: the hash is still there.
        let reopened = TrustStore::open_at(path);
        assert!(reopened.is_trusted("abc"));
        assert!(!reopened.is_trusted("def"));
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::open_at(dir.path().join("nope"));
        assert!(!store.is_trusted("x"));
    }
}
