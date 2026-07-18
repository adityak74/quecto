# Project Overview

**quecto** — The leanest, fastest, smallest AI harness and coding agent built on it. Named after the SI prefix for 10⁻³⁰ (the smallest unit in the metric system).

Built by **Aditya Karnam**. MIT licensed. Actively developed (status: M1–M7b shipped).

---

## Cargo Workspace

Three crates defined in the root `Cargo.toml`:

| Crate | Purpose | Status | Size (optimized) |
|-------|---------|--------|------------------|
| **`quecto`** | Core library & CLI — prompt → LLM → output | ✅ Shipped | ~1.3 MB |
| **`quecto-agent`** | Coding agent — multi-step tool loop, session persistence, chat REPL | ✅ Shipped | ~3.5 MB |
| **`quecto-mcp`** | MCP client for the agent (STDIO, Streamable HTTP, legacy SSE) | 🚧 In Progress | TBD |

---

## Core Crate (`quecto`)

### What it does
Take a prompt, run it through any OpenAI-compatible LLM (cloud: OpenAI; local: Ollama, LM Studio, vLLM), return output buffered or streamed. **One job, zero opinions.**

### Library API (4 functions)
```rust
// Primitives — you supply the exact URL, headers, and JSON body.
quecto_raw(url, headers, body)     -> Result<Value, Error>  // buffered
quecto_stream(url, headers, body, on_delta) -> Result<String, Error>  // streamed (SSE)

// Conveniences — OpenAI-flavored sugar over the primitives.
quecto_to(prompt, base_url, api_key, model) -> Result<String, Error>
quecto(prompt)                     -> Result<String, Error>  // reads env vars
```

### CLI modes
- **One-shot**: `quecto "write me a haiku"` — prints answer, exits.
- **REPL**: `quecto` — interactive turns, `Ctrl-D` to quit.
- **Bootstrap**: `eval "$(quecto --init)"` — prints eval-able env exports.

### Dependencies
Only **2 direct dependencies**, ~30 transitive:
- `ureq` (v2, json feature) — synchronous HTTP with rustls TLS
- `serde_json` (v1) — build request bodies, parse responses

**No `tokio`, no `reqwest`, no async runtime.**

### Key source files
- `src/lib.rs` — core library exports (`quecto_raw`, `quecto_stream`)
- `src/main.rs` — CLI entry point

---

## Agent Crate (`quecto-agent`)

### What it does
Full coding agent built entirely on the core's `quecto_raw` primitive. Multi-step tool use, file edits under approval (or deny list), sandbox denylist, verification gates, SQLite-backed session persistence (resume/undo/diff), named flavor manifests with trust-on-first-use — all in a 3.5 MB binary with zero async.

### CLI subcommands
- **One-shot**: `quecto-agent "<task>"` [options]
- **Chat REPL**: `quecto-agent chat` — interactive turns with rotating loading verbs
- **Resume**: `quecto-agent resume <session-id>`
- **Undo**: `quecto-agent undo` — revert last file change
- **Diff**: `quecto-agent diff` — summarize file changes this session
- **New flavor**: `quecto-agent new <flavor-name>` — scaffold `.quecto/flavors/`

### Chat REPL slash commands
| Command | Description |
|---------|-------------|
| `/help` | Show command list |
| `/commands` | List available tools for this session |
| `/model` | Show active model name |
| `/context` | Session ID, message count, approx. character count |
| `/status` | Session ID and current status |
| `/diff` | Summarise file changes this session |
| `/undo` | Revert the last recorded file change |
| `/approve` | Auto-approve all edits/commands for this session |
| `/deny` | Deny all edits/commands for this session |
| `/clear` | Forget the conversation (keeps system prompt) |
| `/exit` | Leave chat |

### Tool set (8 tool modules)
| Tool | File | Purpose |
|------|------|---------|
| `read_file` | `tools/fs.rs` | Read file contents |
| `write_file` | `tools/fs.rs` | Write/overwrite files |
| `list_files` | `tools/fs.rs` | List directory entries |
| `search_text` | `tools/search.rs` | Grep for patterns in files |
| `apply_patch` | `tools/patch.rs` | Apply unified diff patches |
| `git_diff`, `git_status`, `git_init`, etc. | `tools/git.rs` | Git operations |
| `run_command` | `tools/shell.rs` | Execute shell commands |
| `start_background_process`, `kill_background_process` | `tools/subagent.rs` | Background process management |
| `invoke_subagent` | `tools/subagent.rs` | Delegate to a subagent |
| `read_note`, `write_note`, etc. | `tools/notes.rs` | Read/write `.qkb` Markdown notes |

### Safety features
- **Hard denylist**: blocks `sudo`, `rm -rf /`, `git push` even under `--yes`
- **Verification gate**: `QUECTO_VERIFY` runs shell commands (tests, linters) as post-edit checks
- **Approval presets**: default denies writes/commands in non-interactive mode; `/approve` or `--yes` lifts it; denylist always wins

### Session persistence (SQLite-backed)
- **On disk**: plaintext SQLite DB (`~/.local/state/quecto/sessions.db`) with `messages` and `file_changes` tables
- Powers: `resume`, `undo`, `diff`
- No encryption or expiry — local dev database, not a secrets store

### Flavor system (BYOC)
Named profiles in `.quecto/flavors/*.toml` bundle system prompt, tool policy, and defaults into a swappable manifest with content-hash **trust-on-first-use**.

---

## MCP Client (`quecto-mcp`)

Build the agent with `--features mcp` to consume any MCP-compatible server as a tool source. Supports:

| Transport | Use case |
|-----------|----------|
| `stdio` | Local single-user tools (Claude Desktop / Cursor pattern) |
| `streamable_http` | Remote/production (current MCP standard, March 2025+) |
| `sse` | Legacy servers only — deprecated compat support |

Tools are prefixed `mcp__<server>__<tool>` to avoid collisions with native tools.

---

## Key Source Files Summary

### Core (`quecto/src/`)
- `lib.rs` — public API, primitives, conveniences
- `main.rs` — CLI binary

### Agent (`quecto-agent/src/`)
- `agent.rs` — the main agent loop & tool dispatch
- `chat.rs` — chat REPL interaction loop
- `sandbox.rs` — hard denylist enforcement
- `session.rs` — SQLite session persistence
- `flavor.rs` / `trust.rs` — flavor manifests + trust-on-first-use
- `verify.rs` — verification gate runner
- `model.rs` — model selection & chat iteration
- `render.rs` — activity renderer (spinner verbs)
- `policy.rs` / `approval.rs` — tool policies & approval logic
- `recorder.rs` — message recording for persistence
- `context.rs` — context management
- `instructions.rs` — system prompt handling
- `mcp_adapter.rs` — MCP tool integration adapter

### MCP (`quecto-mcp/src/`)
- `lib.rs`, `protocol.rs`, `registry.rs`, `server.rs` — core client logic
- `transport/` — STDIO, SSE, Streamable HTTP transport backends
- `config.rs`, `error.rs`, `tofu.rs` — configuration & trust-on-first-use

---

## Evaluation Suite (`evals/`)

Smoke tests (10 TerminalBench-style tasks with deterministic `verify.sh`):
1. `tb_01_git_conflict_resolution` — 3-way merge conflict + commit
2. `tb_02_package_refactoring` — Flat scripts → Python package
3. `tb_03_advanced_sed_awk` — CSV cleaning with awk/sed only
4. `tb_04_openssl_decryption` — AES-256-CBC decryption
5. `tb_05_dynamic_dependency_script` — BS scraper with self-installing fallback
6. `tb_06_docker_build` — Fix broken Dockerfile, build & run
7. `tb_07_debug_c_crash` — NULL-deref segfault fix + recompile
8. `tb_08_sqlite_query` — Query SQLite DB to file
9. `tb_09_fix_rust_build` — Fix immutability + logic errors, build release
10. `tb_10_openssl_selfsigned_cert` — Self-signed cert generation & expiry verify

Plus Harbor/Adapter for **89-task Terminal-Bench 2.x** benchmark (`evals/harbor/quecto_agent.py`).

---

## Architecture Philosophy

```
mega (10⁶) → kilo (10³) → base → milli (10⁻³) → micro (10⁻⁶) → ... → quecto (10⁻³⁰)
```
1. **LLMs are the backend.** The harness is just the glue.
2. **Everything is composable.** Small pieces → big things.
3. **Describe it, run it.** If you can explain it to an LLM, quecto handles it.

The core primitives (`quecto_raw`, `quecto_stream`) decide nothing and discard nothing. Every opinion is optional sugar. Companions (agent, MCP) build on top of `quecto_raw` without polluting the core with async, tool execution, or state.

---

## Milestones & Recent Announcements (in reverse chronological order)

| Date | Milestone / Feature |
|------|---------------------|
| 2026-07-16 | Chat UI Polish & Tool Enhancements — verbose tool summaries, `.qkb` note directory enforcement |
| 2026-07-16 | Chat UX: `crossterm` event loop, Bracketed Paste, background process tracking, `invoke_subagent`, `.qkb` notes |
| 2026-07-15 | Evaluation Suite shipped — 10 smoke tasks + Harbor Terminal-Bench adapter |
| 2026-07-15 | OpenTelemetry (OTEL) tracing support (`otel` feature flag) |
| 2026-07-14 | UAT accepted: 41 scenarios, 0 failures, all partials resolved — ACCEPT verdict |
| 2026-07-14 | Bug-fix release (CRLF patches, DB errors, millisecond timestamps, `/status`, CLI UX) |
| 2026-07-12 | `quecto-agent` shipped (M1–M7b): tools, editing under approval, sandbox, verification, session persistence, flavors |
| 2026-07-10 | Core crate landed: library API, SSE + non-SSE streaming, REPL, CLI. Clippy-clean. 24 tests. |
| 2026-07-10 | Size-optimized build: ~1.3 MB core, ~3.5 MB agent — statically linked, no runtime |

---

## Testing Summary
**183 passing tests** across both crates (clippy-clean). The `cargo test --workspace` command runs all of them.
