use std::path::{Path, PathBuf};

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".agent/instructions.md"];

/// Collect instruction files walking from `repo_root` down to `cwd`. Root-level
/// files come first and nearer (deeper) files come last, so when read
/// top-to-bottom the nearest instructions take precedence. Empty files are
/// skipped. Returns `None` when nothing is found.
pub fn load(repo_root: &Path, cwd: &Path) -> Option<String> {
    let mut sections = Vec::new();
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    for dir in dir_chain(&root, cwd) {
        for name in INSTRUCTION_FILES {
            let path = dir.join(name);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if text.trim().is_empty() {
                continue;
            }
            let label = path.strip_prefix(&root).unwrap_or(&path);
            sections.push(format!("## {}\n{}", label.display(), text.trim_end()));
        }
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Directories from `root` to `cwd` inclusive, root first. If `cwd` is not a
/// descendant of `root`, only `root` is returned.
fn dir_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let here = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let rel = match here.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => return vec![root.to_path_buf()],
    };
    let mut chain = vec![root.to_path_buf()];
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur = cur.join(comp);
        chain.push(cur.clone());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn none_when_no_instruction_files() {
        let dir = tempdir().unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }

    #[test]
    fn loads_root_agents_md() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
        let out = load(dir.path(), dir.path()).unwrap();
        assert!(out.contains("## AGENTS.md"));
        assert!(out.contains("root rules"));
    }

    #[test]
    fn nearer_file_appears_after_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
        let sub = dir.path().join("crate");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("AGENTS.md"), "crate rules").unwrap();
        let out = load(dir.path(), &sub).unwrap();
        let root_at = out.find("root rules").unwrap();
        let crate_at = out.find("crate rules").unwrap();
        assert!(root_at < crate_at, "nearer file must come last");
    }

    #[test]
    fn empty_files_are_skipped() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "   \n").unwrap();
        assert!(load(dir.path(), dir.path()).is_none());
    }
}
