# quecto-agent — Capsule Create Design

> A follow-on to the capsules feature (`docs/superpowers/specs/2026-07-23-capsules-design.md`):
> a REPL command that lets a user describe what a capsule should do and has the agent draft and
> write the `CAPSULE.md` for them, immediately loaded into the current session. This is the
> "create your own capsule" counterpart to Claude Code's Skills authoring flow, bundled directly
> in `quecto-agent` — no external tooling required.

## Motivation

Capsules (v1) are entirely hand-authored: a user must manually create
`~/.quecto/capsules/<name>/CAPSULE.md` (or the project equivalent) with an editor, following the
YAML-frontmatter-plus-markdown format themselves. This is a low-friction win only for users who
already know the format. `/capsule-create` closes that gap by letting the agent itself draft a
well-formed capsule from a short natural-language description, entirely inside the REPL.

## Command

```
/capsule-create <name> <what it should do>
```

- `<name>` — the capsule's name (single token, no spaces).
- `<what it should do>` — free-text description of the capsule's purpose; used as the drafting
  prompt sent to the model.

Both arguments are required. `"capsule-create"` is added to `capsule::RESERVED_NAMES` (see the
v1 spec) so no discovered capsule can ever shadow this command.

## `ChatCommand` parsing (`chat.rs`)

New variant:

```rust
CreateCapsule { name: String, description: String }
```

Parsing: `/capsule-create <name> <rest of line>` → `CreateCapsule { name, description: rest }`.
If `<rest of line>` is empty (name given, no description) or `<name>` is missing entirely, this
falls through to a usage notice at dispatch time (consistent with how `/load`/`/unload` handle
missing arguments today) rather than a new `Unknown` variant — no special parse-time error type
is introduced.

## Handler flow (`main.rs` / `handle_chat_command`)

1. **Validate before drafting** (no model call is made unless all of these pass):
   - `name` non-empty and `description` non-empty → else usage notice:
     `"usage: /capsule-create <name> <what it should do>"`.
   - `capsule::is_reserved(name)` is false → else error notice:
     `"<name> is a reserved command name, choose another"`.
   - `name` not already present in the live `CapsuleState`'s registry (project or user scope) →
     else error notice: `"capsule <name> already exists (see /capsules)"`.
2. **Draft**: build a meta-prompt and run it through the existing `agent.run()` path (the same
   call `InvokeCapsule`'s optional prompt uses):

   > Draft a CAPSULE.md file for a new capsule named `<name>` that does the following:
   > `<description>`. Output ONLY the file content: YAML frontmatter with `name: <name>` and a
   > one-line `description:`, followed by a `---` closing delimiter and a markdown instructions
   > body. Wrap the entire file content in a single fenced code block and output nothing else.

   This is a normal turn — it's appended to conversation history and rendered like any other
   response, exactly as `/<capsule_name> <prompt>` already is.
3. **Extract**: pull the contents of the first fenced code block (```` ``` ````...```` ``` ````,
   language tag ignored if present) out of the model's final response text. New pure function in
   `capsule.rs`:

   ```rust
   pub fn extract_fenced_block(text: &str) -> Result<String, String>
   ```

   Returns `Err("no fenced code block found in model output")` if none is present.
4. **Validate**: run the extracted text through the existing `Capsule::parse` (same validator
   `CapsuleRegistry` discovery uses). If parsing fails, surface the same error message
   `Capsule::parse` already produces (e.g. `"missing frontmatter delimiter"`).
5. **Reconcile name**: if the parsed capsule's `name` doesn't case-insensitively match the
   requested `<name>`, overwrite the parsed `Capsule.name` field with the requested `<name>` —
   the CLI argument is authoritative, not whatever the model wrote.
6. **Write**: `fs::create_dir_all` the project capsule directory
   (`capsule::project_capsules_dir(cwd).join(name)`), then write `CAPSULE.md` with the
   (name-reconciled) validated content. I/O failure → error notice with the OS error message;
   nothing is registered/loaded.
7. **Register + load**: insert the new `Capsule` into the live `CapsuleRegistry` (new
   `CapsuleRegistry::insert(&mut self, capsule: Capsule)` method — the registry becomes mutable
   after discovery, used only by this path), then push it onto `CapsuleState`'s active set and
   rebuild the system prompt, mirroring what `CapsuleState::load` already does.
8. **Notice**: `"created and loaded capsule <name> at <path>"`.

If step 2–5 fails, nothing is written, nothing is registered — the user can retry
`/capsule-create` (perhaps with a clearer description) or write the file by hand.

## Error handling

| Situation | Behavior |
|---|---|
| Missing `<name>` or `<description>` | Usage notice; no model call |
| `<name>` collides with a reserved built-in (including `capsule-create` itself) | Error notice; no model call |
| `<name>` already exists in the registry (either scope) | Error notice; no model call |
| `<name>` is not a single clean path component (contains `/`, is `.`/`..`, or is an absolute path) | Error notice; no model call |
| Model output has no fenced code block | Error notice: `"capsule draft failed: no fenced code block found in model output"`; nothing written |
| Fenced block fails `Capsule::parse` | Error notice with the parser's reason; nothing written |
| Model's `name:` frontmatter field ≠ requested `<name>` | Silently corrected to the requested name before writing (not an error) |
| Filesystem write failure | Error notice with the OS error; nothing registered |

## Testing

- **`capsule.rs` unit tests**: `extract_fenced_block` — happy path (plain and language-tagged
  fences), no-fence error, multiple fences (first one wins); `CapsuleRegistry::insert` — new
  capsule appears in `names()`/`get()`, overwrites an existing in-memory entry of the same
  (lowercased) name.
- **`chat.rs` parse-table tests**: `/capsule-create foo does the thing` →
  `CreateCapsule { name: "foo", description: "does the thing" }`; `/capsule-create foo` (no
  description) and `/capsule-create` (no args) both fall through to the existing usage-notice
  path rather than a new parse variant.
- **`main.rs` integration test**: drive `/capsule-create demo do the thing` through
  `handle_chat_command` against a fake model scripted to reply with a fenced CAPSULE.md block;
  assert the file lands under the project's `.quecto/capsules/demo/CAPSULE.md`, `CapsuleState`
  reports it active, and the rendered system prompt contains its `## Capsule: demo` section.
- **`main.rs` rejection test**: `/capsule-create` targeting an already-registered name is
  rejected before the fake model is invoked (assert the fake model's call counter is unchanged).
- **`main.rs` malformed-draft test**: fake model scripted to reply with no fenced block (or an
  invalid one) → notice surfaces the parse error, and neither the filesystem nor the registry is
  touched.

## Out of scope (v1)

- `--user` scope flag (project scope only) and `--force` overwrite.
- Scaffolding a capsule's `scripts/` subdirectory.
- Editing or deleting an existing capsule via command (`/capsule-edit`, `/capsule-delete`).
- One-shot CLI support — REPL only, matching the v1 capsules spec.
