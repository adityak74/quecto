# quecto-agent — Architecture (MVP)

> The coding-agent product built on the tiny `quecto` core. This is the **MVP** architecture:
> a working local coding agent — loop, tools, patch engine, sandbox, context, verification,
> session, renderer, CLI. Serious-harness additions (planning, checkpoints, compaction,
> subagents, observability) are **deferred**, each its own later spec.
>
> Companion specs: `2026-07-09-quecto-harness-design.md` (core),
> `2026-07-10-quecto-agent-flavors-design.md` (extensibility/flavors),
> `2026-07-09-full-harness-reference.md` (maximal reference).

## Scope & non-goals

**In scope (the 13-item minimum):** CLI + streaming output, model calls via the core, the
agent/tool loop, instruction loading, the ~9 built-in tools, the search/replace patch engine,
the command sandbox + approval policy, deterministic context retrieval, the verification
(test-and-fix) loop, SQLite session persistence, the activity renderer.

**Deferred to later specs:** planning/task-tracking, checkpoints/undo beyond in-session,
context compaction, subagents, observability/metrics, embeddings/vector retrieval, a
declarative shell-tool format.

## Execution model

**Synchronous by default — no `tokio` in the default build**, matching the core. The agent
loop is inherently sequential (reason → tools → observe → repeat), tools do blocking file/
process I/O, and `quecto_raw`/`quecto_stream` are called directly. The rare need for parallel
tool calls within a turn uses `std::thread`. Only the optional **`mcp` feature** pulls in
`tokio` + `rmcp`, and it runs a runtime scoped to MCP clients — the core loop stays sync.

### Cancellation (this fixes the core's Ctrl-C limitation)

A `SIGINT` handler (via `ctrlc`) sets an `AtomicBool` "cancel requested" rather than killing
the process. The loop checks it:

- **Between steps and tool calls** → abort the turn, return to the prompt (chat) or exit
  cleanly (one-shot).
- **During model streaming** → because `quecto_stream` reads line-by-line synchronously, the
  agent's delta handler checks the flag per chunk and stops early.
- **During `run_command`** → the child runs in its own process group; on cancel the whole
  process tree is killed (see [Sandbox](#command-sandbox)).

So the agent gets real per-turn cancellation without async. A second Ctrl-C within a short
window exits hard.

## Crate layout

`quecto-agent` is a **library + default binary** (per the flavors design). Modules:

```
quecto-agent/
├── Cargo.toml            # [lib] + [[bin]] quecto-agent; feature "mcp"
├── src/
│   ├── lib.rs            # re-exports: Agent, Tool, Policy, Renderer, Session, Flavor
│   ├── agent.rs          # the loop, state, limits, completion, cancellation
│   ├── model.rs          # StreamAssembler over quecto_stream/quecto_raw
│   ├── tools/
│   │   ├── mod.rs        # Tool trait, Registry, dispatch
│   │   ├── fs.rs         # read_file, list_files, write_file
│   │   ├── search.rs     # search_text (ripgrep libs)
│   │   ├── patch.rs      # apply_patch (search/replace engine)
│   │   ├── shell.rs      # run_command (sandbox)
│   │   ├── git.rs        # git_diff, git_status
│   │   └── ask.rs        # ask_user
│   ├── sandbox.rs        # process groups, timeout, output cap, redaction, denylist
│   ├── policy.rs         # approval resolution (allow|ask|deny + presets)
│   ├── context.rs        # discovery, git snapshot, token budget, retrieval helpers
│   ├── instructions.rs   # AGENTS.md/CLAUDE.md loader → [repo rules] section
│   ├── verify.rs         # completion gate + [verify] command runner
│   ├── session.rs        # SQLite: persist + resume
│   ├── flavor.rs         # manifest load/merge, trust-on-first-use (see flavors spec)
│   ├── render.rs         # activity lines, streaming, slash-commands
│   └── main.rs           # CLI: clap subcommands/flags → Agent
│   └── mcp.rs            # (feature "mcp") rmcp clients → tools in the registry
```

Code-flavors depend on the **library** and `register()` custom tools; the **binary** is the
default flavor.

## The agent loop

```rust
pub struct Agent { /* registry, policy, renderer, session, flavor, model cfg, limits */ }

pub enum Outcome { Complete(String), StepLimit, Cancelled, Error(Box<dyn Error + Send + Sync>) }

impl Agent {
    pub fn from_env() -> Result<Self, …>;   // resolves flavor (merge + trust), builds registry
    pub fn register(self, tool: impl Tool + 'static) -> Self;
    pub fn run(&mut self, task: &str) -> Outcome;
}
```

One iteration:

```
loop {
    if cancel_requested()          -> return Cancelled
    if step >= max_steps           -> return StepLimit

    messages = build_messages(state)          // layered system + history + observations
    body     = { model, messages, tools: registry.enabled_schemas(), stream: true }

    // StreamAssembler renders content deltas live AND reconstructs the full assistant
    // message (content + tool_calls) from on_delta(&Value). See model.rs.
    msg = model.turn(url, headers, body, &renderer)?

    append(state, msg)                         // assistant message into history

    if msg.tool_calls.is_empty() {
        // model is done talking → maybe verify, then finalize
        if changed_files() && flavor.auto_verify {
            match verify.run() {
                Passed          -> return Complete(msg.content)
                Failed(report)  -> { append_observation(report); continue }  // test-and-fix
            }
        }
        return Complete(msg.content)
    }

    for call in msg.tool_calls {               // execute each tool call
        if cancel_requested() { return Cancelled }
        let decision = policy.decide(&call);   // allow | ask | deny
        let out = match decision {
            Deny        => ToolOutput::denied(),
            Ask if !renderer.confirm(&call) => ToolOutput::denied(),
            _           => registry.dispatch(&call, &mut cx),
        };
        append_tool_result(state, &call, truncate(out));   // tool message into history
    }
    step += 1
}
```

Loop invariants enforced: `max_steps`, per-tool-output truncation (head+tail with a byte cap),
cancellation checks, and a **repeated-action guard** (if the same tool+args is issued N times
consecutively with identical results, inject a nudge/observation to break the loop).

### StreamAssembler (`model.rs`)

Wraps `quecto_stream`. Its `on_delta(&Value)` closure:
1. appends `delta.content` to a buffer and prints it live through the renderer;
2. merges `delta.tool_calls` fragments (index, id, name, arguments) into an accumulating map.

On stream end it yields a complete `AssistantMessage { content, tool_calls, finish_reason }`.
This is where the "reassemble partial tool_calls" work the core deliberately skipped lives —
exactly what the core's `on_delta(&Value)` hook was designed to enable. If `QUECTO_STREAM=0`
or streaming is unsupported, it falls back to a single `quecto_raw` call.

## Tool system

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;        // the model reads this to choose the tool
    fn schema(&self) -> serde_json::Value; // JSON Schema for arguments
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult;
}
pub type ToolResult = Result<ToolOutput, ToolError>;   // ToolError is reported to the model, not fatal
```

`Context` exposes the repo root, the `Policy`, the `Renderer` (for progress), the `Session`,
and helpers (safe path resolution, git). The **Registry** holds the universe of registered
tools; `enabled_schemas()` applies the flavor's `[tools]` allow-list (over built-ins *and*
code-registered tools). `dispatch()` routes a `tool_call` to the matching `Tool`, returning a
structured error message to the model (never a panic) when a tool fails.

### Path safety (shared by every fs/patch/shell tool)

All paths are resolved against the repo root and **must** canonicalize to inside it; any
`..`/symlink escape is rejected before I/O. There is no generic unrestricted filesystem tool.

## Built-in tools (the essential ~9)

| Tool | Purpose | Notes |
|---|---|---|
| `read_file` | Read a UTF-8 file, optional `start_line`/`end_line` | Output capped; large files require a range |
| `list_files` | List a directory / the tree | Respects `.gitignore` via the `ignore` crate |
| `search_text` | Regex/literal search across the repo | ripgrep libraries (`grep`, `ignore`) — no `rg` binary needed |
| `write_file` | Create or overwrite a whole file | For new files / wholesale rewrites; records prior content |
| `apply_patch` | Edit via search/replace blocks | See [Patch engine](#patch-engine); the primary edit path |
| `run_command` | Run a shell command in the repo | Fully sandboxed + approval-gated (see [Sandbox](#command-sandbox)) |
| `git_diff` | Show working-tree diff | Read-only |
| `git_status` | Show working-tree status | Read-only |
| `ask_user` | Ask the human a question mid-task | Pauses for input; the human-in-the-loop escape |

Verification (`test`/`lint`/`build`) is **not** separate tools — it runs the flavor's
`[verify]` commands through the same sandboxed `run_command` path.

## Patch engine (`apply_patch`)

Default format: **search/replace blocks** (flavor-configurable via `edit_format`).

```
------ src/auth.rs
<<<<<<< SEARCH
const TIMEOUT: u64 = 1000;
=======
const TIMEOUT: u64 = 5000;
>>>>>>> REPLACE
```

Engine rules:
1. Resolve + path-check the target file.
2. Locate the `SEARCH` block **exactly** (whitespace-sensitive). If not found, return a
   structured error so the model can re-read and retry — never a fuzzy/ambiguous apply.
3. If the `SEARCH` text occurs more than once, reject as ambiguous (ask the model to include
   more surrounding context).
4. Replace, preserving the file's existing line endings.
5. Record the previous file contents (for in-session undo and the session log).
6. Produce a git-style diff for the renderer (`+N -M`).

Multiple blocks per turn are applied in order; a later failure does not roll back earlier
successful blocks (each block is reported independently to the model). An empty `SEARCH`
creates a new file (equivalent to `write_file`).

## Command sandbox

`run_command` is the most dangerous tool; it enforces:

- **Repo-scoped cwd** — commands run at the repo root; cwd cannot be moved outside it.
- **Approval** — the `Policy` is consulted first (`allow|ask|deny`); `ask` prompts the human.
- **Denylist** — obviously destructive patterns are hard-denied regardless of policy
  (`sudo`, `rm -rf /`, disk/`mkfs` ops, writing outside the repo). Denylist beats `full`.
- **Timeout** — a wall-clock limit (flavor `command_timeout`, default e.g. 120s) via a
  wait-with-timeout; on expiry the process tree is killed.
- **Process groups** — the child is spawned in its own group so timeout/cancel kills the whole
  tree, not just the shell.
- **Output cap** — stdout/stderr captured up to a byte cap, truncated head+tail with a
  `[… N bytes truncated …]` marker so a runaway command can't blow the context budget.
- **Secret redaction** — `QUECTO_API_KEY` and configured secret patterns are stripped from the
  child's environment and redacted from captured output before it reaches the model.

## Approval policy (`policy.rs`)

Enforces the flavor's `[approval]` (defined in the flavors spec): every operation resolves to
`allow | ask | deny`; `preset` (`read-only`/`editor`/`full`) expands to a per-operation map;
the built-in default is `read-only`; `sudo`/outside-repo/`push` are never auto-allowed. The
policy classifies each tool call by operation (read, edit, run_command, delete, network,
install, push) and returns the decision the loop acts on. `ask` decisions route to the
renderer's confirmation prompt.

## Context engine (`context.rs`)

Deterministic, no embeddings (MVP). Rather than pre-stuffing the whole repo, the agent gives
the **model the tools to retrieve** (model-directed search) and seeds a small starting context:

- **Seed context** (once, in the first system/context message): repo root, a shallow file
  tree, current `git status`, and the working-tree `git diff` if non-empty.
- **Retrieval** happens through `list_files`/`search_text`/`read_file` as the model requests.
- **Token budget**: every tool output is truncated to a cap; `read_file` favors ranges; the
  seed tree is depth-limited. A running estimate guards the total; when near the model's
  window the oldest tool observations are dropped first (full compaction is deferred).

Discovery respects `.gitignore` (the `ignore` crate). This matches the reference's guidance
that ripgrep + file paths + model-directed search suffice for the MVP.

## Instruction loader (`instructions.rs`)

Builds the `[repo rules]` section of the layered system prompt (see the flavors spec's
[System prompt composition]). It walks from the repo root to the working directory collecting
`AGENTS.md` / `CLAUDE.md` (and `.agent/instructions.md`), nearer files taking precedence, and
concatenates them under labeled headers. The result is one section; the flavor persona and the
`QUECTO_SYSTEM`/`--system` override wrap around it.

## Verification loop (`verify.rs`)

When the model stops with edits present and `auto_verify` is on, the agent runs the flavor's
`[verify]` commands (through the sandbox) as an explicit **completion gate**:

```
complete = changes_exist
        && diff_reviewed            // the agent surfaced the diff to the user
        && required_checks_passed   // [verify] test/lint/build as configured
        && no_unresolved_tool_errors
```

A failing check is fed back as an observation and the loop continues (test-and-fix) until it
passes or `max_steps` is hit. With `auto_verify` off, the model drives verification itself via
`run_command`. Which checks are "required" is a flavor setting.

## Session state (`session.rs`)

SQLite (via `rusqlite`, bundled) at `~/.local/state/quecto/sessions.db` (XDG). Enough to
resume an interrupted task.

```sql
sessions(id, task, repo, flavor, model, created, updated, status)
messages(id, session_id, role, content, ts)              -- full transcript
tool_calls(id, session_id, name, args, result, ts)
file_changes(id, session_id, path, before, after, ts)    -- enables in-session undo
usage(session_id, prompt_tokens, completion_tokens, turns)
```

`quecto-agent resume <id>` rehydrates messages + state and continues the loop.
`quecto-agent undo` reverts the most recent `file_changes` row (restores `before`).
`quecto-agent diff` prints the accumulated working-tree diff.

## Renderer & CLI (`render.rs`, `main.rs`)

**Renderer** — line-based activity output (not a full-screen TUI; `crossterm` for color +
prompts, no `ratatui` for MVP). Live-streams assistant text; prints one activity line per tool:

```
● Searching "authenticate"          7 matches
● Reading src/auth/middleware.rs
● Editing src/auth/middleware.rs     +12 -4
● Running cargo test                 18 passed, 1 failed
● Running cargo test                 19 passed
```

Hidden reasoning is never shown; only actions and results. In `chat` mode, slash-commands:
`/help /model /context /diff /status /undo /approve /deny /clear /exit`.

**CLI** (`clap`, justified here — the agent has real subcommands, unlike the tiny core):

```
quecto-agent "fix the failing auth tests"    # one-shot run
quecto-agent chat                            # interactive session
quecto-agent resume <id>
quecto-agent diff | undo
quecto-agent new <flavor> [--crate]          # scaffold (flavors spec)
quecto-agent init                            # full install wizard (flavors spec)
```

Flags (override env, which overrides the flavor — per the flavors precedence):
`--flavor <name>`, `--model`, `--base-url`, `--max-steps`, `--approval <preset>`,
`--cwd <dir>`, `--no-stream`.

> Binary name (`quecto-agent` vs. a shorter alias, possibly shipping *as* `quecto` with the
> tiny core kept purely as a library) is a **branding decision** left open; the spec uses
> `quecto-agent` throughout.

## Dependencies

The heavy product layer — deliberately more than the core's two, but still curated:

| Crate | Purpose |
|---|---|
| `quecto` | the model-adapter core (`quecto_raw`/`quecto_stream`) |
| `serde` / `serde_json` | tool schemas, tool_call parsing, message assembly |
| `toml` | flavor manifests |
| `clap` | CLI subcommands/flags |
| `rusqlite` (bundled) | session persistence |
| `ignore` | gitignore-aware file discovery (ripgrep library) |
| `grep` | in-process text search (ripgrep library) — no `rg` binary needed |
| `crossterm` | color + interactive prompts (no full TUI) |
| `ctrlc` | SIGINT → cancel flag |
| `wait-timeout` | command timeout without async |
| `sha2` | flavor trust hashing |
| `tokio` + `rmcp` | **only** behind the `mcp` feature |

Still **no `tokio` in the default build**. `main` is a plain `fn main()`.

## Relationship to the core

`quecto-agent` talks to models **only** through `quecto_raw`/`quecto_stream`. A flavor's
`model`/`base_url`/`api_key` become the `url`/`headers`/`body` handed to those primitives; the
`StreamAssembler` consumes the core's `on_delta(&Value)` to reconstruct tool calls. Everything
stateful, opinionated, or heavy lives here — the core stays the tiny, sync, opinion-free
4-function adapter.

## Deferred (future specs)

Planning/task-tracking · checkpoints & restore (beyond in-session undo) · context compaction ·
subagents (explorer/implementer/reviewer) · observability/metrics · embeddings/vector
retrieval · declarative shell-tool manifests. Each is additive and does not alter the MVP
contracts above.
