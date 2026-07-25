# Capsule Create Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/capsule-create <name> <what it should do>` REPL command that has the agent draft a new `CAPSULE.md` from a natural-language description, writes it to the project capsule scope, and loads it into the current session immediately.

**Architecture:** Three additive layers matching the existing capsules code: pure helpers in `quecto-agent/src/capsule.rs` (fenced-block extraction, mutable registry insert, create-and-load on `CapsuleState`), a new `ChatCommand::CreateCapsule` variant parsed in `quecto-agent/src/chat.rs`, and a new match arm in `handle_chat_command` (`quecto-agent/src/main.rs`) that validates, drafts via `agent.run()`, parses, writes, and registers.

**Tech Stack:** Rust (existing `quecto-agent` crate), `std::fs`, existing `Agent`/`Outcome`/`Renderer` types — no new dependencies.

## Global Constraints

- Command name: `/capsule-create <name> <what it should do>`, both arguments required.
- `"capsule-create"` must be added to `capsule::RESERVED_NAMES` so no discovered capsule can shadow it.
- Project scope only (`<cwd>/.quecto/capsules/<name>/CAPSULE.md`) — no `--user` flag, no `--force` overwrite, no `scripts/` scaffolding, no edit/delete commands. These are all out of scope for v1 per the spec.
- No model call is made unless name/description are present, the name isn't reserved, and the name doesn't already exist in the registry.
- On success the capsule is registered in the live `CapsuleRegistry` and pushed onto `CapsuleState`'s active set in the same call — usable on the very next turn, no `/load` needed.
- Reference spec: `docs/superpowers/specs/2026-07-25-capsule-create-design.md`.

---

### Task 1: `capsule.rs` — fenced-block extraction, mutable registry insert, create-and-load

**Files:**
- Modify: `quecto-agent/src/capsule.rs`

**Interfaces:**
- Consumes: existing `Capsule` struct/fields, existing `CapsuleRegistry { capsules: BTreeMap<String, Capsule> }`, existing `CapsuleState { registry, base_system_prompt, active }`, existing private `Capsule::parse(text: &str, dir: PathBuf) -> Result<Capsule, String>`.
- Produces (used by Task 3/4):
  - `pub fn extract_fenced_block(text: &str) -> Result<String, String>`
  - `Capsule::parse` becomes `pub fn parse(text: &str, dir: PathBuf) -> Result<Capsule, String>` (was private)
  - `impl CapsuleRegistry { pub fn insert(&mut self, capsule: Capsule) }`
  - `impl CapsuleState { pub fn create_and_load(&mut self, capsule: Capsule) }`
  - `RESERVED_NAMES` gains `"capsule-create"`.

- [ ] **Step 1: Write failing tests for `extract_fenced_block`**

Add to the `#[cfg(test)] mod tests` block in `quecto-agent/src/capsule.rs`:

```rust
    #[test]
    fn extract_fenced_block_returns_plain_fence_contents() {
        let text = "here you go:\n```\n---\nname: demo\n---\nbody\n```\nhope that helps";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "---\nname: demo\n---\nbody");
    }

    #[test]
    fn extract_fenced_block_strips_language_tag() {
        let text = "```markdown\n---\nname: demo\n---\nbody\n```";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "---\nname: demo\n---\nbody");
    }

    #[test]
    fn extract_fenced_block_uses_first_fence_when_multiple_present() {
        let text = "```\nfirst\n```\nand another:\n```\nsecond\n```";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "first");
    }

    #[test]
    fn extract_fenced_block_errors_when_no_fence_present() {
        let err = extract_fenced_block("just plain text, no fences here").unwrap_err();
        assert_eq!(err, "no fenced code block found in model output");
    }

    #[test]
    fn extract_fenced_block_errors_when_fence_never_closes() {
        let err = extract_fenced_block("```\nname: demo\nno closing fence").unwrap_err();
        assert_eq!(err, "no fenced code block found in model output");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent extract_fenced_block --lib`
Expected: FAIL with "cannot find function `extract_fenced_block`"

- [ ] **Step 3: Implement `extract_fenced_block`**

Add this function near the top of `quecto-agent/src/capsule.rs`, after the `use` statements and before the `Capsule` struct:

```rust
/// Pull the contents of the first fenced code block (```` ``` ````...```` ``` ````,
/// optional language tag on the opening fence ignored) out of `text`. Used by
/// `/capsule-create` to extract the drafted `CAPSULE.md` body from the model's
/// reply.
pub fn extract_fenced_block(text: &str) -> Result<String, String> {
    const ERR: &str = "no fenced code block found in model output";
    let start = text.find("```").ok_or_else(|| ERR.to_string())?;
    let after_start = &text[start + 3..];
    let content_start = after_start.find('\n').map(|i| i + 1).unwrap_or(0);
    let rest = &after_start[content_start..];
    let end = rest.find("```").ok_or_else(|| ERR.to_string())?;
    Ok(rest[..end].trim_end().to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent extract_fenced_block --lib`
Expected: PASS (5 tests)

- [ ] **Step 5: Write failing test for `CapsuleRegistry::insert`**

Add to the same test module:

```rust
    #[test]
    fn registry_insert_adds_a_capsule_findable_by_name() {
        let mut registry = CapsuleRegistry::default();
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "d".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
        });
        assert_eq!(registry.get("demo").unwrap().description, "d");
        assert_eq!(registry.names(), vec!["demo".to_string()]);
    }

    #[test]
    fn registry_insert_overwrites_existing_entry_of_same_name() {
        let mut registry = CapsuleRegistry::default();
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "old".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
        });
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "new".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
        });
        assert_eq!(registry.get("demo").unwrap().description, "new");
        assert_eq!(registry.names().len(), 1);
    }
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test -p quecto-agent registry_insert --lib`
Expected: FAIL with "no method named `insert` found"

- [ ] **Step 7: Implement `CapsuleRegistry::insert`**

Add this method inside `impl CapsuleRegistry { ... }`, after the existing `iter` method:

```rust
    /// Insert or replace a capsule in the registry (case-insensitive key).
    /// Used by `/capsule-create` to register a freshly drafted capsule
    /// without a full re-scan of disk.
    pub fn insert(&mut self, capsule: Capsule) {
        let key = capsule.name.to_lowercase();
        self.capsules.insert(key, capsule);
    }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p quecto-agent registry_insert --lib`
Expected: PASS (2 tests)

- [ ] **Step 9: Write failing test for `CapsuleState::create_and_load`**

Add to the same test module:

```rust
    #[test]
    fn create_and_load_registers_and_activates_the_capsule() {
        let mut state = CapsuleState::new(CapsuleRegistry::default(), "base".to_string());
        state.create_and_load(Capsule {
            name: "demo".to_string(),
            description: "d".to_string(),
            instructions: "Follow the demo workflow.".to_string(),
            dir: PathBuf::from("/capsules/demo"),
        });

        assert!(state.is_active("demo"));
        assert!(state.registry().get("demo").is_some());
        assert!(state
            .render_system_prompt()
            .contains("Follow the demo workflow."));
    }
```

- [ ] **Step 10: Run test to verify it fails**

Run: `cargo test -p quecto-agent create_and_load --lib`
Expected: FAIL with "no method named `create_and_load` found"

- [ ] **Step 11: Implement `CapsuleState::create_and_load`**

Add this method inside `impl CapsuleState { ... }`, after the existing `unload` method:

```rust
    /// Register `capsule` in the live registry and immediately mark it
    /// active. Used by `/capsule-create` right after writing a freshly
    /// drafted `CAPSULE.md` to disk — the capsule is usable on the very
    /// next turn without a separate `/load`.
    pub fn create_and_load(&mut self, capsule: Capsule) {
        let name = capsule.name.clone();
        self.registry.insert(capsule);
        self.active.push(name);
    }
```

- [ ] **Step 12: Run test to verify it passes**

Run: `cargo test -p quecto-agent create_and_load --lib`
Expected: PASS

- [ ] **Step 13: Make `Capsule::parse` public and add the reserved name**

In `quecto-agent/src/capsule.rs`, change:

```rust
    fn parse(text: &str, dir: PathBuf) -> Result<Capsule, String> {
```

to:

```rust
    /// Parse a `CAPSULE.md` file's contents into a `Capsule`. Public so
    /// `/capsule-create` can validate a model-drafted capsule body before
    /// writing it to disk, reusing the exact same validation discovery uses.
    pub fn parse(text: &str, dir: PathBuf) -> Result<Capsule, String> {
```

Then update the `RESERVED_NAMES` array (it currently ends `"commands", "capsules", "load", "unload",`) to also include `"capsule-create"`:

```rust
pub const RESERVED_NAMES: &[&str] = &[
    "help", "h", "?", "model", "context", "diff", "status", "undo", "approve", "deny", "clear",
    "exit", "quit", "q", "reasoning", "tools", "commands", "capsules", "load", "unload",
    "capsule-create",
];
```

- [ ] **Step 14: Write failing test for the new reserved name**

Add to the test module:

```rust
    #[test]
    fn capsule_create_is_a_reserved_name() {
        assert!(is_reserved("capsule-create"));
        assert!(is_reserved("Capsule-Create"));
    }
```

- [ ] **Step 15: Run full capsule.rs test suite**

Run: `cargo test -p quecto-agent --lib capsule::`
Expected: PASS (all capsule.rs tests, including the new ones and `capsule_create_is_a_reserved_name`)

- [ ] **Step 16: Commit**

```bash
git add quecto-agent/src/capsule.rs
git commit -m "feat(capsule): add fenced-block extraction and mutable registry insert for /capsule-create"
```

---

### Task 2: `lib.rs` — export `extract_fenced_block`

**Files:**
- Modify: `quecto-agent/src/lib.rs:25-28`

**Interfaces:**
- Consumes: `capsule::extract_fenced_block` from Task 1.
- Produces: `quecto_agent::extract_fenced_block` usable from `main.rs`.

- [ ] **Step 1: Add the export**

Change the `pub use capsule::{...}` block in `quecto-agent/src/lib.rs` from:

```rust
pub use capsule::{
    default_user_capsules_dir, is_reserved, project_capsules_dir, Capsule, CapsuleRegistry,
    CapsuleState, RESERVED_NAMES,
};
```

to:

```rust
pub use capsule::{
    default_user_capsules_dir, extract_fenced_block, is_reserved, project_capsules_dir, Capsule,
    CapsuleRegistry, CapsuleState, RESERVED_NAMES,
};
```

- [ ] **Step 2: Verify the workspace still builds**

Run: `cargo build -p quecto-agent`
Expected: builds cleanly, zero warnings

- [ ] **Step 3: Commit**

```bash
git add quecto-agent/src/lib.rs
git commit -m "feat(capsule): export extract_fenced_block from quecto-agent lib"
```

---

### Task 3: `chat.rs` — `ChatCommand::CreateCapsule` parsing

**Files:**
- Modify: `quecto-agent/src/chat.rs`

**Interfaces:**
- Consumes: nothing new — same `parse_command(line: &str, capsule_names: &[String]) -> ChatCommand` entry point.
- Produces (used by Task 4): `ChatCommand::CreateCapsule { name: String, description: String }` variant.

- [ ] **Step 1: Write failing parse-table tests**

Add to the `#[cfg(test)] mod tests` block in `quecto-agent/src/chat.rs`, near the existing `LoadCapsule`/`UnloadCapsule` tests:

```rust
    #[test]
    fn capsule_create_with_name_and_description() {
        assert_eq!(
            parse_command("/capsule-create foo does the thing", &[]),
            ChatCommand::CreateCapsule {
                name: "foo".to_string(),
                description: "does the thing".to_string()
            }
        );
    }

    #[test]
    fn capsule_create_with_name_but_no_description() {
        assert_eq!(
            parse_command("/capsule-create foo", &[]),
            ChatCommand::CreateCapsule {
                name: "foo".to_string(),
                description: String::new()
            }
        );
    }

    #[test]
    fn capsule_create_with_no_args() {
        assert_eq!(
            parse_command("/capsule-create", &[]),
            ChatCommand::CreateCapsule {
                name: String::new(),
                description: String::new()
            }
        );
    }

    #[test]
    fn capsule_create_trims_extra_whitespace_around_description() {
        assert_eq!(
            parse_command("/capsule-create foo   does the thing  ", &[]),
            ChatCommand::CreateCapsule {
                name: "foo".to_string(),
                description: "does the thing".to_string()
            }
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p quecto-agent --lib chat::`
Expected: FAIL to compile — `ChatCommand::CreateCapsule` doesn't exist yet

- [ ] **Step 3: Add the `CreateCapsule` variant and parsing arm**

In `quecto-agent/src/chat.rs`, add the variant to the `ChatCommand` enum (after `InvokeCapsule`):

```rust
    InvokeCapsule { name: String, prompt: Option<String> },
    CreateCapsule { name: String, description: String },
```

Then add a match arm in `parse_command`, alongside the existing `"load"`/`"unload"` arms:

```rust
        "load" => ChatCommand::LoadCapsule(remainder.to_string()),
        "unload" => ChatCommand::UnloadCapsule(remainder.to_string()),
        "capsule-create" => {
            let mut parts = remainder.splitn(2, char::is_whitespace);
            let cap_name = parts.next().unwrap_or("").to_string();
            let description = parts.next().unwrap_or("").trim().to_string();
            ChatCommand::CreateCapsule {
                name: cap_name,
                description,
            }
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --lib chat::`
Expected: PASS (all chat.rs tests, including the 4 new ones)

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/chat.rs
git commit -m "feat(capsule): parse /capsule-create <name> <description> command"
```

---

### Task 4: `main.rs` — wire `/capsule-create` into `handle_chat_command`

**Files:**
- Modify: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: `ChatCommand::CreateCapsule { name, description }` (Task 3), `quecto_agent::{Capsule, is_reserved, extract_fenced_block, project_capsules_dir}` (Tasks 1-2), existing `CapsuleState::create_and_load` (Task 1), existing `agent.run(&str) -> Outcome`, existing `run_and_render` pattern for rendering non-`Complete` outcomes.
- Produces: working `/capsule-create` end-to-end behavior; no further consumers.

- [ ] **Step 1: Update imports**

In `quecto-agent/src/main.rs`, extend the `use quecto_agent::{...}` block (lines 2-10) to include `Capsule` and `extract_fenced_block` and `is_reserved`:

```rust
use quecto_agent::{
    cancel_token, chat_spinner_renderer, content_hash, default_user_capsules_dir,
    extract_fenced_block, is_reserved, join_url, load_instructions, new_session_id,
    parse_command, parse_spinner_verbs, project_capsules_dir, project_raw,
    render_assistant_text, render_change_summary, resolve_scoped_configured, seed_context, Agent,
    ApprovalMode, Capsule, CapsuleRegistry, CapsuleState, ChatCommand, ConfiguredFlavor, Flavor,
    HttpModel, LineRenderer, Message, Outcome, Policy, Preset, Provider, ReasoningCommand,
    ReasoningMode, Renderer, SqliteRecorder, Store, TrustStore, Verifier,
};
```

- [ ] **Step 2: Add the HELP text line**

In the `HELP` constant (`quecto-agent/src/main.rs:680`), add a line after the existing `/<capsule_name> [text]` line:

```rust
/<capsule_name> [text]  load a capsule (if needed) and optionally send a prompt through it
/capsule-create <name> <what it should do>  draft and load a new capsule via the agent
```

- [ ] **Step 3: Add the `CreateCapsule` match arm**

In `handle_chat_command` (`quecto-agent/src/main.rs`), add a new arm after the existing `ChatCommand::InvokeCapsule { .. } => { ... }` block and before `ChatCommand::Unknown(name) => { ... }`:

```rust
        ChatCommand::CreateCapsule { name, description } => {
            if name.is_empty() || description.is_empty() {
                out.notice("usage: /capsule-create <name> <what it should do>");
            } else if is_reserved(&name) {
                out.notice(&format!(
                    "{name} is a reserved command name, choose another"
                ));
            } else if capsules.registry().get(&name).is_some() {
                out.notice(&format!("capsule {name} already exists (see /capsules)"));
            } else {
                let meta_prompt = format!(
                    "Draft a CAPSULE.md file for a new capsule named `{name}` that does the \
                     following: {description}. Output ONLY the file content: YAML frontmatter \
                     with `name: {name}` and a one-line `description:`, followed by a `---` \
                     closing delimiter and a markdown instructions body. Wrap the entire file \
                     content in a single fenced code block and output nothing else."
                );
                match agent.run(&meta_prompt) {
                    Outcome::Complete(answer) => {
                        out.assistant(&answer);
                        let dir = project_capsules_dir(cwd).join(&name);
                        let drafted = extract_fenced_block(&answer)
                            .and_then(|block| Capsule::parse(&block, dir.clone()));
                        match drafted {
                            Ok(mut capsule) => {
                                if !capsule.name.eq_ignore_ascii_case(&name) {
                                    capsule.name = name.clone();
                                }
                                if let Err(e) = std::fs::create_dir_all(&dir) {
                                    out.notice(&format!(
                                        "capsule draft failed: could not create {}: {e}",
                                        dir.display()
                                    ));
                                } else {
                                    let file_text = format!(
                                        "---\nname: {}\ndescription: {}\n---\n{}\n",
                                        capsule.name, capsule.description, capsule.instructions
                                    );
                                    match std::fs::write(dir.join("CAPSULE.md"), file_text) {
                                        Ok(()) => {
                                            let path = dir.join("CAPSULE.md");
                                            capsules.create_and_load(capsule);
                                            agent.messages[0] =
                                                Message::system(capsules.render_system_prompt());
                                            out.notice(&format!(
                                                "created and loaded capsule {name} at {}",
                                                path.display()
                                            ));
                                        }
                                        Err(e) => out.notice(&format!(
                                            "capsule draft failed: could not write CAPSULE.md: {e}"
                                        )),
                                    }
                                }
                            }
                            Err(reason) => {
                                out.notice(&format!("capsule draft failed: {reason}"));
                            }
                        }
                    }
                    Outcome::StepLimit => out.notice("(step limit reached)"),
                    Outcome::VerificationFailed { attempts } => out.notice(&format!(
                        "(verification still failing after {attempts} attempts)"
                    )),
                    Outcome::Cancelled => out.notice("(cancelled)"),
                    Outcome::RepeatedAction => out.notice("(stopped: repeated action)"),
                    Outcome::Blocked => out.notice(
                        "(stopped: actions denied — use /approve to allow this session)",
                    ),
                    Outcome::Error(e) => out.notice(&format!("(error: {e})")),
                }
            }
        }
```

- [ ] **Step 4: Write the rejection test (name already exists — no model call)**

Add to `mod main_tests` in `quecto-agent/src/main.rs`, near the other capsule tests:

```rust
    #[test]
    fn capsule_create_rejects_existing_name_without_calling_the_model() {
        let dir = tempfile::tempdir().unwrap();
        write_capsule(dir.path(), "demo", "demo capsule", "body");
        let mut capsules = capsules_from(dir.path());
        // fake_agent's FakeModel would return this reply if called; assert it wasn't.
        let mut agent = fake_agent("```\n---\nname: demo\ndescription: x\n---\nshould not run\n```");
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/capsule-create demo does the thing", &mut agent, &store, "s1", dir.path(),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(
            out.notices,
            vec!["capsule demo already exists (see /capsules)".to_string()]
        );
        // no model turn means no user/assistant messages were appended
        assert_eq!(agent.messages.len(), 1);
    }

    #[test]
    fn capsule_create_rejects_missing_arguments() {
        let mut capsules = test_capsules();
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/capsule-create", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(
            out.notices,
            vec!["usage: /capsule-create <name> <what it should do>".to_string()]
        );
    }

    #[test]
    fn capsule_create_rejects_reserved_name() {
        let mut capsules = test_capsules();
        let mut agent = test_agent(None);
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/capsule-create load does the thing", &mut agent, &store, "s1", Path::new("/repo"),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert_eq!(
            out.notices,
            vec!["load is a reserved command name, choose another".to_string()]
        );
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --bin quecto-agent capsule_create`
Expected: PASS for `capsule_create_rejects_existing_name_without_calling_the_model`, `capsule_create_rejects_missing_arguments`, `capsule_create_rejects_reserved_name`

- [ ] **Step 6: Write the success-path integration test**

Add to the same test module:

```rust
    #[test]
    fn capsule_create_drafts_writes_and_loads_a_new_capsule() {
        let dir = tempfile::tempdir().unwrap();
        let mut capsules = capsules_from(dir.path());
        let mut agent = fake_agent(
            "Sure, here's the capsule:\n```\n---\nname: demo\ndescription: demo capsule\n\
             ---\nFollow the demo workflow.\n```\n",
        );
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        let exit = handle_chat_command(
            "/capsule-create demo draft a demo workflow", &mut agent, &store, "s1", dir.path(),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!exit);
        assert!(capsules.is_active("demo"));
        let written = std::fs::read_to_string(
            dir.path().join(".quecto").join("capsules").join("demo").join("CAPSULE.md"),
        )
        .unwrap();
        assert!(written.contains("name: demo"));
        assert!(written.contains("Follow the demo workflow."));
        let prompt = agent.messages[0].text();
        assert!(prompt.contains("## Capsule: demo"));
        assert!(prompt.contains("Follow the demo workflow."));
        assert!(out
            .notices
            .iter()
            .any(|n| n.starts_with("created and loaded capsule demo at")));
    }

    #[test]
    fn capsule_create_reconciles_mismatched_model_provided_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut capsules = capsules_from(dir.path());
        let mut agent = fake_agent(
            "```\n---\nname: wrong-name\ndescription: demo capsule\n---\nbody\n```",
        );
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/capsule-create demo do the thing", &mut agent, &store, "s1", dir.path(),
            "test-model", &mut capsules, &mut out,
        );

        assert!(capsules.is_active("demo"));
        assert!(!capsules.is_active("wrong-name"));
    }

    #[test]
    fn capsule_create_reports_error_when_model_output_has_no_fence() {
        let dir = tempfile::tempdir().unwrap();
        let mut capsules = capsules_from(dir.path());
        let mut agent = fake_agent("sorry, I won't wrap this in a code block");
        let store: Option<Store> = None;
        let mut out = TestRenderer::default();

        handle_chat_command(
            "/capsule-create demo do the thing", &mut agent, &store, "s1", dir.path(),
            "test-model", &mut capsules, &mut out,
        );

        assert!(!capsules.is_active("demo"));
        assert!(out
            .notices
            .iter()
            .any(|n| n == "capsule draft failed: no fenced code block found in model output"));
        assert!(!dir.path().join(".quecto").join("capsules").join("demo").exists());
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p quecto-agent --bin quecto-agent capsule_create`
Expected: PASS for all 6 `capsule_create_*` tests

- [ ] **Step 8: Run the full workspace test suite and build**

Run: `cargo test --workspace && cargo build --workspace`
Expected: all tests pass, zero build warnings

- [ ] **Step 9: Commit**

```bash
git add quecto-agent/src/main.rs
git commit -m "feat(capsule): wire /capsule-create into the REPL"
```

---

### Task 5: README documentation

**Files:**
- Modify: `README.md` (wherever the existing `/capsules`/`/load`/`/unload` commands are documented — search for `/capsules` first)

**Interfaces:**
- Consumes: nothing (documentation only).
- Produces: nothing (documentation only).

- [ ] **Step 1: Find the existing capsules documentation**

Run: `grep -n "/capsules\|/load <name>\|CAPSULE.md" README.md`

- [ ] **Step 2: Add `/capsule-create` to the documented command list**

Next to wherever `/capsules`, `/load <name>`, `/unload <name>` are documented, add a line documenting `/capsule-create <name> <what it should do>` — draft and load a new capsule via the agent, written to `<cwd>/.quecto/capsules/<name>/CAPSULE.md`. Match the existing formatting/tone of that section exactly (read the surrounding lines before editing).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document /capsule-create command"
```
