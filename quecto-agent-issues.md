# quecto / quecto-agent — Issues Found in Code Review (Main Branch Audit)

This document has been audited against the `quecto` main branch (`On branch main`). The status of each issue has been updated to reflect whether it is **Pending** (still in the codebase), **Resolved** (already addressed in the codebase), **Resolved (Uncommitted)** (addressed in the local unstaged changes currently in the workspace), or a **False Positive**.

---

## 1. High / Critical Severity Issues

### Bug: `/context` never shows token usage (chat.rs / main.rs)
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:512-514` (handler), `quecto-agent/src/main.rs:418` (HELP text)
* **Findings:** The issue remains in the codebase. The `/context` command handler only prints `session: {session_id}`. There is no message/character count or token estimation helper implemented in `Agent`.
* **Fix Required:** Add a helper (e.g., `character_count()` or `token_estimate()`) to `Agent` that iterates over `self.messages` and sums the lengths, then update the handler in `main.rs` to print it.

### Bug: `Agent::run()` / `run_loop()` does not sync all exit paths
* **Status:** 🟢 **RESOLVED (Uncommitted)**
* **Files:** `quecto-agent/src/main.rs` (`resume` function)
* **Findings:** `run_loop()` correctly flushes changes to the recorder on all paths. The real issue was that `Agent::resume()` initialized the agent using `String::new()` as the system prompt instead of composing it. This has been resolved in the workspace's uncommitted changes by calling `compose_system_with_persona` and passing `system` in `resume()`.

### Bug: Missing `/tools` command
* **Status:** 🟢 **RESOLVED (Uncommitted)**
* **Files:** `quecto-agent/src/chat.rs`, `quecto-agent/src/main.rs`
* **Findings:** Successfully addressed in the local uncommitted changes. `/tools` (and its alias `/commands`) has been added to the `ChatCommand` enum and successfully implemented in the chat REPL loop.

### Bug: `Agent::resume()` uses empty system prompt
* **Status:** 🟢 **RESOLVED (Uncommitted)**
* **Files:** `quecto-agent/src/main.rs`
* **Findings:** Resolved by the uncommitted changes in `main.rs` (lines 626-658) which compose the system prompt using env vars, personas, and repository/CLAUDE.md instructions, passing it to `Agent::new()`.

### Bug: `take_last_change()` swallows DB errors silently
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/session.rs:276-297`
* **Findings:** The database error swallowing via `.ok()` on line 285 is still present in `session.rs`. If the query fails due to a database issue (e.g. corruption, lock, schema changes), it returns `Ok(None)` ("no changes to undo") rather than propagating the error.
* **Fix Required:** Propagate database errors by matching specifically against `rusqlite::Error::QueryReturnedNoRows` to return `Ok(None)`, and return any other error.

---

## 2. Medium Severity Issues

### M-1: `/status` may show another session's status
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:515-522`
* **Findings:** Still present. The `/status` handler queries `store.latest_session()`, which returns the globally most-recently-updated session. In concurrent scenarios, this will display the status of a different session instead of the current active session.
* **Fix Required:** Implement a query on `Store` that fetches the status of the specific `session_id` and use that in the `/status` handler.

### M-2: `clear_history()` does not reset `recorded_changes` or clear Context changes
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/agent.rs:176-179`
* **Findings:** Still present. When `/clear` is called, it truncates messages and resets `recorded_messages`, but `recorded_changes` remains stale, and the `Context`'s accumulated `changes` vector is not cleared.
* **Fix Required:** Reset `self.recorded_changes = 0` in `clear_history()`, and add a method to `Context` to clear its tracked `changes`.

### M-3: Sandbox cross-boundary secret redaction gap
* **Status:** 🟡 **FALSE POSITIVE / RESOLVED**
* **Files:** `quecto-agent/src/sandbox.rs:362`
* **Findings:** The audit's claim is a false positive. The codebase dynamically calculates the `overlap` window as `max_secret_length - 1` and sorts environment secrets by length in descending order (`sandbox.rs` line 267). This guarantees that any secret straddling chunk boundaries will be correctly retained in the `pending` buffer and matched in the subsequent iteration without partial matches leaking.

### M-4: `now()` nanos-to-i64 cast wraps after ~292 years
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/session.rs:51-56`
* **Findings:** Still present. Casting `duration_since(UNIX_EPOCH).as_nanos()` (which is `u128`) to `i64` will wrap in the year 2262.
* **Fix Required:** Use a lower-precision timestamp (e.g. `as_millis()`) or defensively clamp/saturate the cast.

### M-5: No transactional wrapping for multi-statement writes
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/session.rs:169-187` (`record_message`)
* **Findings:** Still present. Message insertion and session timestamp updates are executed as two independent statements outside of a database transaction.
* **Fix Required:** Wrap multi-statement database modifications inside a transaction.

### M-6: `/context` command may fail when store is not available
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:512-514`
* **Findings:** The `/context` command won't fail (it prints the local `session_id` variable directly), but because of the token-usage display bug (H-1), it does not print actual context sizes.

### NEW M-7: CRLF line endings break `apply_patch` tool
* **Status:** 🔴 **PENDING (NEW FINDING)**
* **Files:** `quecto-agent/src/tools/patch.rs:13-66` (`parse_patch`), `quecto-agent/src/tools/patch.rs:76-86` (`apply_to_text`)
* **Findings:** `parse_patch` processes patch text using `text.lines()`, which strips `\r` (carriage returns) from line endings. The search blocks are then re-assembled with `\n`. When matching these against files on disk that use CRLF (`\r\n`), `content.matches(&block.search)` will fail due to line ending mismatches.
* **Fix Required:** Normalize line endings (e.g., stripping `\r` from target file contents before matching, or adapting search blocks to match file endings).

---

## 3. Low Severity Issues & Polish

### L-1: HELP text omits parser aliases
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/chat.rs:28-37` vs `quecto-agent/src/main.rs`
* **Findings:** Parser accepts aliases like `/q`, `/quit`, and `/h`, but they are not listed in the `/help` output.

### L-2: `/model` shows blank when no model is configured
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:511`
* **Findings:** Still present. Prints `model: ` when `QUECTO_MODEL` and flavor model are unset.

### L-3: `resume()` unconditional stderr print after success
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:677`
* **Findings:** Still present. Printing "resumed session {id}" after completion has already occurred is redundant.

### L-4: No `--version` flag
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs` (`Cli` struct)
* **Findings:** Still present. No `--version` option exists.

### Polish: HELP text has inconsistent indentation
* **Status:** 🟢 **RESOLVED (Uncommitted)**
* **Files:** `quecto-agent/src/main.rs:418-429`
* **Findings:** Addressed in the uncommitted changes. The HELP text was reformatted to a clean, single-line-per-command layout with consistent padding.

### Polish: No `/clear` confirmation for session state
* **Status:** 🔴 **PENDING**
* **Files:** `quecto-agent/src/main.rs:551-554`
* **Findings:** Still present. Clear only shows a generic "conversation cleared" message.

---

## 4. Audit Summary Table

| ID | Severity | Issue | Files | Status on main |
|---|---|---|---|---|
| **H-1** | High | `/context` says "transcript size" but only prints session ID | `main.rs:512-514` | 🔴 **Pending** |
| **H-2** | High | `resume()` prints "resumed" even on failure / redundant message | `main.rs:677` | 🟡 **False Positive (on failure) / Pending (UX Polish)** |
| **H-3** | High | `take_last_change()` silently swallows DB errors via `.ok()` | `session.rs:276-289` | 🔴 **Pending** |
| **M-1** | Medium | `/status` may show another session's status | `main.rs:515-521` | 🔴 **Pending** |
| **M-2** | Medium | `clear_history()` leaves `recorded_changes` stale | `agent.rs:176-179` | 🔴 **Pending** |
| **M-3** | Medium | Sandbox secret redaction may leak across chunk boundaries | `sandbox.rs:362` | 🟢 **False Positive (dynamically sized overlap)** |
| **M-4** | Medium | Nanos-to-i64 cast wraps after ~292 years | `session.rs:51-56` | 🔴 **Pending** |
| **M-5** | Medium | No DB transactions for multi-statement writes | `session.rs:169-187` | 🔴 **Pending** |
| **M-6** | Medium | `/context` does not use agent's internal messages | `main.rs:512-514` | 🔴 **Pending** |
| **M-7** | Medium | CRLF line endings break `apply_patch` tool | `tools/patch.rs` | 🔴 **Pending (New)** |
| **L-1** | Low | HELP omits parser aliases (`/h`, `/q`, etc.) | `chat.rs` / `main.rs` | 🔴 **Pending** |
| **L-2** | Low | `/model` shows blank when no model is configured | `main.rs:511` | 🔴 **Pending** |
| **L-3** | Low | `resume()` redundant success message after `finish()` | `main.rs:677` | 🔴 **Pending** |
| **L-4** | Low | No `--version` flag | `main.rs` (`Cli`) | 🔴 **Pending** |
| **L-5** | Low | HELP text has inconsistent indentation | `main.rs:418-429` | 🟢 **Resolved (Uncommitted)** |
| **L-6** | Low | `/clear` lacks detailed confirmation | `main.rs` | 🔴 **Pending** |