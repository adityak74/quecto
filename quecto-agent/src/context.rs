use crate::tools::cap_output;
use std::path::Path;

/// Build the one-time seed context injected into the first system message:
/// the repo root, a shallow `.gitignore`-aware file tree, and `git status` /
/// `git diff` when the directory is a git repository.
pub fn seed(repo_root: &Path) -> String {
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut out = format!("# Repository context\nroot: {}\n", root.display());
    out.push_str("\n## Files (depth 2, .gitignore-aware)\n");
    out.push_str(&cap_output(&file_tree(&root), 8_000));
    if let Some(status) = git(&root, &["status", "--porcelain"]) {
        let status = status.trim_end();
        let status = if status.is_empty() { "clean" } else { status };
        out.push_str("\n\n## git status\n");
        out.push_str(&cap_output(status, 4_000));
    }
    if let Some(diff) = git(&root, &["diff"]) {
        let diff = diff.trim_end();
        if !diff.is_empty() {
            out.push_str("\n\n## git diff\n");
            out.push_str(&cap_output(diff, 16_000));
        }
    }
    out
}

fn file_tree(root: &Path) -> String {
    let mut entries = Vec::new();
    for dent in ignore::WalkBuilder::new(root)
        .require_git(false)
        .standard_filters(true)
        .max_depth(Some(2))
        .build()
        .flatten()
    {
        if dent.depth() == 0 {
            continue;
        }
        let shown = dent.path().strip_prefix(root).unwrap_or(dent.path());
        entries.push(shown.display().to_string());
        if entries.len() >= 300 {
            break;
        }
    }
    entries.sort();
    entries.join("\n")
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn seed_lists_files_and_marks_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        let out = seed(dir.path());
        assert!(out.contains("# Repository context"));
        assert!(out.contains("main.rs"));
    }

    #[test]
    fn seed_respects_gitignore() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(dir.path().join("kept.rs"), "x").unwrap();
        fs::write(dir.path().join("secret.txt"), "x").unwrap();
        let out = seed(dir.path());
        assert!(out.contains("kept.rs"));
        assert!(!out.contains("secret.txt"));
    }

    #[test]
    fn seed_omits_git_sections_without_a_repo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "x").unwrap();
        let out = seed(dir.path());
        assert!(!out.contains("## git status"));
        assert!(!out.contains("## git diff"));
    }
}
