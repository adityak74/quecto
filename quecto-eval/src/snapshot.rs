use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub fn snapshot_hash(workspace: &Path) -> anyhow::Result<String> {
    let mut paths = walk_files(workspace)?;
    paths.sort();
    let mut hasher = Sha256::new();
    for p in &paths {
        let rel = p.strip_prefix(workspace)?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(fs::read(p)?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn walk_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".git").unwrap_or(false) {
            continue;
        }
        if path.is_dir() {
            out.extend(walk_files(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

pub fn snapshot_copy(workspace: &Path, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    copy_dir_recursive(workspace, dest)
}

pub fn restore(dest: &Path, workspace: &Path) -> anyhow::Result<()> {
    if workspace.exists() {
        fs::remove_dir_all(workspace)?;
    }
    copy_dir_recursive(dest, workspace)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".git").unwrap_or(false) {
            continue;
        }
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn restore_reproduces_identical_hash() {
        let workspace = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("a.txt"), "hello").unwrap();
        let original_hash = snapshot_hash(workspace.path()).unwrap();

        snapshot_copy(workspace.path(), backup.path().join("snap").as_path()).unwrap();
        fs::write(workspace.path().join("a.txt"), "mutated").unwrap();
        assert_ne!(snapshot_hash(workspace.path()).unwrap(), original_hash);

        restore(&backup.path().join("snap"), workspace.path()).unwrap();
        assert_eq!(snapshot_hash(workspace.path()).unwrap(), original_hash);
    }
}
