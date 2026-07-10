# quecto-agent — Extensibility & Flavors Design

> How users create their own **flavors** of the coding agent — per project or per user —
> without forking. This is a focused spec for `quecto-agent`'s extensibility model; the full
> agent loop / tools / sandbox / session design is in `2026-07-10-quecto-agent-architecture.md`.
> Does **not** touch the tiny `quecto` core (still the 4-function model adapter).

## Philosophy

Flavors are quecto's own ethos applied one layer up: **tiny, composable, unopinionated.**
`quecto-agent` ships as a *framework*, not a monolith — a library of composable pieces plus a
default binary — so a "flavor" is an override of wiring or behavior, never a divergent fork.

Forking is possible but never *required*. The whole point is that you can carry five
project-specific flavors as five small manifests (or crates) instead of five diverging clones.
Manifest flavors track upstream automatically (they're just config the current binary reads).
Code flavors don't *diverge* like a fork, but they do depend on the library at a semver
version and recompile against upgrades — a trait-signature change can require a small update.
That's ordinary dependency maintenance, not a merge conflict against a forked codebase.

## The two crates

```
quecto-agent (library)      # composable pieces built on the quecto core:
  - Agent (the loop)        #   loop, limits, cancellation, model calls via quecto_raw/stream
  - Tool trait + Registry   #   structured tools with name/description/schema/run
  - Policy                  #   approval levels + per-operation rules
  - Renderer                #   activity display (● lines, slash-commands)
  - Session                 #   persistence (SQLite)
  - Flavor                  #   manifest loading + precedence

quecto-agent binary         # wires the pieces into a batteries-included default agent
  (default flavor)          #   and loads a flavor manifest at startup
```

Code-flavors depend on the **library**; no-code flavors are read by the **binary**.

## What a flavor customizes

- **System prompt / persona** (terse reviewer vs. verbose pair-programmer)
- **Enabled tools** + **approval policy** per operation
- **MCP servers** to attach
- **Verification commands** (`cargo test` vs `pytest` vs `just check`)
- **Model / endpoint**, `max_steps`, renderer options

## Flavor mechanisms

### 1. Manifest flavor (no code) — the common case

A declarative `flavor.toml`. The default binary reads it; no recompilation.

```toml
name = "reviewer"

# All optional; omitted keys inherit from the layer below (see Selection & precedence).
# NOTE: api_key is NEVER read from a manifest (secret-leak risk) — env/flag only.
model           = "qwen3.6:35b-mlx"
base_url        = "http://localhost:11434/v1"
max_steps       = 30
command_timeout = 120                # seconds; sandbox wall-clock limit for run_command
edit_format     = "search-replace"   # search-replace | begin-patch | unified-diff
auto_verify     = true               # run [verify] as a completion gate (test-and-fix)
system_prompt      = "You are a terse senior reviewer. Prefer diffs over prose."
# or: system_prompt_file = "prompts/reviewer.md"

[tools]
# Allow-list over ALL registered tools (built-in + code-registered). Omit to enable all.
enabled = ["read_file", "search_text", "list_files", "git_diff"]

[approval]
# One vocabulary: every operation resolves to allow | ask | deny.
# `preset` expands to a per-operation map; explicit keys override the preset.
preset       = "read-only"    # read-only | editor | full  (presets, not a fourth verb)
run_command  = "ask"
delete_file  = "deny"

[[mcp]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[verify]
test     = "cargo test"
lint     = "cargo clippy -- -D warnings"
build    = "cargo build"
required = ["test"]    # which checks gate completion when auto_verify = true

[render]
style = "compact"    # compact | plain
color = true
```

A manifest can only *wire* existing pieces (toggle registered tools, set prompts/policy/MCP).
Genuinely new tool behavior needs a code flavor.

### Approval: one vocabulary, safe defaults

Every operation resolves to exactly **`allow | ask | deny`**. `preset` is sugar that expands
to a per-operation map, which explicit keys then override:

| Preset | Expands to |
|---|---|
| `read-only` | reads/searches `allow`; every mutation, `run_command`, network, delete → `ask`; `sudo`/outside-repo → `deny` |
| `editor` | reads + file edits `allow`; `run_command`/install/network/delete → `ask`; `sudo`/outside-repo → `deny` |
| `full` | reads/edits/`run_command` `allow`; install/network/delete/push → `ask`; `sudo`/outside-repo → `deny` |

**Built-in default when `[approval]` is omitted = `read-only`.** A flavor can only become more
permissive *explicitly*, and even `full` never auto-allows `sudo`, operations outside the
repo, or `git push`. There is no silent-permissive path.

### 2. Code flavor (custom Rust) — the power case

Depend on the `quecto-agent` library and register your own `Tool`s (or custom prompt logic),
then call `run()`. This is the idiomatic "build on a framework" path (à la Axum/clap).

```rust
use quecto_agent::{Agent, Tool, ToolResult, Context};

struct Deploy;
impl Tool for Deploy {
    fn name(&self) -> &str { "deploy" }
    fn description(&self) -> &str { "Deploy the current branch to staging." }
    fn schema(&self) -> serde_json::Value { /* JSON Schema */ }
    fn run(&self, args: &serde_json::Value, cx: &mut Context) -> ToolResult { /* … */ }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Agent::from_env()      // resolves the flavor via the precedence below
        .register(Deploy)  // add a custom tool on top of the flavor's built-ins
        .run()
}
```

The loop feeds the registry's schemas to `quecto_raw` as the `tools` array, reads back
`tool_calls`, and dispatches each to the matching registered `Tool` — the `quecto` core's
`quecto_raw`/`quecto_stream` are the only model hooks it uses.

**Registration vs. the manifest allow-list.** `register()` (built-ins + your custom tools)
defines the *universe* of tools that exist. The manifest's `[tools] enabled` is an allow-list
applied over that whole universe, custom tools included — so a manifest may name `deploy`, and
a flavor may disable a custom tool without touching code. Omitting `enabled` activates every
registered tool. Registration happens before manifest resolution so the allow-list can see
custom names.

### 3. Scaffold — `quecto-agent new`

Bootstraps a starter so nobody begins from a blank file:

```
quecto-agent new my-flavor            # generates a flavor.toml starter (manifest)
quecto-agent new my-flavor --crate    # generates a dependent binary crate (code flavor)
```

The `--crate` template pins the `quecto-agent` library at the currently-installed version, so
a generated flavor builds reproducibly and upgrades are a deliberate version bump.

### 4. Fork — the escape hatch

`git clone` and edit source. Always available for wholesale changes, but not the path for
per-project/per-user flavors (it diverges and loses easy upstream updates).

## Flavor selection & precedence

**Manifests layer and merge key-by-key** — they do not replace each other. Lower layers are
the base; higher layers override only the keys they set. A project flavor that omits `model`
still inherits the user default's `model`.

```
built-in default flavor
  ⊕ merge  ~/.config/quecto/flavor.toml            (user default)
  ⊕ merge  ~/.config/quecto/flavors/<name>.toml    (named user flavor, if --flavor)
  ⊕ merge  ./.quecto/flavor.toml                   (project default, auto-discovered)
  ⊕ merge  ./.quecto/flavors/<name>.toml           (named project flavor, if --flavor)
        = effective flavor
```

Paths are symmetric between user and project scope: each has an unnamed default
(`…/flavor.toml`) plus a `flavors/` directory of named flavors. `--flavor <name>` pulls in the
named layers; project layers override user layers.

**Then individual value overrides** apply on top of the merged manifest, matching the core's
rule:

```
CLI flag  >  env var (QUECTO_*)  >  merged flavor value  >  built-in default
```

So flavors set the baseline, `QUECTO_MODEL` can still override for one run, and `--model`
overrides even that. Everything stays scriptable/CI-friendly.

## Trust: project flavors are not auto-trusted

A `./.quecto/flavor.toml` comes from the repository — potentially untrusted. It can declare
shell commands (`[verify]`, `[[mcp]]`) and loosen `[approval]`, so auto-executing it would be
a drive-by code-execution risk (the direnv/`.envrc` problem).

**Trust-on-first-use.** The first time a project flavor is encountered — or whenever its
content hash changes — the agent shows exactly what it would do (declared commands, MCP
servers, and any approval loosening) and requires an explicit `allow`. The decision is
remembered by content hash; an unchanged, already-approved flavor loads silently thereafter.

```
$ quecto-agent "fix the failing tests"
⚠  ./.quecto/flavor.toml is new/changed and wants to:
     • run commands:  cargo test, cargo clippy
     • start MCP:     npx …@modelcontextprotocol/server-github
     • loosen approval: run_command = allow
   Allow this project flavor? [y/N]
```

Until approved, the safe fields (persona, tool *restrictions*) may apply, but
command-bearing fields and any approval loosening are ignored. User-scope flavors
(`~/.config/quecto/…`) are trusted (the user wrote them); only project-scope flavors gate.

## System prompt composition

Three sources contribute, assembled as **labeled sections in order** — all three apply, the
explicit override is appended last so it wins tie-breaks:

```
[persona]     ← flavor system_prompt / system_prompt_file
[repo rules]  ← instruction loader (AGENTS.md / CLAUDE.md / …, with its own precedence)
[override]    ← QUECTO_SYSTEM env / --system flag
```

This matches how coding agents actually layer a persona, the repository's conventions, and the
user's immediate intent. Any section may be empty. (The core's `QUECTO_SYSTEM`, by contrast,
is a *single* system message — layering is a `quecto-agent` behavior, not a core one.)

## Relationship to the core

Nothing here changes `quecto`. Flavors configure `quecto-agent`; the agent talks to models
only through `quecto_raw`/`quecto_stream`. A flavor's `model`/`base_url`/`api_key` simply
become the `url`/`headers`/body the agent hands to those primitives. The core stays the tiny,
opinion-free adapter it is.

## MCP dependency

`[[mcp]]` entries are served by the `quecto-mcp` companion, which `quecto-agent` depends on
**optionally, behind a `mcp` feature** (it carries `tokio`/`rmcp` — kept off the default build
so a minimal agent stays light). With the feature on, each `[[mcp]]` server's tools join the
registry (and are subject to the same `[tools]` allow-list and approval policy). With it off,
a manifest containing `[[mcp]]` fails fast with a clear "built without MCP support" error
rather than silently ignoring the servers.

## Non-goals (for this extensibility layer)

- A plugin ABI / dynamic loading (`.so`/`.dll`) — code flavors are compiled Rust crates, not
  runtime-loaded plugins. Simpler, safer, and gives real type-checking.
- Declarative custom tools (a "run this shell command as tool X" manifest entry). A manifest
  wires existing tools; new tool behavior is a code flavor. A declarative shell-tool bridge is
  a plausible future addition but is deliberately out of scope now (it reopens the trust
  surface and overlaps `run_command`).
- A flavor marketplace/registry — flavors are just files/crates users share however they like.
- Sandboxing *flavors themselves* — a code flavor is trusted user code (it's your binary).
  Tool-level sandboxing/approvals still apply to what the model does.
