# quecto — Project Summary

## Overview
**quecto** (SI prefix for 10⁻³⁰, the smallest SI unit) is a minimal AI harness and coding agent written in Rust. The project embodies the philosophy that everything should be built from the smallest composable units.

**Two companion crates:**
- **`quecto`** — Core library (~1.3 MB stripped): One endpoint for any OpenAI-compatible LLM (cloud or local). Zero async runtime, statically-linked, two dependencies (`ureq`, `serde_json`).
- **`quecto-agent`** — Coding agent (~3.5 MB stripped): Multi-step tool use, approval-gated edits, sandbox, session persistence, flavor manifests, OTEL tracing — all built on the core's primitives.

## Architecture
```
quecto-core (lib)  ──┐
                      ├──▶ quecto-agent  (coding agent binary)
                      ├──▶ quecto-mcp    (MCP client, in progress)
                      └─── evals/    (benchmark suite)
```

**Core Library API (4 functions):**
- `quecto_raw(url, headers, body)` → `Result<Value>` (buffered)
- `quecto_stream(url, headers, body, on_delta)` → `Result<String>` (streamed SSE)
- `quecto_to(prompt, base_url, api_key, model)` → convenience over primitives
- `quecto(prompt)` → reads env vars, full OpenAI flavor

## Key Features

### quecto-core
- Buffered + streaming chat completions (SSE + non-SSE fallback)
- OpenAI-compatible endpoint abstraction (Ollama, LM Studio, vLLM, OpenAI)
- CLI: one-shot, interactive REPL (`Ctrl-D` to quit), `--init` for eval-ready config

### quecto-agent
- Multi-step tool use: file read/write/patch, search, git, shell, background processes, `.qkb` notes, subagent delegation
- Approval sandbox: hard-denylist (blocks `sudo`, `rm -rf /`, `git push`, etc.)
- SQLite-backed session persistence: resume, undo, diff
- Named flavor manifests (`.quecto/flavors/*.toml`) with trust-on-first-use
- OpenTelemetry tracing (gated by `otel` feature): hierarchical spans for agent runs, steps, tool dispatches; auto-redaction of secrets
- Interactive chat REPL with crossterm event loop + bracketed paste

### Evaluation Suite
- **Smoke tests**: 10 TerminalBench-style tasks with deterministic `verify.sh` verifiers (no LLM judge needed)
- **Harbor adapter** for full 89-task Terminal-Bench 2.x benchmark
- Tasks cover: Git conflicts, refactoring, CLI tools, Docker, debugging, SQL, Rust builds, TLS

## Status
✅ **Shipped**: `quecto` core, `quecto-agent`, evaluation suite (183 tests passing)
🚧 **In Progress**: `quecto-mcp` (MCP client for STDIO / Streamable HTTP transports)

## Dependencies
- Core: 2 direct (`ureq`, `serde_json`), ~30 transitive. No tokio, no reqwest, no async.
- Agent: Adds crossterm, regex, ignore, rusqlite, clap, serde, toml, sha2
- OTEL (optional): tracing, opentelemetry crates + tokio for OTLP

## License
MIT — © 2026 Aditya

## Key Docs
- `README.md` — Full project documentation
- `docs/UAT-report.md` — Acceptance test results (34 pass, 7 polish partials)
- `docs/superpowers/` — Milestone specs and plans
