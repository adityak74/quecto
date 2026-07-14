# CLI, REPL and UX Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address several CLI, REPL, and UX issues in `quecto-agent`, including context token count, status session queries, clear history resets, help command aliases, empty model handling, resume logs, and version flags.

**Architecture:** Modify `main.rs`, `agent.rs`, `chat.rs`, and `session.rs` to implement helpers, reset internal counters correctly, and update command handlers/Clap structure.

**Tech Stack:** Rust, Clap, Rusqlite

## Global Constraints

- Preserve all existing comments and docstrings.
- Follow TDD: write tests before implementation or ensure tests verify the updated behavior.
- Ensure all tests pass.

---

### Task 1: Add Store::session_status and fix /status session querying

**Files:**
- Modify: `quecto-agent/src/session.rs`
- Modify: `quecto-agent/src/main.rs`

- [ ] **Step 1: Write a test for `session_status`**
  Add a test to `quecto-agent/src/session.rs` verifying that `session_status(id)` returns the correct status for a session.

- [ ] **Step 2: Implement `session_status` in `Store`**
  In `quecto-agent/src/session.rs`, implement `pub fn session_status(&self, id: &str) -> Result<Option<String>, BoxErr>`.

- [ ] **Step 3: Run cargo test to verify tests pass**
  Run: `cargo test --package quecto-agent --lib session::tests`
  Expected: PASS

- [ ] **Step 4: Update `/status` command handler in `main.rs`**
  In `quecto-agent/src/main.rs`, modify `ChatCommand::Status` to query `store.session_status(&session_id)` instead of `store.latest_session()`.

- [ ] **Step 5: Run tests and commit**
  Verify all CLI and unit tests pass, then commit.
  ```bash
  git add quecto-agent/src/session.rs quecto-agent/src/main.rs
  git commit -m "fix(cli): query specific session status for /status command"
  ```

---

### Task 2: Reset recorded_changes on clear_history

**Files:**
- Modify: `quecto-agent/src/tools/mod.rs`
- Modify: `quecto-agent/src/agent.rs`
- Modify: `quecto-agent/src/main.rs`

- [ ] **Step 1: Add `clear_changes` to `Context`**
  In `quecto-agent/src/tools/mod.rs`, implement `pub fn clear_changes(&mut self)` to empty the `changes` vector.

- [ ] **Step 2: Update `clear_history` in `Agent`**
  In `quecto-agent/src/agent.rs`, modify `clear_history(&mut self)` to reset `self.recorded_changes = 0` and call `self.cx.clear_changes()`.

- [ ] **Step 3: Update `/clear` REPL confirmation in `main.rs`**
  In `quecto-agent/src/main.rs`, modify `ChatCommand::Clear` handler to display the session ID: `out.notice(&format!("session {} conversation cleared", session_id));`.

- [ ] **Step 4: Add a unit test verifying `clear_history` resets changes**
  Add a test verifying that calling `clear_history` on `Agent` correctly resets the recorded changes and message length.

- [ ] **Step 5: Run tests and commit**
  Verify tests pass and commit.
  ```bash
  git add quecto-agent/src/tools/mod.rs quecto-agent/src/agent.rs quecto-agent/src/main.rs
  git commit -m "fix(repl): reset recorded changes and improve confirmation on /clear"
  ```

---

### Task 3: Context command character and message counts

**Files:**
- Modify: `quecto-agent/src/main.rs`

- [ ] **Step 1: Update `/context` handler in `main.rs`**
  Modify `ChatCommand::Context` in `quecto-agent/src/main.rs` to compute the message count (subtracting system message) and character count:
  ```rust
  let msg_n = agent.messages.len().saturating_sub(1);
  let char_count: usize = agent.messages.iter().map(|m| m.content.len()).sum();
  out.notice(&format!("session: {} ({} messages, ~{} chars)", session_id, msg_n, char_count));
  ```

- [ ] **Step 2: Run tests and commit**
  ```bash
  git add quecto-agent/src/main.rs
  git commit -m "fix(repl): show transcript message and character count in /context"
  ```

---

### Task 4: CLI and REPL polish: parser aliases, model name empty check, resume log, version flag

**Files:**
- Modify: `quecto-agent/src/main.rs`

- [ ] **Step 1: Add Clap version to Cli**
  Add `#[command(version)]` or `#[command(version = env!("CARGO_PKG_VERSION"))]` to the `Cli` struct in `quecto-agent/src/main.rs`.

- [ ] **Step 2: Update HELP command to list parser aliases**
  Update the `HELP` constant in `quecto-agent/src/main.rs` to listaliases for exit (`/exit, /quit, /q`) and help (`/help, /h, /?`).

- [ ] **Step 3: Update `/model` command check**
  In the `ChatCommand::Model` handler, print `model: (not set)` when `model_name` is empty.

- [ ] **Step 4: Move resume message to start**
  In the `resume` function, move/reformat the `"resumed session"` log to print `"quecto-agent: resuming session {id}..."` before `agent.resume()` is called.

- [ ] **Step 5: Run tests and commit**
  Verify all CLI tests pass (running `cargo test --test cli`) and commit.
  ```bash
  git add quecto-agent/src/main.rs
  git commit -m "fix(cli): implement version flag, aliases, model check, and resume UX log"
  ```
