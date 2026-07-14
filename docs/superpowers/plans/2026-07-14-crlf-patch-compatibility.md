# CRLF Patch Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix compatibility issues in `apply_patch` when targeting files with CRLF line endings.

**Architecture:** Detect if the target file has CRLF line endings, and if so, normalize the search and replace block line endings to CRLF before patching.

**Tech Stack:** Rust

## Global Constraints

- Preserve all existing comments and docstrings.
- Follow TDD.
- Ensure all tests pass.

---

### Task 1: CRLF line endings compatibility in apply_patch

**Files:**
- Modify: `quecto-agent/src/tools/patch.rs`

- [ ] **Step 1: Write a test verifying CRLF compatibility**
  Add a unit test `apply_patch_crlf_compatibility` in `quecto-agent/src/tools/patch.rs` that writes a file containing CRLF line endings (`\r\n`), runs `apply_patch` with LF line endings in the SEARCH/REPLACE blocks, and asserts that the patch applies successfully and preserves CRLF endings in the written file.

- [ ] **Step 2: Update apply_block line ending normalization**
  In `quecto-agent/src/tools/patch.rs`, update `apply_block` to detect if the existing file uses CRLF endings, and if so, map `\n` to `\r\n` in search and replace strings:
  ```rust
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
  ```

- [ ] **Step 3: Run tests and verify**
  Run `cargo test` to ensure all tests (including the new CRLF test) pass.
  Expected: PASS

- [ ] **Step 4: Commit changes**
  ```bash
  git add quecto-agent/src/tools/patch.rs
  git commit -m "fix(patch): support applying patches to CRLF line ending files"
  ```
