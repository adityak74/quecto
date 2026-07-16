use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;

pub struct TakeNote;

impl Tool for TakeNote {
    fn name(&self) -> &str {
        "take_note"
    }

    fn description(&self) -> &str {
        "Create or append to a markdown note in the knowledge base (.qkb/ directory)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type":"string","description":"title of the note (used for filename)"},
                "content": {"type":"string","description":"content to append to the note"}
            },
            "required": ["title", "content"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("take_note: 'title' is required"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("take_note: 'content' is required"))?;

        let qkb_dir = cx.repo_root.join(".qkb");
        if !qkb_dir.exists() {
            fs::create_dir_all(&qkb_dir)
                .map_err(|e| ToolError::new(format!("failed to create .qkb directory: {e}")))?;
        }

        let filename = format!("{}.md", title.replace(|c: char| !c.is_alphanumeric(), "-"));
        let file_path = qkb_dir.join(&filename);
        let rel_path = format!(".qkb/{}", filename);

        let before = fs::read_to_string(&file_path).ok();

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .map_err(|e| ToolError::new(format!("failed to open note {}: {}", filename, e)))?;

        writeln!(file, "{}", content)
            .map_err(|e| ToolError::new(format!("failed to write to note {}: {}", filename, e)))?;
        
        drop(file);
        let after = fs::read_to_string(&file_path).unwrap_or_default();
        cx.record_change(&rel_path, before, after);

        let msg = format!("appended to {}", rel_path);
        Ok(ToolOutput::new(msg.clone(), msg))
    }
}

pub struct SearchNotes;

impl Tool for SearchNotes {
    fn name(&self) -> &str {
        "search_notes"
    }

    fn description(&self) -> &str {
        "Search through knowledge base notes (.qkb/ directory)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type":"string","description":"text to search for in notes"}
            },
            "required": ["query"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("search_notes: 'query' is required"))?;
        
        let qkb_dir = cx.repo_root.join(".qkb");
        if !qkb_dir.exists() {
            return Ok(ToolOutput::new("no matches".to_string(), "0 matches"));
        }

        let mut hits: Vec<String> = Vec::new();
        'walk: for dent in ignore::WalkBuilder::new(&qkb_dir)
            .require_git(false)
            .standard_filters(true)
            .build()
        {
            let dent = match dent {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if !dent.path().extension().map_or(false, |ext| ext == "md") {
                continue;
            }
            let text = match std::fs::read_to_string(dent.path()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let shown = dent
                .path()
                .strip_prefix(&cx.repo_root)
                .unwrap_or(dent.path())
                .display()
                .to_string();
            
            for (i, line) in text.lines().enumerate() {
                if line.contains(query) {
                    hits.push(format!("{}:{}: {}", shown, i + 1, line.trim_end()));
                    if hits.len() >= 200 {
                        break 'walk;
                    }
                }
            }
        }
        
        let n = hits.len();
        let content = if hits.is_empty() {
            "no matches".to_string()
        } else {
            hits.join("\n")
        };
        Ok(ToolOutput::new(
            cap_output(&content, 32_000),
            format!("{n} matches"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use tempfile::tempdir;

    #[test]
    fn take_note_creates_and_appends() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        
        // Create note
        let out = TakeNote.run(
            &json!({"title": "Test Note", "content": "hello world"}),
            &mut cx,
        ).unwrap();
        assert!(out.content.contains("Test-Note.md"));
        
        let path = dir.path().join(".qkb").join("Test-Note.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world\n");
        
        // Append to note
        TakeNote.run(
            &json!({"title": "Test Note", "content": "second line"}),
            &mut cx,
        ).unwrap();
        
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world\nsecond line\n");
    }

    #[test]
    fn search_notes_finds_matches() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        
        TakeNote.run(
            &json!({"title": "Rust Tips", "content": "Use cargo clippy"}),
            &mut cx,
        ).unwrap();
        TakeNote.run(
            &json!({"title": "Cargo", "content": "cargo build --release"}),
            &mut cx,
        ).unwrap();
        
        let out = SearchNotes.run(&json!({"query": "cargo"}), &mut cx).unwrap();
        assert!(out.content.contains("Rust-Tips.md"));
        assert!(out.content.contains("Cargo.md"));
    }
}
