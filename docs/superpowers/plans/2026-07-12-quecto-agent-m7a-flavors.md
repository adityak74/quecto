# quecto-agent M7a — Manifest Flavors: config, merge, persona, tool allow-list, configurable approval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a declarative `flavor.toml` (layered user→project, overridden by env then flags) configure the model, persona, enabled tools, approval preset, and verification commands — with no recompilation.

**Architecture:** A `Flavor` mirrors the manifest (all fields optional). `resolve(flavor_name, cwd)` discovers and key-by-key merges the layer chain into one `Flavor`. `main` turns that into effective values, applying `flag > env > flavor > built-in default`. The M4 `Policy` becomes preset-configurable (read-only / editor / full plus per-op overrides) while keeping its hardened denylist and never-auto-allow invariants. The registry gains an allow-list filter; the system prompt gains a `[persona]` section from the flavor.

**Tech Stack:** Rust 2021; `toml = "0.8"` for manifests; existing `serde`/`serde_json`, `clap`, `rusqlite`, `crossterm`. `main` stays a plain `fn main()`.

## Global Constraints

- Build on completed M6b APIs exactly as present: `Agent::new(model, system, max_steps, repo_root, cancel, approval)`, `register_builtins`, `register`, `with_verifier`, `with_recorder`, `with_renderer`; `Policy` (currently a unit struct with `decide(&ToolCall) -> Decision`); `Verifier::new(Vec<String>)`, `Verifier::from_env`; `builtin_tools()`, `Registry`, `Tool`; `ApprovalMode::{AutoApprove, NonInteractive, terminal}`.
- One new dependency only: `toml = "0.8"`. `sha2` and trust-on-first-use are **M7b** — do not add them here.
- **Safe-by-construction scope:** M7a loads flavor manifests but a project flavor's command-bearing fields (`[verify]`) and approval-loosening are applied ONLY for user-scope flavors (trusted) here. Project-scope `[verify]` and `[approval]` loosening are parsed but NOT applied until M7b adds the trust gate. Project-scope safe fields (persona, `[tools]` restrictions, model/base_url/max_steps) DO apply. (User-scope flavors are trusted because the user wrote them.)
- The built-in default policy stays exactly `read-only`: reads allow, edits/`run_command` ask, unknown deny, denylist always denies, `sudo`/`git push`/outside-repo never auto-allow — even under `full`. `Policy::default()` must behave identically to today so every existing policy test passes unchanged (update `let p = Policy;` to `let p = Policy::default();`).
- Precedence for scalar values: `CLI flag > env (QUECTO_*) > merged flavor > built-in default`. `--flavor <name>` selects the named layers.
- `api_key` is NEVER read from a manifest. It comes from env/flag only (unchanged: `HttpModel::from_env`).
- Preserve one-shot and chat UX. Omitting all flavor files must produce today's behavior byte-for-byte.
- Run repository shell commands through `rtk` per `AGENTS.md`. Stage/commit only files named by each task. `fmt`/`clippy -D warnings`/`git diff --check` must pass at the end.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `toml`.
- `quecto-agent/src/flavor.rs` — `Flavor`, sections, `parse`, `merge`, `resolve`, `Scope`, `layer_paths`.
- `quecto-agent/src/policy.rs` — make `Policy` preset-configurable (`Preset`, `from_preset`, `with_override`) while keeping the denylist.
- `quecto-agent/src/tools/mod.rs` — `builtin_tools_filtered(enabled: Option<&[String]>)`.
- `quecto-agent/src/lib.rs` — declare/export new items.
- `quecto-agent/src/main.rs` — resolve the flavor, apply precedence, build agent from it; add `--flavor/--model/--base-url/--max-steps/--approval` global flags.
- `quecto-agent/tests/cli.rs` — flavor integration tests.

---

### Task 1: Flavor manifest struct, parse, and merge

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Create: `quecto-agent/src/flavor.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces:
  - `Flavor { name, model, base_url, max_steps, auto_verify, auto_approve, system_prompt, system_prompt_file, tools: ToolsSection, approval: ApprovalSection, verify: VerifySection }` (all scalar fields `Option`, sections default-empty).
  - `ToolsSection { enabled: Option<Vec<String>> }`
  - `ApprovalSection { preset: Option<String>, overrides: BTreeMap<String, String> }`
  - `VerifySection { test: Option<String>, lint: Option<String>, build: Option<String>, required: Option<Vec<String>> }`
  - `Flavor::parse(&str) -> Result<Flavor, BoxErr>`
  - `Flavor::merge(self, over: Flavor) -> Flavor`

- [ ] **Step 1: Add the dependency**

Add to `quecto-agent/Cargo.toml` under `[dependencies]`:

```toml
toml = "0.8"
```

- [ ] **Step 2: Write the failing tests**

Create `quecto-agent/src/flavor.rs`:

```rust
use crate::BoxErr;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A flavor manifest. Every scalar field is optional so manifests can be merged
/// key-by-key; omitted keys inherit from the layer below.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flavor {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: Option<usize>,
    pub auto_verify: Option<bool>,
    pub auto_approve: Option<bool>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub approval: ApprovalSection,
    #[serde(default)]
    pub verify: VerifySection,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsSection {
    pub enabled: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApprovalSection {
    pub preset: Option<String>,
    /// Per-operation overrides such as `run_command = "allow"`.
    #[serde(flatten)]
    pub overrides: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySection {
    pub test: Option<String>,
    pub lint: Option<String>,
    pub build: Option<String>,
    pub required: Option<Vec<String>>,
}

fn or<T>(base: Option<T>, over: Option<T>) -> Option<T> {
    over.or(base)
}

impl Flavor {
    /// Parse one manifest.
    pub fn parse(text: &str) -> Result<Flavor, BoxErr> {
        Ok(toml::from_str(text)?)
    }

    /// Load a manifest from a file, if it exists. Missing file → `Ok(None)`.
    pub fn load(path: &Path) -> Result<Option<Flavor>, BoxErr> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Flavor::parse(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Box::new(e)),
        }
    }

    /// Merge `over` on top of `self`: `over`'s set keys win; unset keys inherit.
    pub fn merge(self, over: Flavor) -> Flavor {
        let mut overrides = self.approval.overrides;
        overrides.extend(over.approval.overrides);
        Flavor {
            name: or(self.name, over.name),
            model: or(self.model, over.model),
            base_url: or(self.base_url, over.base_url),
            max_steps: or(self.max_steps, over.max_steps),
            auto_verify: or(self.auto_verify, over.auto_verify),
            auto_approve: or(self.auto_approve, over.auto_approve),
            system_prompt: or(self.system_prompt, over.system_prompt),
            system_prompt_file: or(self.system_prompt_file, over.system_prompt_file),
            tools: ToolsSection {
                enabled: or(self.tools.enabled, over.tools.enabled),
            },
            approval: ApprovalSection {
                preset: or(self.approval.preset, over.approval.preset),
                overrides,
            },
            verify: VerifySection {
                test: or(self.verify.test, over.verify.test),
                lint: or(self.verify.lint, over.verify.lint),
                build: or(self.verify.build, over.verify.build),
                required: or(self.verify.required, over.verify.required),
            },
        }
    }

    /// The `[verify]` commands in a stable order (test, lint, build), skipping
    /// unset ones.
    pub fn verify_commands(&self) -> Vec<String> {
        [&self.verify.test, &self.verify.lint, &self.verify.build]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// Where a flavor layer comes from. Project layers are not fully trusted (M7b
/// gates their command-bearing fields).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    User,
    Project,
}

/// The ordered flavor files to merge, low → high precedence, tagged by scope.
/// `home` and `cwd` are injected for testability.
pub fn layer_paths(
    home: &Path,
    cwd: &Path,
    flavor_name: Option<&str>,
) -> Vec<(Scope, PathBuf)> {
    let user = home.join(".config").join("quecto");
    let project = cwd.join(".quecto");
    let mut paths = vec![
        (Scope::User, user.join("flavor.toml")),
        // named user flavor
    ];
    if let Some(name) = flavor_name {
        paths.push((Scope::User, user.join("flavors").join(format!("{name}.toml"))));
    }
    paths.push((Scope::Project, project.join("flavor.toml")));
    if let Some(name) = flavor_name {
        paths.push((
            Scope::Project,
            project.join("flavors").join(format!("{name}.toml")),
        ));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_known_fields_and_sections() {
        let f = Flavor::parse(
            r#"
name = "reviewer"
model = "m1"
max_steps = 30
auto_verify = true
system_prompt = "be terse"

[tools]
enabled = ["read_file", "search_text"]

[approval]
preset = "read-only"
run_command = "ask"

[verify]
test = "cargo test"
required = ["test"]
"#,
        )
        .unwrap();
        assert_eq!(f.name.as_deref(), Some("reviewer"));
        assert_eq!(f.model.as_deref(), Some("m1"));
        assert_eq!(f.max_steps, Some(30));
        assert_eq!(f.auto_verify, Some(true));
        assert_eq!(f.tools.enabled.as_deref().unwrap(), ["read_file", "search_text"]);
        assert_eq!(f.approval.preset.as_deref(), Some("read-only"));
        assert_eq!(f.approval.overrides.get("run_command").map(String::as_str), Some("ask"));
        assert_eq!(f.verify_commands(), vec!["cargo test".to_string()]);
    }

    #[test]
    fn merge_lets_higher_layer_win_and_inherits_unset() {
        let base = Flavor::parse(r#"model = "base-model"
max_steps = 10
system_prompt = "base""#).unwrap();
        let over = Flavor::parse(r#"model = "over-model""#).unwrap();
        let merged = base.merge(over);
        assert_eq!(merged.model.as_deref(), Some("over-model"));
        assert_eq!(merged.max_steps, Some(10));
        assert_eq!(merged.system_prompt.as_deref(), Some("base"));
    }

    #[test]
    fn merge_unions_approval_overrides() {
        let base = Flavor::parse("[approval]\nrun_command = \"ask\"").unwrap();
        let over = Flavor::parse("[approval]\nwrite_file = \"allow\"").unwrap();
        let merged = base.merge(over);
        assert_eq!(merged.approval.overrides.get("run_command").map(String::as_str), Some("ask"));
        assert_eq!(merged.approval.overrides.get("write_file").map(String::as_str), Some("allow"));
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        assert!(Flavor::parse("bogus_key = 1").is_err());
    }

    #[test]
    fn layer_paths_are_ordered_user_then_project() {
        let paths = layer_paths(Path::new("/home/u"), Path::new("/repo"), Some("rev"));
        let shown: Vec<String> = paths
            .iter()
            .map(|(_, p)| p.display().to_string())
            .collect();
        assert_eq!(
            shown,
            vec![
                "/home/u/.config/quecto/flavor.toml".to_string(),
                "/home/u/.config/quecto/flavors/rev.toml".to_string(),
                "/repo/.quecto/flavor.toml".to_string(),
                "/repo/.quecto/flavors/rev.toml".to_string(),
            ]
        );
        assert_eq!(paths[0].0, Scope::User);
        assert_eq!(paths[3].0, Scope::Project);
    }
}
```

- [ ] **Step 3: Declare the module and run**

Add `mod flavor;` to `lib.rs`. Ensure `serde` is available: add `serde = { version = "1", features = ["derive"] }` to `Cargo.toml` `[dependencies]` if not already present (the crate currently uses `serde_json` only, so add `serde`).

Run: `rtk cargo test -p quecto-agent --lib flavor`

Expected: PASS (5 tests).

- [ ] **Step 4: Export the public items**

Add to `lib.rs`:

```rust
pub use flavor::{layer_paths, ApprovalSection, Flavor, Scope, ToolsSection, VerifySection};
```

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/Cargo.toml quecto-agent/src/flavor.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add flavor manifest parsing and merge"
```

---

### Task 2: Flavor resolution over the layer chain

**Files:**
- Modify: `quecto-agent/src/flavor.rs`

**Interfaces:**
- Produces: `resolve(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Result<Flavor, BoxErr>` — the merged flavor from all existing layers (low→high). A missing file is skipped. A malformed file is an error.
- Produces: `resolve_scoped(...) -> Result<(Flavor, Flavor), BoxErr>` returning `(user_merged, project_merged)` so callers can apply project-scope gating in M7b. In M7a, callers merge them but only apply project command-bearing fields when trusted (here: never, since trust lands in M7b).

- [ ] **Step 1: Add tests**

Add to `flavor.rs` tests:

```rust
#[test]
fn resolve_merges_existing_layers_low_to_high() {
    use std::fs;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".config/quecto")).unwrap();
    fs::create_dir_all(repo.path().join(".quecto")).unwrap();
    fs::write(
        home.path().join(".config/quecto/flavor.toml"),
        "model = \"user-model\"\nmax_steps = 5",
    )
    .unwrap();
    fs::write(
        repo.path().join(".quecto/flavor.toml"),
        "model = \"project-model\"",
    )
    .unwrap();
    let f = resolve(home.path(), repo.path(), None).unwrap();
    // Project overrides user for model; user max_steps is inherited.
    assert_eq!(f.model.as_deref(), Some("project-model"));
    assert_eq!(f.max_steps, Some(5));
}

#[test]
fn resolve_returns_default_when_no_layers_exist() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let f = resolve(home.path(), repo.path(), None).unwrap();
    assert!(f.model.is_none());
    assert!(f.tools.enabled.is_none());
}

#[test]
fn resolve_scoped_separates_user_and_project() {
    use std::fs;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".config/quecto")).unwrap();
    fs::create_dir_all(repo.path().join(".quecto")).unwrap();
    fs::write(home.path().join(".config/quecto/flavor.toml"), "model = \"u\"").unwrap();
    fs::write(repo.path().join(".quecto/flavor.toml"), "[verify]\ntest = \"cargo test\"").unwrap();
    let (user, project) = resolve_scoped(home.path(), repo.path(), None).unwrap();
    assert_eq!(user.model.as_deref(), Some("u"));
    assert_eq!(project.verify.test.as_deref(), Some("cargo test"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib flavor::tests::resolve`

Expected: FAIL — `resolve`/`resolve_scoped` do not exist.

- [ ] **Step 3: Implement resolution**

Add to `flavor.rs`:

```rust
/// Merge every existing layer, low → high precedence, into one flavor.
pub fn resolve(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Result<Flavor, BoxErr> {
    let mut merged = Flavor::default();
    for (_scope, path) in layer_paths(home, cwd, flavor_name) {
        if let Some(layer) = Flavor::load(&path)? {
            merged = merged.merge(layer);
        }
    }
    Ok(merged)
}

/// Resolve user-scope and project-scope layers separately so a caller can apply
/// project command-bearing fields only when trusted (M7b).
pub fn resolve_scoped(
    home: &Path,
    cwd: &Path,
    flavor_name: Option<&str>,
) -> Result<(Flavor, Flavor), BoxErr> {
    let mut user = Flavor::default();
    let mut project = Flavor::default();
    for (scope, path) in layer_paths(home, cwd, flavor_name) {
        if let Some(layer) = Flavor::load(&path)? {
            match scope {
                Scope::User => user = user.merge(layer),
                Scope::Project => project = project.merge(layer),
            }
        }
    }
    Ok((user, project))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p quecto-agent --lib flavor`

Expected: PASS.

- [ ] **Step 5: Export and commit**

Add to `lib.rs` export line for flavor: include `resolve, resolve_scoped`:

```rust
pub use flavor::{
    layer_paths, resolve, resolve_scoped, ApprovalSection, Flavor, Scope, ToolsSection,
    VerifySection,
};
```

```bash
rtk git add quecto-agent/src/flavor.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): resolve flavor layer chain"
```

---

### Task 3: Preset-configurable approval policy

**Files:**
- Modify: `quecto-agent/src/policy.rs`

**Interfaces:**
- Produces:
  - `enum Preset { ReadOnly, Editor, Full }` with `Preset::parse(&str) -> Option<Preset>`.
  - `Policy` becomes a struct carrying `edit: Decision` and `run: Decision`; `Policy::default()` == read-only (edits `Ask`, run `Ask`).
  - `Policy::from_preset(Preset) -> Policy`.
  - `Policy::with_override(self, op: &str, decision: &str) -> Policy` (op ∈ {`write_file`,`apply_patch`,`edit`,`run_command`}; decision ∈ {`allow`,`ask`,`deny`}; unknown op/decision is ignored).
- Behavior unchanged for the default: reads allow, unknown deny, `run_command` denylist always denies regardless of preset (never-auto-allow invariants intact).

- [ ] **Step 1: Add preset tests**

Add to the `tests` module in `policy.rs` (keep all existing tests; update `let p = Policy;` to `let p = Policy::default();` throughout):

```rust
#[test]
fn editor_preset_allows_edits_but_still_asks_run() {
    let p = Policy::from_preset(Preset::Editor);
    assert!(matches!(p.decide(&call("write_file", json!({}))), Decision::Allow));
    assert!(matches!(p.decide(&call("apply_patch", json!({}))), Decision::Allow));
    assert!(matches!(
        p.decide(&call("run_command", json!({"command":"cargo test"}))),
        Decision::Ask
    ));
}

#[test]
fn full_preset_allows_run_but_denylist_still_wins() {
    let p = Policy::from_preset(Preset::Full);
    assert!(matches!(
        p.decide(&call("run_command", json!({"command":"cargo test"}))),
        Decision::Allow
    ));
    assert!(matches!(
        p.decide(&call("run_command", json!({"command":"sudo rm -rf /"}))),
        Decision::Deny(_)
    ));
    assert!(matches!(
        p.decide(&call("run_command", json!({"command":"git push origin main"}))),
        Decision::Deny(_)
    ));
}

#[test]
fn overrides_tighten_or_loosen_individual_operations() {
    let p = Policy::from_preset(Preset::ReadOnly).with_override("run_command", "allow");
    assert!(matches!(
        p.decide(&call("run_command", json!({"command":"cargo test"}))),
        Decision::Allow
    ));
    let p2 = Policy::from_preset(Preset::Editor).with_override("write_file", "deny");
    assert!(matches!(p2.decide(&call("write_file", json!({}))), Decision::Deny(_)));
}

#[test]
fn preset_parse_accepts_known_names() {
    assert!(matches!(Preset::parse("read-only"), Some(Preset::ReadOnly)));
    assert!(matches!(Preset::parse("editor"), Some(Preset::Editor)));
    assert!(matches!(Preset::parse("full"), Some(Preset::Full)));
    assert!(Preset::parse("bogus").is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib policy`

Expected: FAIL — `Preset`, `from_preset`, `with_override` missing (and the `Policy;` → `Policy::default()` edits are needed to compile).

- [ ] **Step 3: Refactor `Policy`**

Replace the top of `policy.rs` (the `Policy` struct and `impl`) with:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Preset {
    ReadOnly,
    Editor,
    Full,
}

impl Preset {
    pub fn parse(name: &str) -> Option<Preset> {
        match name.trim().to_ascii_lowercase().as_str() {
            "read-only" | "read_only" | "readonly" => Some(Preset::ReadOnly),
            "editor" => Some(Preset::Editor),
            "full" => Some(Preset::Full),
            _ => None,
        }
    }
}

/// Per-operation approval policy. Reads are always allowed and unknown tools are
/// always denied; the `run_command` denylist always denies regardless of preset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    edit: Decision,
    run: Decision,
}

impl Default for Policy {
    fn default() -> Self {
        Policy::from_preset(Preset::ReadOnly)
    }
}

fn parse_decision(word: &str) -> Option<Decision> {
    match word.trim().to_ascii_lowercase().as_str() {
        "allow" => Some(Decision::Allow),
        "ask" => Some(Decision::Ask),
        "deny" => Some(Decision::Deny("denied by flavor policy".to_string())),
        _ => None,
    }
}

impl Policy {
    pub fn from_preset(preset: Preset) -> Policy {
        match preset {
            Preset::ReadOnly => Policy {
                edit: Decision::Ask,
                run: Decision::Ask,
            },
            Preset::Editor => Policy {
                edit: Decision::Allow,
                run: Decision::Ask,
            },
            Preset::Full => Policy {
                edit: Decision::Allow,
                run: Decision::Allow,
            },
        }
    }

    /// Apply one `[approval]` override key. Unknown operations or decisions are
    /// ignored (a manifest typo cannot silently loosen policy).
    pub fn with_override(mut self, op: &str, decision: &str) -> Policy {
        let Some(decision) = parse_decision(decision) else {
            return self;
        };
        match op.trim().to_ascii_lowercase().as_str() {
            "write_file" | "apply_patch" | "edit" => self.edit = decision,
            "run_command" => self.run = decision,
            _ => {}
        }
        self
    }

    pub fn decide(&self, call: &ToolCall) -> Decision {
        match call.name.as_str() {
            "read_file" | "list_files" | "search_text" | "git_diff" | "git_status" => {
                Decision::Allow
            }
            "write_file" | "apply_patch" => self.edit.clone(),
            "run_command" => {
                let command = call
                    .arguments
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(reason) = deny_reason(command) {
                    Decision::Deny(reason)
                } else {
                    self.run.clone()
                }
            }
            _ => Decision::Deny(format!(
                "tool '{}' is not permitted by the built-in policy",
                call.name
            )),
        }
    }
}
```

Leave `deny_reason` and all its helpers unchanged.

- [ ] **Step 4: Update `agent.rs` construction**

In `agent.rs`, the field init `policy: Policy,` in `Agent::new` must become `policy: Policy::default(),`. Also add a builder so `main` can set a flavor policy:

```rust
    /// Replace the approval policy (default: read-only preset).
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }
```

(The `Policy` import in `agent.rs` already exists via `use crate::policy::{Decision, Policy};`.)

- [ ] **Step 5: Run and commit**

Run: `rtk cargo test -p quecto-agent --lib policy`
Run: `rtk cargo test -p quecto-agent --lib agent::tests`

Expected: PASS (all policy tests, including 4 new; all agent tests).

Export `Preset` from `lib.rs`:

```rust
pub use policy::{Decision, Policy, Preset};
```

```bash
rtk git add quecto-agent/src/policy.rs quecto-agent/src/agent.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): make approval policy preset-configurable"
```

---

### Task 4: Tool allow-list

**Files:**
- Modify: `quecto-agent/src/tools/mod.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces: `builtin_tools_filtered(enabled: Option<&[String]>) -> Vec<Box<dyn Tool>>` — all built-ins when `enabled` is `None`; otherwise only those whose `name()` is in `enabled`.

- [ ] **Step 1: Add tests**

Add to the `tests` module in `tools/mod.rs`:

```rust
#[test]
fn filtered_builtins_default_to_all() {
    let all = builtin_tools().len();
    let same = builtin_tools_filtered(None).len();
    assert_eq!(all, same);
}

#[test]
fn filtered_builtins_respect_allow_list() {
    let enabled = vec!["read_file".to_string(), "search_text".to_string()];
    let tools = builtin_tools_filtered(Some(&enabled));
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"search_text"));
    assert!(!names.contains(&"run_command"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --lib filtered_builtins`

Expected: FAIL — `builtin_tools_filtered` missing.

- [ ] **Step 3: Implement**

In `tools/mod.rs`, add next to `builtin_tools`:

```rust
/// Built-in tools filtered by an optional allow-list of tool names. `None`
/// enables all; `Some(list)` keeps only the named ones.
pub fn builtin_tools_filtered(enabled: Option<&[String]>) -> Vec<Box<dyn Tool>> {
    match enabled {
        None => builtin_tools(),
        Some(list) => builtin_tools()
            .into_iter()
            .filter(|t| list.iter().any(|n| n == t.name()))
            .collect(),
    }
}
```

Add an `Agent` builder in `agent.rs` to register a filtered set:

```rust
    /// Register the built-in tools filtered by an allow-list (`None` = all).
    pub fn register_builtins_filtered(mut self, enabled: Option<&[String]>) -> Self {
        for tool in crate::tools::builtin_tools_filtered(enabled) {
            self.registry.register(tool);
        }
        self
    }
```

- [ ] **Step 4: Run, export, commit**

Run: `rtk cargo test -p quecto-agent --lib`

Expected: PASS.

Export from `lib.rs` (add `builtin_tools_filtered` to the `tools::{...}` re-export list).

```bash
rtk git add quecto-agent/src/tools/mod.rs quecto-agent/src/agent.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add built-in tool allow-list"
```

---

### Task 5: CLI wiring — flavor resolution, precedence, and value flags

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Global flags: `--flavor <name>`, `--model <m>`, `--base-url <url>`, `--max-steps <n>`, `--approval <preset>`.
- Precedence: `flag > env (QUECTO_*) > flavor > default`.
- `run` and `chat` build the agent from the resolved flavor: persona prepended to the system prompt, `register_builtins_filtered` with `[tools] enabled`, `with_policy` from `[approval]`, and — for **user-scope** verify only — a flavor `Verifier` (project verify stays inert until M7b).

- [ ] **Step 1: Add integration tests**

Add to `quecto-agent/tests/cli.rs`. These exercise precedence and persona through the captured request body:

```rust
#[test]
fn flavor_model_is_used_when_env_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".quecto")).unwrap();
    std::fs::write(
        dir.path().join(".quecto/flavor.toml"),
        "system_prompt = \"PERSONA_MARKER\"",
    )
    .unwrap();
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["do", "it"])
        .current_dir(dir.path())
        .env("HOME", dir.path()) // isolate user-scope flavor discovery
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let body = request.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert!(body.contains("PERSONA_MARKER"), "persona should be in the system prompt: {body}");
}

#[test]
fn model_flag_overrides_env_and_flavor() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".quecto")).unwrap();
    std::fs::write(dir.path().join(".quecto/flavor.toml"), "model = \"flavor-model\"").unwrap();
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["--model", "flag-model", "do", "it"])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "env-model")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(out.status.success());
    let body = request.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert!(body.contains("flag-model"), "flag must win: {body}");
    assert!(!body.contains("flavor-model"));
    assert!(!body.contains("env-model"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli flavor_model_is_used model_flag_overrides`

Expected: FAIL — flags and flavor wiring do not exist; persona/model are not applied.

- [ ] **Step 3: Add global flags to the CLI**

In `main.rs`, extend `struct Cli` with global flags:

```rust
    #[arg(long, global = true)]
    flavor: Option<String>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    base_url: Option<String>,
    #[arg(long, global = true)]
    max_steps: Option<usize>,
    #[arg(long, global = true)]
    approval: Option<String>,
}
```

Thread a small `Overrides` struct built from the CLI into `run`, `chat`, and `resume`:

```rust
struct Overrides {
    flavor: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    max_steps: Option<usize>,
    approval: Option<String>,
}
```

Build it in `main` and pass to each command (extend their signatures to take `&Overrides`). For `undo`/`diff`, no overrides are needed.

- [ ] **Step 4: Add a flavor→config resolver in `main.rs`**

Add:

```rust
use quecto_agent::{resolve_scoped, Flavor, Policy, Preset, Verifier};

/// Resolve the effective flavor (user + project layers) for the current run.
fn resolve_flavor(overrides: &Overrides) -> (Flavor, Flavor) {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    match resolve_scoped(&home, &cwd, overrides.flavor.as_deref()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("quecto-agent: flavor error: {e}");
            std::process::exit(1);
        }
    }
}

/// Effective scalar with precedence flag > env > flavor > default.
fn pick(flag: Option<&str>, env: &str, flavor: Option<&str>, default: &str) -> String {
    flag.map(str::to_string)
        .or_else(|| std::env::var(env).ok().filter(|s| !s.is_empty()))
        .or_else(|| flavor.map(str::to_string))
        .unwrap_or_else(|| default.to_string())
}
```

Build the model with precedence and construct the agent. In `run` (and analogously `chat`), replace the model/system/agent construction with:

```rust
    let (user_flavor, project_flavor) = resolve_flavor(overrides);
    // Safe fields merge from both scopes; command-bearing/loosening from project
    // are withheld until M7b's trust gate, so only user-scope verify/approval apply.
    let merged = user_flavor.clone().merge(project_flavor.clone());

    let base_url = pick(
        overrides.base_url.as_deref(),
        "QUECTO_BASE_URL",
        merged.base_url.as_deref(),
        "http://localhost:11434/v1",
    );
    let model_name = pick(
        overrides.model.as_deref(),
        "QUECTO_MODEL",
        merged.model.as_deref(),
        "",
    );
    let api_key = std::env::var("QUECTO_API_KEY").ok().filter(|s| !s.is_empty());
    let model = HttpModel {
        url: join_url(&base_url, "chat/completions"),
        api_key,
        model: model_name,
    };
```

This constructs `HttpModel` directly (its fields `url`/`api_key`/`model` are public). To build the URL, re-export the core helper: add `pub use quecto::join_url;` to `lib.rs`, then import `join_url` in `main.rs` via `use quecto_agent::join_url;` and call `join_url(&base_url, "chat/completions")` (shown above as `quecto::join_url`; use the re-export `join_url`). Do NOT use `HttpModel::from_env()` in this path — it ignores flavor/flag values. `api_key` comes only from `QUECTO_API_KEY` (never a manifest).

System prompt persona: prepend the flavor persona as the `[persona]` section before repo rules and seed context. Replace `compose_system(cwd)` usage with:

```rust
fn compose_system_with_persona(cwd: &Path, persona: Option<&str>) -> String {
    let mut system = String::new();
    if let Some(p) = persona {
        if !p.trim().is_empty() {
            system.push_str("# Persona\n");
            system.push_str(p.trim());
            system.push_str("\n\n");
        }
    }
    system.push_str(&compose_system(cwd));
    system
}
```

Resolve persona text: `merged.system_prompt` or, if `system_prompt_file` is set, read that file relative to cwd. Precedence with `QUECTO_SYSTEM`/`--system` is preserved because `compose_system` already honors `QUECTO_SYSTEM` as the base and repo rules; the flavor persona is an additional labeled section prepended here.

Max steps precedence:

```rust
let steps = overrides
    .max_steps
    .or_else(|| std::env::var("QUECTO_MAX_STEPS").ok().and_then(|v| v.parse().ok()))
    .or(merged.max_steps)
    .unwrap_or(20);
```

Policy: build from `--approval` flag or merged `[approval] preset` (user-scope only for loosening in M7a — since project loosening is withheld, use `user_flavor.approval` for overrides and the preset from flag/user):

```rust
fn build_policy(flag: Option<&str>, user: &Flavor) -> Policy {
    let preset_name = flag
        .map(str::to_string)
        .or_else(|| user.approval.preset.clone());
    let mut policy = match preset_name.as_deref().and_then(Preset::parse) {
        Some(p) => Policy::from_preset(p),
        None => Policy::default(),
    };
    for (op, decision) in &user.approval.overrides {
        policy = policy.with_override(op, decision);
    }
    policy
}
```

Wire it: `.with_policy(build_policy(overrides.approval.as_deref(), &user_flavor))`.

Tools: `.register_builtins_filtered(merged.tools.enabled.as_deref())` instead of `register_builtins()`.

Verifier (user-scope only in M7a): if `!no_verify`, and user-scope has verify commands, attach `Verifier::new(user_flavor.verify_commands())`; else fall back to `Verifier::from_env()`.

- [ ] **Step 5: Run all tests, fmt, clippy, diff-check**

Run: `rtk cargo test -p quecto-agent`
Expected: PASS.

Run: `rtk cargo fmt --all -- --check` (fix with `rtk cargo fmt --all` if needed)
Run: `rtk cargo test --workspace -- --test-threads=1`
Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`
Run: `rtk git diff --check`
Expected: all PASS / no output.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): wire flavors and value flags into the CLI"
```

---

## Final Acceptance Checklist

- [ ] A `flavor.toml` sets model/persona/max_steps/tools/approval/verify; omitting all files reproduces today's behavior.
- [ ] Layers merge low→high (user default → user named → project default → project named); unset keys inherit.
- [ ] Precedence holds: `--model` > `QUECTO_MODEL` > flavor `model` > default; likewise base_url/max_steps.
- [ ] `Policy::default()` is byte-identical read-only; `editor`/`full` presets and per-op overrides work; the denylist and never-auto-allow invariants still hold under `full`.
- [ ] `[tools] enabled` restricts the registered built-ins; omitting it enables all.
- [ ] The flavor persona appears as a labeled section in the system prompt; `QUECTO_SYSTEM`/`--system` still apply.
- [ ] Project-scope `[verify]`/approval-loosening are parsed but NOT applied in M7a (they await M7b's trust gate); user-scope flavors apply fully.
- [ ] `api_key` is never read from a manifest.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work (M7b and beyond)

- **M7b:** `sha2` trust-on-first-use for project flavors (content-hash trust store; gate `[verify]` + approval loosening; TTY prompt / non-interactive deny / `--yes` allow); `new <name>` manifest scaffold and `new <name> --crate` code-flavor scaffold; `init` wizard.
- MCP (`[[mcp]]`) behind the `mcp` feature (`tokio` + `rmcp`); `edit_format`/`tool_protocol` variants; `[render]` style options. Windows support.
