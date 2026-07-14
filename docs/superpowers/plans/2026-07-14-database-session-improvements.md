# Database and Session Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix error swallowing in `take_last_change()`, update `now()` to use milliseconds to prevent casting wrap-around, and wrap multi-statement writes in transactions.

**Architecture:** Modify `session.rs` to propagate rusqlite errors, update timestamp conversion, and add transaction wrapping around `record_message`.

**Tech Stack:** Rust, rusqlite

## Global Constraints

- Preserve all existing comments and docstrings.
- Follow TDD.
- Ensure all tests pass.

---

### Task 1: Fix take_last_change DB error swallowing

**Files:**
- Modify: `quecto-agent/src/session.rs:276-297`

- [ ] **Step 1: Update error handling in `take_last_change`**
  In `quecto-agent/src/session.rs`, modify the query handler of `take_last_change` to match `rusqlite::Error::QueryReturnedNoRows` instead of converting all errors to `None` with `.ok()`:
  ```rust
  let query_result = self.conn.query_row(
      "SELECT id, path, before, after FROM file_changes \
       WHERE session_id = ?1 ORDER BY seq DESC, id DESC LIMIT 1",
      [id],
      |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
  );
  let (row_id, path, before, after) = match query_result {
      Ok(val) => val,
      Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
      Err(e) => return Err(e.into()),
  };
  ```

- [ ] **Step 2: Run tests to verify correctness**
  Run: `cargo test --package quecto-agent --lib session::tests`
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add quecto-agent/src/session.rs
  git commit -m "fix(db): propagate database errors in take_last_change"
  ```

---

### Task 2: Use milliseconds for now() to prevent wrap-around

**Files:**
- Modify: `quecto-agent/src/session.rs:51-56`

- [ ] **Step 1: Modify `now()` in `session.rs`**
  Change `as_nanos() as i64` to `as_millis() as i64`:
  ```rust
  fn now() -> i64 {
      SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .map(|d| d.as_millis() as i64)
          .unwrap_or(0)
  }
  ```

- [ ] **Step 2: Run tests and verify**
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add quecto-agent/src/session.rs
  git commit -m "fix(db): use millisecond timestamp to prevent i64 overflow in 292 years"
  ```

---

### Task 3: Wrap record_message in database transactions

**Files:**
- Modify: `quecto-agent/src/session.rs:169-187`

- [ ] **Step 1: Wrap statements in `record_message`**
  Modify `record_message` to run within a SQL transaction:
  ```rust
  pub fn record_message(&self, id: &str, seq: i64, m: &Message) -> Result<(), BoxErr> {
      self.conn.execute("BEGIN TRANSACTION", [])?;
      let res1 = self.conn.execute(
          "INSERT INTO messages (session_id, seq, role, content, tool_calls, tool_call_id) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
          (
              id,
              seq,
              &m.role,
              &m.content,
              calls_to_json(&m.tool_calls),
              &m.tool_call_id,
          ),
      );
      if let Err(e) = res1 {
          let _ = self.conn.execute("ROLLBACK", []);
          return Err(e.into());
      }
      let res2 = self.conn.execute(
          "UPDATE sessions SET updated = ?2 WHERE id = ?1",
          (id, now()),
      );
      if let Err(e) = res2 {
          let _ = self.conn.execute("ROLLBACK", []);
          return Err(e.into());
      }
      self.conn.execute("COMMIT", [])?;
      Ok(())
  }
  ```

- [ ] **Step 2: Run tests and verify**
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add quecto-agent/src/session.rs
  git commit -m "fix(db): execute record_message within a transaction for atomicity"
  ```
