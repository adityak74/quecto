use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file in the repository. Optional 1-based start_line/end_line select a range."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type":"string","description":"repo-relative file path"},
                "start_line": {"type":"integer"},
                "end_line": {"type":"integer"}
            },
            "required": ["path"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("read_file: 'path' is required"))?;
        let full = cx.resolve_existing(path)?;
        let text =
            std::fs::read_to_string(&full).map_err(|e| ToolError::new(format!("{path}: {e}")))?;

        let start = args.get("start_line").and_then(|v| v.as_u64());
        let end = args.get("end_line").and_then(|v| v.as_u64());
        let selected = if start.is_some() || end.is_some() {
            let lines: Vec<&str> = text.lines().collect();
            let s = start.unwrap_or(1).max(1) as usize;
            let e = (end.unwrap_or(lines.len() as u64) as usize).min(lines.len());
            lines.get(s.saturating_sub(1)..e).unwrap_or(&[]).join("\n")
        } else {
            text
        };
        let n = selected.lines().count();
        Ok(ToolOutput::new(
            cap_output(&selected, 64_000),
            format!("{path} ({n} lines)"),
        ))
    }
}

pub struct ListFiles;

impl Tool for ListFiles {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories under a repo-relative path (default the repo root). Respects .gitignore."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type":"string","description":"repo-relative directory (default '.')"} },
            "required": []
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = cx.resolve_existing(rel)?;
        let mut entries = Vec::new();
        for dent in ignore::WalkBuilder::new(&base)
            .require_git(false)
            .standard_filters(true)
            .max_depth(Some(2))
            .build()
        {
            let dent = match dent {
                Ok(d) => d,
                Err(_) => continue,
            };
            if dent.depth() == 0 {
                continue;
            }
            let shown = dent
                .path()
                .strip_prefix(&cx.repo_root)
                .unwrap_or(dent.path());
            entries.push(shown.display().to_string());
            if entries.len() >= 500 {
                break;
            }
        }
        entries.sort();
        let n = entries.len();
        Ok(ToolOutput::new(
            cap_output(&entries.join("\n"), 32_000),
            format!("{n} entries"),
        ))
    }
}

/// Create a new file or overwrite an existing one with complete contents.
pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create a new file or overwrite an existing one with the given content. For a targeted edit, prefer apply_patch."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type":"string","description":"repo-relative file path"},
                "content": {"type":"string","description":"the full new file contents"}
            },
            "required": ["path", "content"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("write_file: 'path' is required"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("write_file: 'content' is required"))?;
        let abs = cx.resolve_for_create(path)?;
        let before = std::fs::read_to_string(&abs).ok();
        std::fs::write(&abs, content).map_err(|e| ToolError::new(format!("{path}: {e}")))?;
        cx.record_change(path, before.clone(), content.to_string());
        let lines = content.lines().count();
        let verb = if before.is_some() {
            "overwrote"
        } else {
            "created"
        };
        Ok(ToolOutput::new(
            format!("{verb} {path} ({lines} lines)"),
            format!("{verb} {lines} lines"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn read_file_returns_contents() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = ReadFile.run(&json!({"path":"a.txt"}), &mut cx).unwrap();
        assert_eq!(out.content, "hello\nworld\n");
    }

    #[test]
    fn read_file_honors_line_range() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = ReadFile
            .run(
                &json!({"path":"a.txt","start_line":2,"end_line":3}),
                &mut cx,
            )
            .unwrap();
        assert_eq!(out.content, "two\nthree");
    }

    #[test]
    fn read_file_missing_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(ReadFile.run(&json!({"path":"nope.txt"}), &mut cx).is_err());
    }

    #[test]
    fn list_files_lists_entries_gitignore_aware() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();
        fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = ListFiles.run(&json!({}), &mut cx).unwrap();
        assert!(out.content.contains("kept.txt"));
        assert!(!out.content.contains("ignored.txt"));
    }

    #[test]
    fn write_file_creates_and_records() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = WriteFile
            .run(&json!({"path":"new.txt","content":"hello\n"}), &mut cx)
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "hello\n"
        );
        assert!(out.content.contains("created"));
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].before, None);
    }

    #[test]
    fn write_file_overwrites_and_records_before() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "old").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = WriteFile
            .run(&json!({"path":"a.txt","content":"new"}), &mut cx)
            .unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new");
        assert!(out.content.contains("overwrote"));
        assert_eq!(cx.changes()[0].before, Some("old".to_string()));
    }

    #[test]
    fn write_file_rejects_escape() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(WriteFile
            .run(&json!({"path":"../evil.txt","content":"x"}), &mut cx)
            .is_err());
    }
}
