# quecto-agent M3 — Editing (Patch Engine + write_file + Change Tracking) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `quecto-agent` edit files: add `write_file` (create/overwrite) and `apply_patch` (the search/replace-block engine), scoped by a create-safe path resolver, with every mutation recorded in-session for later undo/verification.

**Architecture:** Build on M2. `Context` gains `resolve_for_create` (canonicalizes the **parent** dir and checks containment, so a not-yet-existing file can't escape via a symlinked parent) and an in-memory change log (`Vec<FileChange>` with `before`/`after`). `write_file` writes a whole file. The patch engine (`tools/patch.rs`) parses search/replace blocks (`------ path`, `<<<<<<< SEARCH`/`=======`/`>>>>>>> REPLACE`), locates each `SEARCH` **exactly** (rejecting not-found and ambiguous multi-match), replaces the first occurrence preserving surrounding text, records the change, and reports a per-block result so the model can retry failures. Both tools join the built-in registry. **Approval/sandbox (M4), verify gate (M5), and SQLite persistence (M6) are still deferred** — changes are tracked in memory only.

**Tech Stack:** Rust (edition 2021). **No new dependencies** — pure `std` + `serde_json`. No async.

## Global Constraints

- Rust edition = **2021**. No async. **No new crates** — M3 adds zero dependencies.
- Error handling: tool failures use `ToolError` (reported to the model); `BoxErr` only on core paths.
- **Path safety is mandatory.** Existing files use `Context::resolve_existing` (M2); new/overwritten files use the new `Context::resolve_for_create`, which canonicalizes the parent directory and rejects any path whose parent escapes the repo root. No writes outside the repo root, ever.
- **Patch engine is exact-match, never fuzzy.** `SEARCH` must match verbatim (whitespace-sensitive). Not found → structured error to retry. More than one match → ambiguous error. Never guess.
- **Blocks are independent:** multiple blocks apply in order; a later block failing does NOT roll back earlier successes; each block's outcome is reported separately. An **empty `SEARCH` creates/overwrites** the whole file.
- Every successful write records a `FileChange { path, before, after }` in `Context` (in-memory; SQLite persistence is M6).
- **Only `write_file` and `apply_patch`** are added in M3. Do NOT build `run_command`, the sandbox, approval policy, the verify gate, session persistence, flavors, or the rich renderer — later milestones.
- Line-ending note (accepted limitation): the engine matches against the file's raw bytes-as-UTF-8 text. `SEARCH` reconstructed with `\n` will not match a CRLF file and returns a clean not-found error (retry). Full CRLF handling is deferred.
- **No approval gate yet (M4):** in M3 the editing tools run whenever the model calls them, scoped only by path safety. This is an accepted milestone state; gating arrives in M4.

---

## File Structure

- `quecto-agent/src/tools/mod.rs` — `Context` gains `resolve_for_create` + a `changes` log + `record_change`/`changes`; new `FileChange` struct; `builtin_tools()` gains the two editors (Task 5). (Tasks 1 & 5.)
- `quecto-agent/src/tools/fs.rs` — add `WriteFile`. (Task 2.)
- `quecto-agent/src/tools/patch.rs` — parser + engine (`parse_patch`, `apply_to_text`, `line_delta`) and the `ApplyPatch` tool. (Tasks 3–4.)
- `quecto-agent/src/lib.rs` — re-export `WriteFile`, `ApplyPatch`, `FileChange`. (Task 5.)

---

### Task 1: `Context::resolve_for_create` + change tracking

**Files:**
- Modify: `quecto-agent/src/tools/mod.rs`

**Interfaces:**
- Consumes: M2 `Context`, `ToolError`.
- Produces:
  - `pub struct FileChange { pub path: String, pub before: Option<String>, pub after: String }`
  - `Context::resolve_for_create(&self, rel: &str) -> Result<PathBuf, ToolError>` — canonical parent must exist and be inside the repo root; returns the intended absolute path (may not yet exist).
  - `Context::record_change(&mut self, path: impl Into<String>, before: Option<String>, after: String)`
  - `Context::changes(&self) -> &[FileChange]`

- [ ] **Step 1: Write the failing tests** — add to the `mod tests` in `quecto-agent/src/tools/mod.rs`

```rust
    #[test]
    fn resolve_for_create_allows_new_file_in_repo() {
        let dir = tempfile::tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf());
        let p = cx.resolve_for_create("new.txt").unwrap();
        assert!(p.starts_with(&cx.repo_root));
        assert!(p.ends_with("new.txt"));
    }

    #[test]
    fn resolve_for_create_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf());
        assert!(cx.resolve_for_create("../evil.txt").is_err());
    }

    #[test]
    fn record_change_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        cx.record_change("a.txt", None, "hi".to_string());
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].path, "a.txt");
        assert_eq!(cx.changes()[0].before, None);
        assert_eq!(cx.changes()[0].after, "hi");
    }
```

Note: this task adds the first use of `tempfile` inside `tools/mod.rs` tests — `tempfile` is already a dev-dependency (added in M2), so no manifest change is needed.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib tools::tests`
Expected: FAIL — `resolve_for_create` / `record_change` / `changes` not found.

- [ ] **Step 3: Add `FileChange` and extend `Context`** — in `quecto-agent/src/tools/mod.rs`

Add the struct (near `Context`):
```rust
/// A recorded file mutation — enables in-session change summaries and (M6) undo.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub before: Option<String>,
    pub after: String,
}
```

Replace the `Context` struct + `impl Context` block with:
```rust
pub struct Context {
    pub repo_root: PathBuf,
    changes: Vec<FileChange>,
}

impl Context {
    pub fn new(repo_root: PathBuf) -> Self {
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        Context { repo_root, changes: Vec::new() }
    }

    /// Resolve a repo-relative path that must already exist, rejecting escapes.
    pub fn resolve_existing(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let canon = self
            .repo_root
            .join(rel)
            .canonicalize()
            .map_err(|e| ToolError::new(format!("{rel}: {e}")))?;
        if !canon.starts_with(&self.repo_root) {
            return Err(ToolError::new(format!("path '{rel}' escapes the repository root")));
        }
        Ok(canon)
    }

    /// Resolve a repo-relative path for creation/overwrite. The file need not exist,
    /// but its parent directory must exist and canonicalize to inside the repo root
    /// (so a symlinked parent cannot smuggle a write outside the tree).
    pub fn resolve_for_create(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let joined = self.repo_root.join(rel);
        let parent = joined
            .parent()
            .ok_or_else(|| ToolError::new(format!("invalid path '{rel}'")))?;
        let parent_canon = parent
            .canonicalize()
            .map_err(|e| ToolError::new(format!("{rel}: parent {e}")))?;
        if !parent_canon.starts_with(&self.repo_root) {
            return Err(ToolError::new(format!("path '{rel}' escapes the repository root")));
        }
        let file_name = joined
            .file_name()
            .ok_or_else(|| ToolError::new(format!("invalid path '{rel}'")))?;
        Ok(parent_canon.join(file_name))
    }

    /// Record a file mutation (before/after contents; `before` is None for a new file).
    pub fn record_change(&mut self, path: impl Into<String>, before: Option<String>, after: String) {
        self.changes.push(FileChange { path: path.into(), before, after });
    }

    /// All file changes recorded this session, in order.
    pub fn changes(&self) -> &[FileChange] {
        &self.changes
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib`
Expected: PASS (new Context tests + all prior). Warning-free.

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/tools/mod.rs
git commit -m "feat(agent): add create-safe path resolver and in-session change tracking"
```

---

### Task 2: `write_file` tool

**Files:**
- Modify: `quecto-agent/src/tools/fs.rs`

**Interfaces:**
- Consumes: `Tool`, `ToolOutput`, `ToolError`, `Context` (`resolve_for_create`, `record_change`) (Task 1).
- Produces: `pub struct WriteFile;` (`impl Tool`).

- [ ] **Step 1: Write the failing tests** — add to the `mod tests` in `quecto-agent/src/tools/fs.rs`

```rust
    #[test]
    fn write_file_creates_and_records() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = WriteFile.run(&json!({"path":"new.txt","content":"hello\n"}), &mut cx).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("new.txt")).unwrap(), "hello\n");
        assert!(out.content.contains("created"));
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].before, None);
    }

    #[test]
    fn write_file_overwrites_and_records_before() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "old").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = WriteFile.run(&json!({"path":"a.txt","content":"new"}), &mut cx).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new");
        assert!(out.content.contains("overwrote"));
        assert_eq!(cx.changes()[0].before, Some("old".to_string()));
    }

    #[test]
    fn write_file_rejects_escape() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        assert!(WriteFile.run(&json!({"path":"../evil.txt","content":"x"}), &mut cx).is_err());
    }
```

(The `use` header of `tools/fs.rs` already imports `Context`, `Tool`, `ToolError`, `ToolOutput`, `ToolResult`, `cap_output`, `json`, `Value` from M2; the test module already imports `std::fs` and `tempfile::tempdir`. Add no new imports.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib fs`
Expected: FAIL — `WriteFile` not found.

- [ ] **Step 3: Implement `WriteFile`** — add above the `#[cfg(test)]` module in `quecto-agent/src/tools/fs.rs`

```rust
/// Create or overwrite a whole file. Records the prior contents (for undo/summary).
pub struct WriteFile;
impl Tool for WriteFile {
    fn name(&self) -> &str { "write_file" }
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
            "required": ["path","content"]
        })
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("write_file: 'path' is required"))?;
        let content = args.get("content").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("write_file: 'content' is required"))?;
        let abs = cx.resolve_for_create(path)?;
        let before = std::fs::read_to_string(&abs).ok();
        std::fs::write(&abs, content).map_err(|e| ToolError::new(format!("{path}: {e}")))?;
        cx.record_change(path, before.clone(), content.to_string());
        let n = content.lines().count();
        let verb = if before.is_some() { "overwrote" } else { "created" };
        Ok(ToolOutput::new(format!("{verb} {path} ({n} lines)"), format!("{verb} {n} lines")))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib fs`
Expected: PASS (3 new + M2 fs tests). Then `cargo test -p quecto-agent` all green.

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/tools/fs.rs
git commit -m "feat(agent): add write_file tool with change recording"
```

---

### Task 3: Patch parser + engine (pure functions)

**Files:**
- Create: `quecto-agent/src/tools/patch.rs`
- Modify: `quecto-agent/src/tools/mod.rs` (declare `pub mod patch;`)

**Interfaces:**
- Consumes: nothing beyond `std`.
- Produces (all `pub(crate)` — internal to the tool):
  - `struct PatchBlock { path: String, search: String, replace: String }`
  - `fn parse_patch(text: &str) -> Vec<PatchBlock>`
  - `enum ApplyErr { NotFound, Ambiguous(usize) }`
  - `fn apply_to_text(content: &str, search: &str, replace: &str) -> Result<String, ApplyErr>`
  - `fn line_delta(before: &str, after: &str) -> (usize, usize)`

- [ ] **Step 1: Write the failing tests** — create `quecto-agent/src/tools/patch.rs` with this test module (implementation added in Step 3)

```rust
use crate::tools::{cap_output, Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;

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
        let two = format!("{PATCH}\n------ src/b.rs\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE");
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
        assert!(matches!(apply_to_text("nope", "zzz", "q"), Err(ApplyErr::NotFound)));
    }

    #[test]
    fn apply_to_text_ambiguous() {
        assert!(matches!(apply_to_text("x\nx\n", "x", "y"), Err(ApplyErr::Ambiguous(2))));
    }

    #[test]
    fn line_delta_counts_changes() {
        let (a, r) = line_delta("one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!((a, r), (1, 0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib patch`
Expected: FAIL — `parse_patch` / `apply_to_text` / `line_delta` / `ApplyErr` not found. (Do Step 4's module declaration first if the file isn't compiled.)

- [ ] **Step 3: Implement the parser + engine** — add above the `#[cfg(test)]` module in `quecto-agent/src/tools/patch.rs`

```rust
use std::collections::HashMap;

/// One search/replace block targeting a file.
pub(crate) struct PatchBlock {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Parse zero or more search/replace blocks. Each block is:
///   `------ <path>` / `<<<<<<< SEARCH` / …search… / `=======` / …replace… / `>>>>>>> REPLACE`
/// Lines outside a well-formed block are ignored; a truncated trailing block is dropped.
pub(crate) fn parse_patch(text: &str) -> Vec<PatchBlock> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    let mut pending = lines.next();
    while let Some(line) = pending {
        pending = lines.next();
        let path = match line.strip_prefix("------ ") {
            Some(p) => p.trim().to_string(),
            None => continue,
        };
        // expect the SEARCH marker next
        if pending.map(|l| l.trim_end()) != Some("<<<<<<< SEARCH") {
            continue;
        }
        pending = lines.next(); // consume SEARCH marker

        let mut search = Vec::new();
        let mut saw_divider = false;
        while let Some(l) = pending {
            pending = lines.next();
            if l.trim_end() == "=======" {
                saw_divider = true;
                break;
            }
            search.push(l);
        }
        if !saw_divider {
            break;
        }

        let mut replace = Vec::new();
        let mut saw_end = false;
        while let Some(l) = pending {
            pending = lines.next();
            if l.trim_end() == ">>>>>>> REPLACE" {
                saw_end = true;
                break;
            }
            replace.push(l);
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

/// Why an exact search/replace could not be applied.
pub(crate) enum ApplyErr {
    NotFound,
    Ambiguous(usize),
}

/// Replace the single exact occurrence of `search` with `replace`. Errors if the
/// search text is absent or occurs more than once — never a fuzzy/ambiguous apply.
pub(crate) fn apply_to_text(content: &str, search: &str, replace: &str) -> Result<String, ApplyErr> {
    match content.matches(search).count() {
        0 => Err(ApplyErr::NotFound),
        1 => Ok(content.replacen(search, replace, 1)),
        n => Err(ApplyErr::Ambiguous(n)),
    }
}

/// Approximate added/removed line counts (multiset difference) for a change summary.
pub(crate) fn line_delta(before: &str, after: &str) -> (usize, usize) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in before.lines() {
        *counts.entry(l).or_default() -= 1;
    }
    for l in after.lines() {
        *counts.entry(l).or_default() += 1;
    }
    let mut added = 0usize;
    let mut removed = 0usize;
    for c in counts.values() {
        if *c > 0 {
            added += *c as usize;
        } else if *c < 0 {
            removed += (-*c) as usize;
        }
    }
    (added, removed)
}
```

- [ ] **Step 4: Declare the module** — add to `quecto-agent/src/tools/mod.rs`

```rust
pub mod patch;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib patch`
Expected: PASS (6 tests). Warning: `cap_output`/`Context`/`Tool`/… imports in `patch.rs` are unused until Task 4 — to keep the build warning-free, add the `ApplyPatch` tool in Task 4 in the **same review cycle**; if committing Task 3 alone, temporarily narrow the `use` line to `use std::` items only and re-add the tool imports in Task 4. (Recommended: treat Tasks 3–4 as one commit — see Task 4 Step 6.)

- [ ] **Step 6: (Deferred commit)** Tasks 3 and 4 commit together at the end of Task 4 to avoid an intermediate unused-import warning. Proceed directly to Task 4.

---

### Task 4: `apply_patch` tool

**Files:**
- Modify: `quecto-agent/src/tools/patch.rs`

**Interfaces:**
- Consumes: `parse_patch`, `apply_to_text`, `line_delta`, `ApplyErr` (Task 3); `Context` (`resolve_existing`, `resolve_for_create`, `record_change`); `Tool`, `ToolOutput`, `ToolError`, `cap_output`.
- Produces: `pub struct ApplyPatch;` (`impl Tool`) + private `apply_block`.

- [ ] **Step 1: Write the failing tests** — add to the `mod tests` in `quecto-agent/src/tools/patch.rs`

```rust
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn apply_patch_edits_and_records() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "let x = 1;\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let out = ApplyPatch.run(&json!({"patch": PATCH}), &mut cx).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.rs")).unwrap(), "let x = 2;\n");
        assert!(out.content.contains("applied"));
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].before, Some("let x = 1;\n".to_string()));
    }

    #[test]
    fn apply_patch_reports_not_found_without_writing() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "let y = 9;\n").unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        // returns Ok, but the per-block result says not found; file is unchanged
        let out = ApplyPatch.run(&json!({"patch": PATCH}), &mut cx).unwrap();
        assert!(out.content.contains("not found"));
        assert_eq!(fs::read_to_string(dir.path().join("a.rs")).unwrap(), "let y = 9;\n");
        assert_eq!(cx.changes().len(), 0);
    }

    #[test]
    fn apply_patch_empty_search_creates_file() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        let create = "------ new.rs\n<<<<<<< SEARCH\n=======\nfn main() {}\n>>>>>>> REPLACE";
        let out = ApplyPatch.run(&json!({"patch": create}), &mut cx).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("new.rs")).unwrap(), "fn main() {}");
        assert!(out.content.contains("created"));
    }

    #[test]
    fn apply_patch_no_blocks_is_error() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf());
        assert!(ApplyPatch.run(&json!({"patch":"garbage with no blocks"}), &mut cx).is_err());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib patch`
Expected: FAIL — `ApplyPatch` not found.

- [ ] **Step 3: Implement `ApplyPatch` + `apply_block`** — add above the `#[cfg(test)]` module in `quecto-agent/src/tools/patch.rs`

```rust
/// Edit files with search/replace blocks. Exact-match only; unmatched or ambiguous
/// blocks are reported back (never applied fuzzily). Blocks apply independently.
pub struct ApplyPatch;
impl Tool for ApplyPatch {
    fn name(&self) -> &str { "apply_patch" }
    fn description(&self) -> &str {
        "Edit files using search/replace blocks. Format per block: a line '------ <path>', then \
'<<<<<<< SEARCH', the exact text to find, '=======', the replacement, '>>>>>>> REPLACE'. \
The SEARCH text must match exactly and uniquely. An empty SEARCH creates/overwrites the file."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "patch": {"type":"string","description":"one or more search/replace blocks"} },
            "required": ["patch"]
        })
    }
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let patch = args.get("patch").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("apply_patch: 'patch' is required"))?;
        let blocks = parse_patch(patch);
        if blocks.is_empty() {
            return Err(ToolError::new(
                "no valid search/replace blocks found. Each block is: '------ <path>', \
'<<<<<<< SEARCH', text, '=======', text, '>>>>>>> REPLACE'.",
            ));
        }
        let mut applied = 0usize;
        let mut lines = Vec::new();
        for b in &blocks {
            let (ok, msg) = apply_block(cx, b);
            if ok { applied += 1; }
            lines.push(msg);
        }
        let summary = format!("{}/{} blocks applied", applied, blocks.len());
        Ok(ToolOutput::new(cap_output(&lines.join("\n"), 16_000), summary))
    }
}

/// Apply one block; returns (succeeded, human-readable result line for the model).
fn apply_block(cx: &mut Context, b: &PatchBlock) -> (bool, String) {
    // Empty SEARCH → create/overwrite the whole file with the replacement.
    if b.search.is_empty() {
        let abs = match cx.resolve_for_create(&b.path) {
            Ok(p) => p,
            Err(e) => return (false, format!("{}: {}", b.path, e.message)),
        };
        let before = std::fs::read_to_string(&abs).ok();
        if let Err(e) = std::fs::write(&abs, &b.replace) {
            return (false, format!("{}: write failed: {e}", b.path));
        }
        let verb = if before.is_some() { "overwrote" } else { "created" };
        let n = b.replace.lines().count();
        cx.record_change(b.path.clone(), before, b.replace.clone());
        return (true, format!("{}: {} ({} lines)", b.path, verb, n));
    }

    let abs = match cx.resolve_existing(&b.path) {
        Ok(p) => p,
        Err(e) => return (false, format!("{}: {}", b.path, e.message)),
    };
    let content = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(e) => return (false, format!("{}: {e}", b.path)),
    };
    match apply_to_text(&content, &b.search, &b.replace) {
        Err(ApplyErr::NotFound) => (
            false,
            format!("{}: SEARCH not found — re-read the file and retry with exact text", b.path),
        ),
        Err(ApplyErr::Ambiguous(n)) => (
            false,
            format!("{}: SEARCH matches {n} places — include more surrounding context", b.path),
        ),
        Ok(new) => {
            if let Err(e) = std::fs::write(&abs, &new) {
                return (false, format!("{}: write failed: {e}", b.path));
            }
            let (added, removed) = line_delta(&content, &new);
            cx.record_change(b.path.clone(), Some(content), new);
            (true, format!("{}: applied (+{added} -{removed})", b.path))
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib patch`
Expected: PASS (Task 3 pure-fn tests + 4 tool tests). Warning-free (all imports now used).

- [ ] **Step 5: Full check**

Run: `cargo test -p quecto-agent && cargo clippy -p quecto-agent --all-targets`
Expected: all pass, no clippy warnings.

- [ ] **Step 6: Commit (Tasks 3 + 4 together)**

```bash
git add quecto-agent/src/tools/patch.rs quecto-agent/src/tools/mod.rs
git commit -m "feat(agent): add apply_patch search/replace engine (exact-match, ambiguity-rejecting)"
```

---

### Task 5: Register the editors + loop integration test

**Files:**
- Modify: `quecto-agent/src/tools/mod.rs` (`builtin_tools`)
- Modify: `quecto-agent/src/lib.rs` (re-exports)
- Modify: `quecto-agent/src/agent.rs` (add a loop integration test)

**Interfaces:**
- Consumes: `WriteFile` (Task 2), `ApplyPatch` (Task 4).
- Produces: `builtin_tools()` now returns all seven tools; `WriteFile`/`ApplyPatch`/`FileChange` re-exported.

- [ ] **Step 1: Add the editors to `builtin_tools`** — update the function in `quecto-agent/src/tools/mod.rs`

```rust
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFile),
        Box::new(fs::ListFiles),
        Box::new(fs::WriteFile),
        Box::new(search::SearchText),
        Box::new(patch::ApplyPatch),
        Box::new(git::GitDiff),
        Box::new(git::GitStatus),
    ]
}
```

- [ ] **Step 2: Re-export the new items** — update `quecto-agent/src/lib.rs`

Change the fs re-export line and add patch/FileChange:
```rust
pub use tools::fs::{ListFiles, ReadFile, WriteFile};
pub use tools::patch::ApplyPatch;
pub use tools::{
    builtin_tools, cap_output, Context, FileChange, Registry, Tool, ToolError, ToolOutput, ToolResult,
};
```
(Keep the existing `pub use tools::git::{GitDiff, GitStatus};` and `pub use tools::search::SearchText;` lines.)

- [ ] **Step 3: Write the failing loop integration test** — add to the `mod tests` in `quecto-agent/src/agent.rs`

```rust
    #[test]
    fn agent_write_file_flows_through_the_loop() {
        use crate::tools::fs::WriteFile;
        let dir = tempfile::tempdir().unwrap();
        // turn 1: the model asks to write a file; turn 2: it answers.
        let call = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"hello.txt","content":"hi there\n"}),
            }],
            finish_reason: "tool_calls".into(),
        };
        let model = Scripted::new(vec![call, text("done")]);
        let mut a = Agent::new(Box::new(model), "sys", 10, dir.path().to_path_buf())
            .register(Box::new(WriteFile));
        match a.run("make the file") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hi there\n"
        );
    }
```

(`tempfile` is a dev-dependency; the `agent.rs` test module already imports `AssistantMessage`, `ToolCall`, `json`, `Agent`, `Outcome`, `Scripted`, `text`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent`
Expected: PASS — the new loop test writes the file end-to-end; all prior tests still green.

- [ ] **Step 5: Full verification**

Run: `cargo test -p quecto-agent && cargo clippy -p quecto-agent --all-targets`
Expected: all tests pass, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add quecto-agent/src/tools/mod.rs quecto-agent/src/lib.rs quecto-agent/src/agent.rs
git commit -m "feat(agent): register write_file and apply_patch as built-in tools"
```

---

## Self-Review

**Spec coverage** (against `2026-07-10-quecto-agent-architecture.md`, scoped to M3):

- `write_file` — create/overwrite whole file, records prior content (spec Built-in tools table): T2. ✅
- `apply_patch` — search/replace blocks; resolve+path-check; exact match; not-found → structured error; ambiguous → reject; replace first occurrence; record previous contents; multiple blocks apply in order, later failure doesn't roll back earlier, each reported independently; empty SEARCH creates a file (spec §Patch engine, rules 1–6): T3–T4. ✅
- Path safety for not-yet-existing files — canonicalize the parent, reject symlinked-parent escape (spec §Path safety): `Context::resolve_for_create` (T1). ✅
- Change recording enabling in-session undo (spec §Session `file_changes`, minus persistence): `Context` change log (T1); persistence deferred to M6. ✅
- Diff summary for the renderer (+N -M) (spec §Patch engine rule 6): `line_delta` (T3), shown in the per-block result and activity summary. ✅ (Approximate multiset delta; a precise Myers diff can replace it later without changing the interface.)
- Tools join the registry / model can call them (T5). ✅

**Deliberately deferred (later milestones):** `run_command` + sandbox + approval policy + denylist + interactivity + cancel + repeated-action guard (M4); verify gate + instruction loader + context seed (M5); SQLite session persistence + `undo`/`diff` CLI + chat/slash-commands + rich renderer (M6); flavors + `edit_format` config + `[tools]` allow-list + `text` protocol + `ask_user` (M7). None are in M3.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to Task N"; every code step has complete code. ✅

**Type consistency:** `Context::{resolve_existing,resolve_for_create,record_change,changes}` signatures match all call sites (write_file, apply_patch, tests); `FileChange { path:String, before:Option<String>, after:String }` consistent at record + assertions; `Tool::run(&self,&Value,&mut Context)->ToolResult` unchanged for the new tools; `parse_patch`/`apply_to_text`/`line_delta`/`ApplyErr` signatures match their uses in `apply_block` and tests; `builtin_tools() -> Vec<Box<dyn Tool>>` unchanged. ✅

**Scope decisions flagged:** (1) `line_delta` is an **approximate** multiset line delta (dependency-free) rather than a precise diff — fine for the cosmetic +N/-N summary; (2) editing tools run **without an approval gate** in M3 (path-safety only) — gating is M4; (3) the patch engine assumes **LF** line endings (CRLF → clean not-found error to retry).

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-10-quecto-agent-m3-editing.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Or hand it to Codex (as with M1/M2) — I'll verify.**
