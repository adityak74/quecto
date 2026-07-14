use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;

/// One search/replace block targeting a file.
pub(crate) struct PatchBlock {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Parse zero or more search/replace blocks from patch text.
pub(crate) fn parse_patch(text: &str) -> Vec<PatchBlock> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    let mut pending = lines.next();

    while let Some(line) = pending {
        pending = lines.next();
        let path = match line.strip_prefix("------ ") {
            Some(path) => path.trim().to_string(),
            None => continue,
        };

        if pending.map(|next| next.trim_end()) != Some("<<<<<<< SEARCH") {
            continue;
        }
        pending = lines.next();

        let mut search = Vec::new();
        let mut saw_divider = false;
        while let Some(current) = pending {
            pending = lines.next();
            if current.trim_end() == "=======" {
                saw_divider = true;
                break;
            }
            search.push(current);
        }
        if !saw_divider {
            break;
        }

        let mut replace = Vec::new();
        let mut saw_end = false;
        while let Some(current) = pending {
            pending = lines.next();
            if current.trim_end() == ">>>>>>> REPLACE" {
                saw_end = true;
                break;
            }
            replace.push(current);
        }
        if !saw_end {
            break;
        }

        blocks.push(PatchBlock {
            path,
            search: search.join("\n"),
            replace: replace.join("\n"),
        });
    }

    blocks
}

/// Reasons an exact search/replace apply can fail.
#[derive(Debug)]
pub(crate) enum ApplyErr {
    NotFound,
    Ambiguous(usize),
}

/// Replace the unique exact occurrence of `search` with `replace`.
pub(crate) fn apply_to_text(
    content: &str,
    search: &str,
    replace: &str,
) -> Result<String, ApplyErr> {
    match content.matches(search).count() {
        0 => Err(ApplyErr::NotFound),
        1 => Ok(content.replacen(search, replace, 1)),
        count => Err(ApplyErr::Ambiguous(count)),
    }
}

/// Count approximate added/removed lines for summaries.
pub(crate) fn line_delta(before: &str, after: &str) -> (usize, usize) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for line in before.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    for line in after.lines() {
        *counts.entry(line).or_default() += 1;
    }

    let mut added = 0usize;
    let mut removed = 0usize;
    for count in counts.values() {
        if *count > 0 {
            added += *count as usize;
        } else if *count < 0 {
            removed += (-*count) as usize;
        }
    }
    (added, removed)
}

/// Edit files using exact search/replace blocks applied in order.
pub struct ApplyPatch;

impl Tool for ApplyPatch {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Edit files using search/replace blocks. Format per block: a line '------ <path>', then '<<<<<<< SEARCH', the exact text to find, '=======', the replacement, '>>>>>>> REPLACE'. The SEARCH text must match exactly and uniquely. An empty SEARCH creates or overwrites the file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {"type":"string","description":"one or more search/replace blocks"}
            },
            "required": ["patch"]
        })
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let patch = args
            .get("patch")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::new("apply_patch: 'patch' is required"))?;
        let blocks = parse_patch(patch);
        if blocks.is_empty() {
            return Err(ToolError::new(
                "no valid search/replace blocks found. Each block is: '------ <path>', '<<<<<<< SEARCH', text, '=======', text, '>>>>>>> REPLACE'.",
            ));
        }

        let mut applied = 0usize;
        let mut lines = Vec::new();
        for block in &blocks {
            let (ok, message) = apply_block(cx, block);
            if ok {
                applied += 1;
            }
            lines.push(message);
        }

        Ok(ToolOutput::new(
            cap_output(&lines.join("\n"), 16_000),
            format!("{applied}/{} blocks applied", blocks.len()),
        ))
    }
}

fn apply_block(cx: &mut Context, block: &PatchBlock) -> (bool, String) {
    if block.search.is_empty() {
        let abs = match cx.resolve_for_create(&block.path) {
            Ok(path) => path,
            Err(err) => return (false, format!("{}: {}", block.path, err.message)),
        };
        let before = std::fs::read_to_string(&abs).ok();
        let has_crlf = before.as_ref().map(|s| s.contains("\r\n")).unwrap_or(false);
        let replace = if has_crlf {
            block.replace.replace('\n', "\r\n")
        } else {
            block.replace.clone()
        };
        if let Err(err) = std::fs::write(&abs, &replace) {
            return (false, format!("{}: write failed: {err}", block.path));
        }
        let verb = if before.is_some() {
            "overwrote"
        } else {
            "created"
        };
        let line_count = replace.lines().count();
        cx.record_change(block.path.clone(), before, replace);
        return (true, format!("{}: {verb} ({line_count} lines)", block.path));
    }

    let abs = match cx.resolve_existing(&block.path) {
        Ok(path) => path,
        Err(err) => return (false, format!("{}: {}", block.path, err.message)),
    };
    let content = match std::fs::read_to_string(&abs) {
        Ok(content) => content,
        Err(err) => return (false, format!("{}: {err}", block.path)),
    };

    let has_crlf = content.contains("\r\n");
    let (search, replace) = if has_crlf {
        (block.search.replace('\n', "\r\n"), block.replace.replace('\n', "\r\n"))
    } else {
        (block.search.clone(), block.replace.clone())
    };

    match apply_to_text(&content, &search, &replace) {
        Err(ApplyErr::NotFound) => (
            false,
            format!(
                "{}: SEARCH not found — re-read the file and retry with exact text",
                block.path
            ),
        ),
        Err(ApplyErr::Ambiguous(matches)) => (
            false,
            format!(
                "{}: SEARCH matches {matches} places — include more surrounding context",
                block.path
            ),
        ),
        Ok(new_content) => {
            if let Err(err) = std::fs::write(&abs, &new_content) {
                return (false, format!("{}: write failed: {err}", block.path));
            }
            let (added, removed) = line_delta(&content, &new_content);
            cx.record_change(block.path.clone(), Some(content), new_content);
            (
                true,
                format!("{}: applied (+{added} -{removed})", block.path),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use std::fs;
    use tempfile::tempdir;

    const PATCH: &str = "\
------ src/a.rs
<<<<<<< SEARCH
let x = 1;
=======
let x = 2;
>>>>>>> REPLACE";

    #[test]
    fn parse_patch_reads_one_block() {
        let blocks = parse_patch(PATCH);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "src/a.rs");
        assert_eq!(blocks[0].search, "let x = 1;");
        assert_eq!(blocks[0].replace, "let x = 2;");
    }

    #[test]
    fn parse_patch_reads_multiple_blocks() {
        let two =
            format!("{PATCH}\n------ src/b.rs\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE");
        let blocks = parse_patch(&two);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].path, "src/b.rs");
    }

    #[test]
    fn apply_to_text_replaces_single_match() {
        let out = apply_to_text("let x = 1;\n", "let x = 1;", "let x = 2;").unwrap();
        assert_eq!(out, "let x = 2;\n");
    }

    #[test]
    fn apply_to_text_not_found() {
        assert!(matches!(
            apply_to_text("nope", "zzz", "q"),
            Err(ApplyErr::NotFound)
        ));
    }

    #[test]
    fn apply_to_text_ambiguous() {
        assert!(matches!(
            apply_to_text("x\nx\n", "x", "y"),
            Err(ApplyErr::Ambiguous(2))
        ));
    }

    #[test]
    fn line_delta_counts_changes() {
        let (a, r) = line_delta("one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!((a, r), (1, 0));
    }

    #[test]
    fn apply_patch_edits_and_records() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "let x = 1;\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = ApplyPatch.run(&json!({"patch": PATCH}), &mut cx).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("src/a.rs")).unwrap(),
            "let x = 2;\n"
        );
        assert!(out.content.contains("applied"));
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].before, Some("let x = 1;\n".to_string()));
    }

    #[test]
    fn apply_patch_reports_not_found_without_writing() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "let y = 9;\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = ApplyPatch.run(&json!({"patch": PATCH}), &mut cx).unwrap();
        assert!(out.content.contains("not found"));
        assert_eq!(
            fs::read_to_string(dir.path().join("src/a.rs")).unwrap(),
            "let y = 9;\n"
        );
        assert_eq!(cx.changes().len(), 0);
    }

    #[test]
    fn apply_patch_empty_search_creates_file() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let create = "------ new.rs\n<<<<<<< SEARCH\n=======\nfn main() {}\n>>>>>>> REPLACE";
        let out = ApplyPatch.run(&json!({"patch": create}), &mut cx).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("new.rs")).unwrap(),
            "fn main() {}"
        );
        assert!(out.content.contains("created"));
    }

    #[test]
    fn apply_patch_no_blocks_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(ApplyPatch
            .run(&json!({"patch":"garbage with no blocks"}), &mut cx)
            .is_err());
    }

    #[test]
    fn apply_patch_crlf_compatibility() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        // File with CRLF line endings
        fs::write(dir.path().join("src/a.rs"), "let x = 1;\r\nlet y = 2;\r\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());

        // Patch with LF line endings in SEARCH/REPLACE
        let crlf_patch = "\
------ src/a.rs
<<<<<<< SEARCH
let x = 1;
let y = 2;
=======
let x = 3;
let y = 4;
>>>>>>> REPLACE";

        let out = ApplyPatch.run(&json!({"patch": crlf_patch}), &mut cx).unwrap();
        assert!(out.content.contains("applied"));

        let updated_content = fs::read_to_string(dir.path().join("src/a.rs")).unwrap();
        // Assert that the patch is applied and uses CRLF endings
        assert_eq!(updated_content, "let x = 3;\r\nlet y = 4;\r\n");
    }
}
