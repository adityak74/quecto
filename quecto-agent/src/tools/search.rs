use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

pub struct SearchText;

impl Tool for SearchText {
    fn name(&self) -> &str {
        "search_text"
    }

    fn description(&self) -> &str {
        "Search the repository for a regular expression. Returns matching lines as path:line: text. Respects .gitignore."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type":"string","description":"regular expression"},
                "path": {"type":"string","description":"repo-relative directory to search (default '.')"}
            },
            "required": ["pattern"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("search_text: 'pattern' is required"))?;
        let re =
            regex::Regex::new(pattern).map_err(|e| ToolError::new(format!("invalid regex: {e}")))?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = cx.resolve_existing(rel)?;

        let mut hits: Vec<String> = Vec::new();
        'walk: for dent in ignore::WalkBuilder::new(&base)
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
                if re.is_match(line) {
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
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn search_text_finds_matches_with_line_numbers() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "fn main() {}\nlet x = 1;\nfn helper() {}\n",
        )
        .unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = SearchText.run(&json!({"pattern":"fn "}), &mut cx).unwrap();
        assert!(out.content.contains("a.rs:1:"));
        assert!(out.content.contains("a.rs:3:"));
        assert!(!out.content.contains("a.rs:2:"));
    }

    #[test]
    fn search_text_reports_no_matches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "nothing here\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = SearchText.run(&json!({"pattern":"zzz"}), &mut cx).unwrap();
        assert!(out.content.contains("no matches"));
    }

    #[test]
    fn search_text_invalid_regex_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        assert!(SearchText.run(&json!({"pattern":"("}), &mut cx).is_err());
    }
}
