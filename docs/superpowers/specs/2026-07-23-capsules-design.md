# quecto-agent — Capsules Design

> A skills-like extensibility feature for the interactive REPL: reusable bundles of
> instructions (and optional scripts) that can be loaded, invoked, and unloaded mid-session
> via slash commands. Scoped to `quecto-agent`'s chat REPL only; the one-shot CLI path is
> untouched.

## Motivation

Users want to package a recurring workflow — a set of instructions, conventions, and
optionally helper scripts — the way Claude Code's Skills feature does, and invoke it inline
during a chat session with `/<capsule_name> <prompt>`, without leaving the REPL or restarting
the agent. Capsules can be loaded explicitly, stacked (multiple active at once), and unloaded
when no longer needed.

## Architecture overview

A new `quecto-agent/src/capsule.rs` module owns discovery, parsing, and the active-set model.
It mirrors two patterns already in the codebase: `instructions.rs` (layered AGENTS.md/CLAUDE.md
loading) and `flavor.rs` (TOML config layering with project-over-user precedence).

- **`Capsule`** — `{ name, description, instructions (markdown body), dir }`, parsed from one
  `CAPSULE.md` file.
- **`CapsuleRegistry`** — scans `~/.quecto/capsules/*/CAPSULE.md` (user scope) and
  `<cwd>/.quecto/capsules/*/CAPSULE.md` (project scope) once at REPL startup, parses each
  file's YAML frontmatter (`name`, `description`) plus markdown body, and merges the two sets:
  a project capsule overrides a user capsule of the same name. Capsules whose name collides
  with a reserved built-in command are skipped (with a warning) during discovery.
- **Active set** — the REPL's `chat()` loop holds an ordered list of currently-loaded capsule
  names (load order). Loading/unloading rebuilds the agent's system message as:
  `base_system_prompt + "\n\n## Capsule: <name>\n<instructions>"` for each active capsule, in
  load order.
- **Script execution** — no new tool-schema plumbing. A capsule's injected instructions
  mention its `scripts/` directory's absolute path (when present); the model invokes scripts
  there through the existing `run_command`/shell tool, exactly like any other repo file.

## On-disk format

```
~/.quecto/capsules/<name>/CAPSULE.md        # user scope
<repo>/.quecto/capsules/<name>/CAPSULE.md   # project scope
```

Each capsule is a directory. `CAPSULE.md` has YAML frontmatter followed by a markdown body:

```markdown
---
name: release-notes
description: Draft release notes from recent commits
---

Summarize commits since the last tag into categorized release notes
(Features / Fixes / Other). Use `scripts/collect-commits.sh <since-tag>`
to gather raw commit data before drafting.
```

An optional `scripts/` subdirectory sits alongside `CAPSULE.md`. Scripts are plain executable
files; the capsule's own instructions are responsible for telling the model when/how to use
them. There is no manifest entry for scripts and no sandboxing beyond what `run_command`
already enforces.

`name` in frontmatter should match the directory name; if they differ, the frontmatter `name`
is authoritative for matching against slash commands, but the directory name is used as a
fallback label when frontmatter is missing/malformed enough to skip parsing `name`.

## Discovery & precedence rules

1. Scan project scope, then user scope (or vice versa — order doesn't matter since merging is
   by name with project winning).
2. Within a single scope, if two capsule directories declare the same `name`, the first one
   encountered by directory scan order wins; a warning is printed for the shadowed one.
3. Across scopes, project always overrides user for the same `name`.
4. A capsule whose `name` collides with a reserved built-in command (see below) is dropped
   entirely, with a warning. Built-ins can never be shadowed.
5. A malformed `CAPSULE.md` (unreadable file, invalid YAML, missing `name`) is skipped with a
   warning; it never blocks REPL startup.

Reserved built-in names (case-insensitive): `help`, `h`, `?`, `model`, `context`, `diff`,
`status`, `undo`, `approve`, `deny`, `clear`, `exit`, `quit`, `q`, `reasoning`, `tools`,
`commands`, `capsules`, `load`, `unload`.

## Command parsing (`chat.rs`)

`parse_command` gains a second parameter: the list of known capsule names (discovered +
currently loaded).

```rust
pub fn parse_command(line: &str, capsule_names: &[String]) -> ChatCommand
```

New `ChatCommand` variants:

- `Capsules` — `/capsules`
- `LoadCapsule(String)` — `/load <name>`
- `UnloadCapsule(String)` — `/unload <name>`
- `InvokeCapsule { name: String, prompt: Option<String> }` — `/<capsule_name> [prompt text...]`

Dispatch order: reserved built-ins are matched first, exactly as today (unchanged from the
existing `match name.to_ascii_lowercase().as_str()`). Only when `name` doesn't match a
built-in does it check `capsule_names` for a case-insensitive match, producing `InvokeCapsule`.
If neither matches, falls through to the existing `Unknown(name)` — typo behavior is
unchanged.

`/load` and `/unload` validate against the full discovered registry (not just the active set):

- `/load <unknown>` → error notice, capsule not in registry.
- `/load <already-active>` → idempotent notice, no duplicate system-prompt section.
- `/unload <not-loaded>` → no-op notice.

`/<capsule_name>` with no trailing text loads the capsule (if not already active) and confirms
— equivalent to `/load <capsule_name>`. `/<capsule_name> <text>` loads it if needed (silently,
if already active) and then feeds `<text>` to the agent exactly as if it had been typed as
plain input (same path as `ChatCommand::Say`).

## REPL wiring (`main.rs`)

- `chat()` builds a `CapsuleRegistry` once at startup, after resolving `cwd`, alongside the
  existing flavor/model/agent setup.
- `handle_chat_command` gains access to the active-capsule list and the `CapsuleRegistry`, plus
  a way to rewrite the agent's system message. `Agent` gains a small
  `set_system_prompt(&mut self, text: String)` helper alongside its existing
  `clear_history`/`base_system_prompt`. `clear_history` resets to the *current* system prompt
  (base + active capsules), not a separately pinned pristine base — `base_system_prompt` is
  updated whenever the capsule set changes, so `/clear` preserves loaded capsules per the
  agreed behavior below.
- New match arms in `handle_chat_command`:
  - `Capsules` → lists all discovered capsules (name + description), marking which are
    currently active (e.g. `● foo — does the thing` vs `  bar — other thing`).
  - `LoadCapsule(name)` → look up in registry; error if missing; no-op notice if already
    active; otherwise push to active set, rebuild system prompt, notice `"loaded <name>"`.
  - `UnloadCapsule(name)` → remove from active set if present, rebuild system prompt, notice;
    otherwise notice `"<name> is not loaded"`.
  - `InvokeCapsule { name, prompt }` → load-if-needed (as above, silent if already active);
    if `prompt` is `Some`, immediately run it through the same path as
    `ChatCommand::Say(prompt)` (i.e. `agent.run(&prompt)` and render the outcome).
- `/clear` keeps calling `agent.clear_history()` unchanged; because the active-capsule set is
  session-level configuration folded into the rebuilt system prompt (not conversation state),
  it survives `/clear` — matching how `/approve`, `/deny`, and reasoning mode already survive
  `/clear` today.
- `HELP` text gets a new block:
  ```
  /capsules             list available and loaded capsules
  /load <name>          load a capsule
  /unload <name>        unload a capsule
  /<capsule_name> [text]  load a capsule (if needed) and optionally send a prompt through it
  ```

## Error handling

| Situation | Behavior |
|---|---|
| Malformed `CAPSULE.md` | Skipped at discovery; stderr warning `quecto-agent: skipping capsule at <path>: <reason>` |
| Capsule name collides with built-in | Skipped at discovery; stderr warning naming the shadowed built-in |
| Duplicate name within one scope | First by scan order wins; stderr warning for the shadowed one |
| Duplicate name across scopes | Project wins silently (documented, not an error) |
| `/load <unknown>` | REPL notice: `"no such capsule: <name> (see /capsules)"` |
| `/unload <not-loaded>` | REPL notice: `"<name> is not loaded"` |
| `/<name>` where name matches neither built-in nor capsule | Falls through to existing `Unknown` handling, unchanged |
| Capsule has no `scripts/` dir | Not an error; injected instructions omit any scripts path |

## Testing

- **`capsule.rs` unit tests**: frontmatter parsing (valid / missing fields / malformed YAML),
  directory scanning, user/project precedence merging, reserved-name collision skipping,
  duplicate-within-scope handling.
- **`chat.rs` unit tests** (extending the existing `parse_command` table): `/capsules`,
  `/load foo`, `/unload foo`, `/foo` → `InvokeCapsule{name:"foo", prompt: None}`,
  `/foo do the thing` → `InvokeCapsule{name:"foo", prompt: Some("do the thing")}`, a capsule
  name matching a built-in never produces `InvokeCapsule`, unknown name with no capsule match
  still falls to `Unknown`.
- **`main.rs` integration-style tests** (alongside the existing
  `create_session_with_reasoning_mode`-style tests): loading a capsule updates the agent's
  system prompt; unloading removes just that capsule's block while others stay; `/clear`
  preserves the active capsule set and rebuilt system prompt; loading an already-loaded
  capsule is idempotent (no duplicate section).
- **End-to-end smoke test**: piped-stdin REPL (like the existing non-interactive branch in
  `chat()`) driving `/load`, a capsule invocation with a prompt, `/unload`, `/exit` through a
  fake in-memory model, asserting the transcript/system-prompt sequence.

## Out of scope (v1)

- One-shot CLI (`quecto-agent <task>`) capsule support — REPL only.
- Capsule marketplaces, remote installation, or versioning.
- New tool-schema entries for capsule scripts — they run through the existing shell tool.
- Persisting the active-capsule set across REPL restarts/sessions.
