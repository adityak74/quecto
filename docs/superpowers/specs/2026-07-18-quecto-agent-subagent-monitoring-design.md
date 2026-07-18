# Concurrent Subagent Spawning + Monitoring

## Problem

`quecto-agent` already has one subagent mechanism: the `invoke_subagent` tool
(`quecto-agent/src/tools/subagent.rs`). It is fully synchronous — the calling
agent's step blocks until the nested `Agent` finishes. There is no way for the
model to have more than one subagent "in flight" at a time, and no way to
check on a subagent's progress without waiting for it to fully complete.

This adds a second, concurrent path: spawn one or more subagents that run in
the background, and a way to check their status/progress/results while
(or after) they run. `invoke_subagent` is unchanged and remains the simple,
blocking option for a single bounded delegation.

## Non-goals

- No change to `invoke_subagent`'s existing behavior or signature.
- No cross-process distribution — subagents run as threads within the same
  `quecto-agent` process, same as today's single-subagent case.
- No persistence of subagent pool state across process restarts; it's an
  in-memory registry scoped to one top-level agent run, mirroring how
  `Context::background_processes` already works for shell commands.

## New tools

All three share one `SubagentPool` (defined below), constructed once and
cloned into each tool, the same way `AgentConfig` is already shared today.

### `spawn_subagent`

Args: `{ "prompt": string, "role": string? }` (same shape as `invoke_subagent`).

Starts a new subagent on a background thread and returns immediately with an
integer id, e.g. `"spawned subagent #3"`. Rejects the call with an error
`ToolOutput` if `MAX_CONCURRENT_SUBAGENTS` (8) are already running — this is a
deliberate guardrail, since we've already seen a model loop on a tool call
when confused (see the `RepeatGuard` fix in the prior session).

### `monitor_subagents`

Args: `{ "id": integer? }`.

- With `id`: reports that subagent's status (`running` / `complete` /
  `cancelled` / `failed`), elapsed time, and a tail of its recent activity
  (last N tool calls/results). If `complete`, includes the final result text.
  If `failed`, includes the panic message.
- Without `id`: a status line per subagent spawned so far this run (id,
  role, status, elapsed), newest first.

Read-only; classified in `policy.rs` alongside `invoke_subagent` as always
allowed (no approval prompt), same as `list_background_processes` today.

### `cancel_subagent`

Args: `{ "id": integer }`.

Flips that subagent's own cancel flag. The subagent's `Agent::run_loop`
already checks its `CancelToken` at each step/tool boundary, so it stops
promptly rather than immediately — same latency as today's Ctrl-C handling.
Returns an error `ToolOutput` if the id is unknown or already finished.

## State: `SubagentPool`

```rust
#[derive(Clone)]
pub struct SubagentPool {
    next_id: Arc<AtomicU32>,
    handles: Arc<Mutex<HashMap<u32, SubagentHandle>>>,
}

struct SubagentHandle {
    role: String,
    prompt: String,
    started: Instant,
    cancel: CancelToken,           // this subagent's own token
    progress: Arc<Mutex<Vec<String>>>, // ring buffer, capped e.g. at 50 lines
    status: Arc<Mutex<RunStatus>>,
}

enum RunStatus {
    Running,
    Complete(String),
    Cancelled,
    Failed(String),
}
```

Cloning `SubagentPool` is cheap (two `Arc`s); each of the three tools holds
its own clone pointing at the same underlying map, so all three see the same
set of subagents regardless of which tool instance is invoked.

## Live progress capture

The existing `RunRecorder` trait (`agent.rs`) — currently used only for
SQLite session persistence — is reused here. A small `ProgressRecorder`
implements `RunRecorder::message` and `RunRecorder::change`, formatting each
observation into a one-line summary (tool name + args, or result snippet) and
pushing it into the handle's `progress` ring buffer (`Vec<String>`, truncated
to the last 50 entries on push). `monitor_subagents` reads this buffer
directly — no need to reach into the private state of a running `Agent` on
another thread.

## Cancellation cascade

Each spawned subagent gets its own fresh `CancelToken` (`Arc::new(AtomicBool::new(false))`),
independent of the parent's, so `cancel_subagent` can stop one without
affecting siblings or the parent run. To avoid orphaning subagents if the
*parent* is cancelled (Ctrl-C, or the parent's own `Outcome::Cancelled`), a
small watcher thread is spawned alongside each subagent:

```rust
let parent_cancel = config.cancel.clone();
let child_cancel = handle_cancel.clone();
thread::spawn(move || {
    while !parent_cancel.load(Ordering::SeqCst) && !child_cancel.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(200));
    }
    child_cancel.store(true, Ordering::SeqCst);
});
```

This watcher exits as soon as either token flips, so it doesn't leak.

## Thread lifecycle

The subagent's worker thread wraps `subagent.run(prompt)` in
`std::panic::catch_unwind` so a panic becomes `RunStatus::Failed(message)`
instead of poisoning the mutex or leaving `monitor_subagents` reporting
`running` forever. The `JoinHandle` itself is not retained/joined — status is
read from the shared `RunStatus`, not from thread completion, so nothing
blocks waiting on it. (This matches the existing `background_processes`
pattern, where processes are tracked by state, not by blocking on them.)

## Wiring into `Agent`

`agent.rs::register_builtins_filtered` already special-cases
`invoke_subagent`; the same block gains a `SubagentPool::new()` plus
conditional registration of the three new tools (respecting the same
tool-name allow-list mechanism already used for every other builtin).

## Policy

`monitor_subagents` is read-only/side-effect-free from the policy engine's
point of view and joins `invoke_subagent` in the existing always-allowed list
in `policy.rs::Policy::decide`. `spawn_subagent` and `cancel_subagent` start
and stop background work the same way `start_background_process` and
`kill_background_process` do today, so they route through `self.run`
(`Policy.run`) exactly like those two — `Decision::Ask` under the default
`read-only` preset, `Decision::Allow` under `full`, same as the existing
process tools. This is a correction from an earlier draft of this doc, which
mistakenly described the process tools as always-allowed; they are not —
only `read_file`/`list_files`/`search_text`/`git_diff`/`git_status`/
`search_notes`/`list_background_processes`/`invoke_subagent` are.

## Testing

- Unit tests for `SubagentPool`: id allocation, cap enforcement, status
  transitions (running → complete/cancelled/failed), progress buffer
  capping.
- Integration-style test spawning a subagent with a trivial task (e.g. "read
  a known file"), polling `monitor_subagents` until `complete`, asserting the
  result text.
- Test that `cancel_subagent` on a deliberately slow/looping task transitions
  status to `cancelled` within a bounded wait.
- Test that exceeding `MAX_CONCURRENT_SUBAGENTS` returns an error `ToolOutput`
  rather than spawning.
