# quecto-agent — Extensibility & Flavors Design

> How users create their own **flavors** of the coding agent — per project or per user —
> without forking. This is a focused spec for `quecto-agent`'s extensibility model; the full
> agent loop / tools / sandbox / session design is a broader future spec. Does **not** touch
> the tiny `quecto` core (still the 4-function model adapter).

## Philosophy

Flavors are quecto's own ethos applied one layer up: **tiny, composable, unopinionated.**
`quecto-agent` ships as a *framework*, not a monolith — a library of composable pieces plus a
default binary — so a "flavor" is an override of wiring or behavior, never a divergent fork.

Forking is possible but never *required*. The whole point is that you can carry five
project-specific flavors as five small manifests (or crates) that all track upstream, instead
of five diverging clones.

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

# All optional; omitted keys fall back to env vars, then built-in defaults.
model        = "qwen3.6:35b-mlx"
base_url     = "http://localhost:11434/v1"
max_steps    = 30
system_prompt      = "You are a terse senior reviewer. Prefer diffs over prose."
# or: system_prompt_file = "prompts/reviewer.md"

[tools]
enabled = ["read_file", "search_text", "list_files", "git_diff"]  # allow-list

[approval]
default      = "read-only"    # read-only | edit | command | risky | always-ask
run_command  = "ask"
delete_file  = "deny"

[[mcp]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[verify]
test  = "cargo test"
lint  = "cargo clippy -- -D warnings"
build = "cargo build"
```

A manifest can only *wire* existing pieces (toggle built-in tools, set prompts/policy/MCP).
Genuinely new tool behavior needs a code flavor.

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

### 3. Scaffold — `quecto-agent new`

Bootstraps a starter so nobody begins from a blank file:

```
quecto-agent new my-flavor            # generates a flavor.toml starter (manifest)
quecto-agent new my-flavor --crate    # generates a dependent binary crate (code flavor)
```

### 4. Fork — the escape hatch

`git clone` and edit source. Always available for wholesale changes, but not the path for
per-project/per-user flavors (it diverges and loses easy upstream updates).

## Flavor selection & precedence

Two independent precedences.

**Which manifest is loaded** (highest wins):

```
--flavor <name>            (explicit flag)
  ↓ else
./.quecto/flavor.toml      (project-local — auto-loaded when present)
  ↓ else
~/.config/quecto/flavors/default.toml   (user default)
  ↓ else
built-in default flavor
```

`--flavor <name>` resolves `<name>` against `./.quecto/flavors/<name>.toml` then
`~/.config/quecto/flavors/<name>.toml`. Project beats user beats built-in — the same
override spirit as the core.

**Individual value overrides** (once a manifest is loaded), matching the core's config rule:

```
CLI flag  >  env var (QUECTO_*)  >  flavor manifest value  >  built-in default
```

So a flavor sets the baseline, `QUECTO_MODEL` can still override its `model` for one run, and
`--model` overrides even that. Everything stays scriptable/CI-friendly.

## Relationship to the core

Nothing here changes `quecto`. Flavors configure `quecto-agent`; the agent talks to models
only through `quecto_raw`/`quecto_stream`. A flavor's `model`/`base_url`/`api_key` simply
become the `url`/`headers`/body the agent hands to those primitives. The core stays the tiny,
opinion-free adapter it is.

## Non-goals (for this extensibility layer)

- A plugin ABI / dynamic loading (`.so`/`.dll`) — code flavors are compiled Rust crates, not
  runtime-loaded plugins. Simpler, safer, and gives real type-checking.
- A flavor marketplace/registry — flavors are just files/crates users share however they like.
- Sandboxing *flavors themselves* — a code flavor is trusted user code (it's your binary).
  Tool-level sandboxing/approvals still apply to what the model does.
