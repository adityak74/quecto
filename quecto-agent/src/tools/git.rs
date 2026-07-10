use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};
use std::path::Path;

fn run_git(repo: &Path, args: &[&str]) -> Result<String, ToolError> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| ToolError::new(format!("git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("not a git repository") {
            return Err(ToolError::new("not a git repository"));
        }
        return Err(ToolError::new(format!("git failed: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub struct GitDiff;

impl Tool for GitDiff {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show the working-tree git diff."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{},"required":[]})
    }

    fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
        let diff = run_git(&cx.repo_root, &["diff"])?;
        let content = if diff.trim().is_empty() {
            "no changes".to_string()
        } else {
            diff
        };
        let summary = format!("{} lines", content.lines().count());
        Ok(ToolOutput::new(cap_output(&content, 64_000), summary))
    }
}

pub struct GitStatus;

impl Tool for GitStatus {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the working-tree git status (porcelain)."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{},"required":[]})
    }

    fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
        let status = run_git(&cx.repo_root, &["status", "--porcelain"])?;
        let n = status.lines().filter(|l| !l.trim().is_empty()).count();
        let content = if status.trim().is_empty() {
            "clean".to_string()
        } else {
            status
        };
        Ok(ToolOutput::new(
            cap_output(&content, 32_000),
            format!("{n} changed"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn git_status_reports_not_a_repo() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let res = GitStatus.run(&json!({}), &mut cx);
        assert!(res.is_err());
        assert!(res.err().unwrap().message.contains("not a git repository"));
    }
}
