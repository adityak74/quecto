# quecto-agent M7b — Trust-on-First-Use and `new` Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unlock a project flavor's command-bearing fields (`[verify]`) and approval loosening only after an explicit, content-hash-remembered trust decision, and add `quecto-agent new <name>` to scaffold a manifest starter.

**Architecture:** A project flavor's raw text is SHA-256 hashed. A file-backed `TrustStore` remembers approved hashes. On a run, if the project flavor `wants_privilege` (declares `[verify]` commands or loosens approval) and its hash is not yet trusted, the agent prints exactly what it would do and requires an explicit `allow` (TTY prompt; non-interactive denies; `--yes` auto-allows). Until trusted, only the project flavor's safe fields (persona, tool restrictions, model/base_url/max_steps) apply — the M7a behavior. `new <name>` writes a commented `flavor.toml` starter.

**Tech Stack:** Rust 2021; `sha2 = "0.10"` for hashing; existing `toml`, `serde`, `clap`. `main` stays a plain `fn main()`.

## Global Constraints

- Build on completed M7a APIs exactly as present: `Flavor` (+ `merge`, `verify_commands`, `approval: ApprovalSection` with `preset: Option<String>` and `overrides: BTreeMap<String,String>`), `resolve_scoped(home, cwd, name)`, `layer_paths(home, cwd, name)`, `Scope::{User, Project}`, `Policy`/`Preset`, `Verifier::new`; the `main.rs` helpers `build_policy(flag, &Flavor)`, `attach_verifier(agent, no_verify, &Flavor)`, `persona`, `resolve_flavor`, `Overrides`.
- One new dependency only: `sha2 = "0.10"`.
- **Safety is the point:** a project flavor's `[verify]` and approval-loosening apply ONLY when its content hash is trusted. An unchanged, already-approved flavor loads silently; any content change re-gates. Non-interactive (no TTY) denies; `--yes`/`auto_approve` allows and records trust. Tightening approval (to `deny`/`ask`) and safe fields never require trust.
- The trust file path is `$QUECTO_TRUST_FILE` when set, else `$XDG_STATE_HOME/quecto/trust` else `$HOME/.local/state/quecto/trust`. Tests MUST set `QUECTO_TRUST_FILE` to a temp path; never touch the real state dir.
- User-scope flavors remain fully trusted (the user wrote them); only project-scope gates.
- Preserve all M7a behavior when no project flavor exists or it has only safe fields.
- Run repository shell commands through `rtk` per `AGENTS.md`. Stage/commit only files named by each task. `fmt`/`clippy -D warnings`/`git diff --check` must pass at the end.

---

## File Structure

- `quecto-agent/Cargo.toml` — add `sha2`.
- `quecto-agent/src/flavor.rs` — `content_hash`, `project_raw`, `Flavor::wants_privilege`, `Flavor::privilege_summary`.
- `quecto-agent/src/trust.rs` — `TrustStore` (file-backed hash set).
- `quecto-agent/src/lib.rs` — declare/export new items.
- `quecto-agent/src/main.rs` — trust gate before building policy/verifier; `New { name }` subcommand + `scaffold`.
- `quecto-agent/tests/cli.rs` — trust-gate and scaffold integration tests.

---

### Task 1: Flavor hashing, raw project text, and privilege detection

**Files:**
- Modify: `quecto-agent/Cargo.toml`
- Modify: `quecto-agent/src/flavor.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces:
  - `content_hash(text: &str) -> String` — lowercase hex SHA-256.
  - `project_raw(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Option<String>` — concatenated raw text of existing project-scope layer files (in layer order), or `None` if none exist.
  - `Flavor::wants_privilege(&self) -> bool` — true if it declares `[verify]` commands or loosens approval (preset `editor`/`full`, or any override to `allow`).
  - `Flavor::privilege_summary(&self) -> Vec<String>` — human-readable lines describing the privileged requests.

- [ ] **Step 1: Add the dependency**

Add to `quecto-agent/Cargo.toml` under `[dependencies]`:

```toml
sha2 = "0.10"
```

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module in `flavor.rs`:

```rust
#[test]
fn content_hash_is_stable_and_sensitive() {
    let a = content_hash("model = \"x\"");
    let b = content_hash("model = \"x\"");
    let c = content_hash("model = \"y\"");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.len(), 64);
    assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn project_raw_concatenates_existing_project_files() {
    use std::fs;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".quecto")).unwrap();
    fs::write(repo.path().join(".quecto/flavor.toml"), "model = \"p\"").unwrap();
    let raw = project_raw(home.path(), repo.path(), None).unwrap();
    assert!(raw.contains("model = \"p\""));
    // No project files → None.
    let empty = tempfile::tempdir().unwrap();
    assert!(project_raw(home.path(), empty.path(), None).is_none());
}

#[test]
fn wants_privilege_true_for_verify_or_loosening() {
    let verify = Flavor::parse("[verify]\ntest = \"cargo test\"").unwrap();
    assert!(verify.wants_privilege());
    let full = Flavor::parse("[approval]\npreset = \"full\"").unwrap();
    assert!(full.wants_privilege());
    let loosen = Flavor::parse("[approval]\nrun_command = \"allow\"").unwrap();
    assert!(loosen.wants_privilege());
}

#[test]
fn wants_privilege_false_for_safe_or_tightening_fields() {
    let safe = Flavor::parse("model = \"m\"\nsystem_prompt = \"hi\"\n[tools]\nenabled=[\"read_file\"]").unwrap();
    assert!(!safe.wants_privilege());
    let readonly = Flavor::parse("[approval]\npreset = \"read-only\"\nrun_command = \"deny\"").unwrap();
    assert!(!readonly.wants_privilege());
}

#[test]
fn privilege_summary_lists_commands_and_loosening() {
    let f = Flavor::parse("[approval]\npreset=\"full\"\n[verify]\ntest=\"cargo test\"\nlint=\"cargo clippy\"").unwrap();
    let lines = f.privilege_summary().join("\n");
    assert!(lines.contains("cargo test"));
    assert!(lines.contains("cargo clippy"));
    assert!(lines.to_lowercase().contains("approval"));
}
```

- [ ] **Step 3: Implement**

Add to `flavor.rs`:

```rust
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of the given text.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Concatenate the raw text of existing project-scope layer files (in layer
/// order). Returns `None` when no project flavor file exists.
pub fn project_raw(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    for (scope, path) in layer_paths(home, cwd, flavor_name) {
        if scope != Scope::Project {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn approval_loosens(approval: &ApprovalSection) -> bool {
    let preset_loosens = approval
        .preset
        .as_deref()
        .and_then(crate::policy::Preset::parse)
        .is_some_and(|p| !matches!(p, crate::policy::Preset::ReadOnly));
    let override_allows = approval
        .overrides
        .values()
        .any(|v| v.trim().eq_ignore_ascii_case("allow"));
    preset_loosens || override_allows
}

impl Flavor {
    /// True if this flavor declares shell verification commands or loosens
    /// approval — the fields a project flavor may apply only once trusted.
    pub fn wants_privilege(&self) -> bool {
        !self.verify_commands().is_empty() || approval_loosens(&self.approval)
    }

    /// Human-readable lines describing what a trust prompt would grant.
    pub fn privilege_summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let commands = self.verify_commands();
        if !commands.is_empty() {
            lines.push(format!("run commands: {}", commands.join(", ")));
        }
        if approval_loosens(&self.approval) {
            let preset = self.approval.preset.as_deref().unwrap_or("(overrides)");
            lines.push(format!("loosen approval: preset {preset}"));
        }
        lines
    }
}
```

- [ ] **Step 4: Run, export, commit**

Run: `rtk cargo test -p quecto-agent --lib flavor`
Expected: PASS.

Export from `lib.rs` (extend the flavor re-export): add `content_hash, project_raw`.

```bash
rtk git add quecto-agent/Cargo.toml quecto-agent/src/flavor.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add flavor hashing and privilege detection"
```

---

### Task 2: File-backed trust store

**Files:**
- Create: `quecto-agent/src/trust.rs`
- Modify: `quecto-agent/src/lib.rs`

**Interfaces:**
- Produces:
  - `TrustStore` with `open() -> TrustStore` (reads `$QUECTO_TRUST_FILE`/XDG/HOME path), `open_at(path: PathBuf) -> TrustStore`, `is_trusted(&self, hash: &str) -> bool`, `trust(&mut self, hash: &str)` (appends + persists), `default_path() -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

Create `quecto-agent/src/trust.rs`:

```rust
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Remembers approved project-flavor content hashes, one lowercase-hex hash per
/// line in a small state file. Best-effort: I/O errors degrade to "not trusted"
/// and are never fatal.
pub struct TrustStore {
    path: PathBuf,
    hashes: BTreeSet<String>,
}

impl TrustStore {
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("QUECTO_TRUST_FILE") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        let base = std::env::var("XDG_STATE_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from(".quecto-state"));
        base.join("quecto").join("trust")
    }

    pub fn open() -> TrustStore {
        TrustStore::open_at(TrustStore::default_path())
    }

    pub fn open_at(path: PathBuf) -> TrustStore {
        let hashes = std::fs::read_to_string(&path)
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        TrustStore { path, hashes }
    }

    pub fn is_trusted(&self, hash: &str) -> bool {
        self.hashes.contains(hash)
    }

    pub fn trust(&mut self, hash: &str) {
        if !self.hashes.insert(hash.to_string()) {
            return;
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let body: String = self
            .hashes
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(&self.path, format!("{body}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust");
        let mut store = TrustStore::open_at(path.clone());
        assert!(!store.is_trusted("abc"));
        store.trust("abc");
        assert!(store.is_trusted("abc"));
        // Reopen: the hash is still there.
        let reopened = TrustStore::open_at(path);
        assert!(reopened.is_trusted("abc"));
        assert!(!reopened.is_trusted("def"));
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::open_at(dir.path().join("nope"));
        assert!(!store.is_trusted("x"));
    }
}
```

- [ ] **Step 2: Declare the module and run**

Add `mod trust;` to `lib.rs`.

Run: `rtk cargo test -p quecto-agent --lib trust`
Expected: PASS (2 tests).

- [ ] **Step 3: Export and commit**

Add to `lib.rs`:

```rust
pub use trust::TrustStore;
```

```bash
rtk git add quecto-agent/src/trust.rs quecto-agent/src/lib.rs
rtk git commit -m "feat(agent): add file-backed trust store"
```

---

### Task 3: Trust gate in the CLI

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Consumes: `quecto_agent::{content_hash, project_raw, TrustStore}`.
- Behavior: a helper `gated_flavor(user, project, home, cwd, name, auto_approve) -> Flavor` returns `user.merge(project)` when the project flavor is trusted (or needs no privilege), else `user`. `build_policy` and `attach_verifier` are called with the gated flavor so project `[verify]`/loosening apply only when trusted.

- [ ] **Step 1: Add integration tests**

Add to `quecto-agent/tests/cli.rs`:

```rust
#[test]
fn untrusted_project_verify_is_not_applied_noninteractively() {
    // A project flavor that would fail verification if applied. Non-interactive
    // (piped) runs must NOT apply it, so the run still succeeds.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".quecto")).unwrap();
    std::fs::write(
        dir.path().join(".quecto/flavor.toml"),
        "[verify]\ntest = \"exit 1\"",
    )
    .unwrap();
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"n.txt\",\"content\":\"x\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let out = Command::new(bin())
        .args(["--yes", "write n.txt"])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env("QUECTO_TRUST_FILE", dir.path().join("trust"))
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    // --yes trusts the project flavor, so verify IS applied and "exit 1" fails
    // the completion gate → non-Complete outcome, exit 1.
    assert!(!out.status.success(), "with --yes the failing verify should gate");

    // Now without --yes and with no TTY: verify is withheld, run completes.
    let base2 = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"n2.txt\",\"content\":\"x\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let out2 = Command::new(bin())
        .args(["write n2.txt"])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("QUECTO_BASE_URL", &base2)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s2.db"))
        .env("QUECTO_TRUST_FILE", dir.path().join("trust2"))
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out2.status.success(),
        "untrusted verify must be withheld non-interactively: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
}
```

Note: the `--yes` case relies on `auto_approve` trusting the project flavor and then the flavor `[verify]` gate running `exit 1` (a failure) → the agent loops and eventually exits non-zero (StepLimit). Keep `QUECTO_MAX_STEPS` unset (default 20) so the failing gate is reached.

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli untrusted_project_verify`

Expected: FAIL — the trust gate is not implemented, so project verify is either always or never applied.

- [ ] **Step 3: Implement the gate**

Add to `main.rs`:

```rust
use quecto_agent::{content_hash, project_raw, TrustStore};
use std::io::IsTerminal;

/// Return the flavor whose command-bearing/loosening fields may be applied:
/// `user ⊕ project` when the project flavor is trusted (or needs no privilege),
/// otherwise `user` alone. Prompts on a TTY; non-interactive denies; `--yes`
/// trusts and records.
fn gated_flavor(
    user: &Flavor,
    project: &Flavor,
    flavor_name: Option<&str>,
    auto_approve: bool,
) -> Flavor {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let Some(raw) = project_raw(&home, &cwd, flavor_name) else {
        return user.clone();
    };
    if !project.wants_privilege() {
        // Only safe project fields exist; nothing to gate for policy/verify.
        return user.clone();
    }
    let hash = content_hash(&raw);
    let mut store = TrustStore::open();
    if store.is_trusted(&hash) {
        return user.clone().merge(project.clone());
    }
    let trusted = if auto_approve {
        store.trust(&hash);
        true
    } else if prompt_trust(project) {
        store.trust(&hash);
        true
    } else {
        eprintln!("quecto-agent: project flavor not trusted; its verify/approval settings are ignored");
        false
    };
    if trusted {
        user.clone().merge(project.clone())
    } else {
        user.clone()
    }
}

/// Ask the human to approve a project flavor. Denies unless stdin is a TTY and
/// the answer is y/yes.
fn prompt_trust(project: &Flavor) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprintln!("⚠  ./.quecto/flavor.toml is new/changed and wants to:");
    for line in project.privilege_summary() {
        eprintln!("     • {line}");
    }
    eprint!("   Allow this project flavor? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
```

Then in `run`, `chat`, and `resume`, replace the policy/verifier construction inputs. Currently they compute `merged` and pass `&user_flavor` to `build_policy`/`attach_verifier`. Change to compute the gated flavor once and use it for BOTH policy and verifier, while `merged` stays for safe fields (model/persona/tools):

```rust
    let gated = gated_flavor(&user_flavor, &project_flavor, overrides.flavor.as_deref(), auto_approve);
```

- Replace `.with_policy(build_policy(overrides.approval.as_deref(), &user_flavor))` with `.with_policy(build_policy(overrides.approval.as_deref(), &gated))`.
- Replace `attach_verifier(agent, no_verify, &user_flavor)` with `attach_verifier(agent, no_verify, &gated)`.

For `resume`, `auto_approve` is the resume command's `--yes`. Pass it through (the `resume` fn already receives an approve flag; if not, thread `overrides` and compute `auto_approve` the same way as its `ApprovalMode`). Keep model/persona/tools bound to `merged` unchanged.

Note: `project_flavor` is already in scope in each command (from `resolve_flavor`). Ensure `auto_approve` is the same boolean used for `ApprovalMode::terminal`/chat approval.

- [ ] **Step 4: Run the integration test**

Run: `rtk cargo test -p quecto-agent --test cli untrusted_project_verify`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs
rtk git commit -m "feat(agent): gate project flavor privileges on trust"
```

---

### Task 4: `new <name>` manifest scaffold

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/tests/cli.rs`

**Interfaces:**
- Behavior: `quecto-agent new <name>` writes `./.quecto/flavors/<name>.toml` with a commented starter and prints the path. Refuses to overwrite an existing file (exit 1).

- [ ] **Step 1: Add integration tests**

Add to `quecto-agent/tests/cli.rs`:

```rust
#[test]
fn new_scaffolds_a_manifest_starter() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args(["new", "reviewer"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let path = dir.path().join(".quecto/flavors/reviewer.toml");
    assert!(path.exists(), "scaffold file should exist");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("name = \"reviewer\""));
    assert!(text.contains("[approval]"));

    // Refuses to overwrite.
    let again = Command::new(bin())
        .args(["new", "reviewer"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!again.status.success(), "second scaffold must not overwrite");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p quecto-agent --test cli new_scaffolds`
Expected: FAIL — `new` is not a subcommand.

- [ ] **Step 3: Implement**

Add the variant to `enum Command`:

```rust
    /// Scaffold a new flavor manifest at ./.quecto/flavors/<name>.toml.
    New { name: String },
```

Dispatch in `main`:

```rust
        Some(Command::New { name }) => scaffold(&name),
```

Add the function:

```rust
const SCAFFOLD_TEMPLATE: &str = r#"name = "{name}"

# All keys are optional; omitted keys inherit from the layer below.
# api_key is NEVER read from a manifest — set QUECTO_API_KEY in the environment.
# model         = "qwen3.6:35b"
# base_url      = "http://localhost:11434/v1"
# max_steps     = 30
# auto_verify   = true
# system_prompt = "You are a terse senior reviewer."

[tools]
# Allow-list over all built-in tools. Omit to enable all.
# enabled = ["read_file", "search_text", "list_files", "git_diff"]

[approval]
# preset = "read-only"   # read-only | editor | full
# run_command = "ask"    # allow | ask | deny

[verify]
# Commands run as a completion gate (project flavors require trust-on-first-use).
# test = "cargo test"
# lint = "cargo clippy -- -D warnings"
"#;

fn scaffold(name: &str) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let dir = cwd.join(".quecto").join("flavors");
    let path = dir.join(format!("{name}.toml"));
    if path.exists() {
        eprintln!("quecto-agent: {} already exists", path.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("quecto-agent: {e}");
        std::process::exit(1);
    }
    let body = SCAFFOLD_TEMPLATE.replace("{name}", name);
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("quecto-agent: {e}");
        std::process::exit(1);
    }
    println!("created {}", path.display());
}
```

- [ ] **Step 4: Run the test**

Run: `rtk cargo test -p quecto-agent --test cli new_scaffolds`
Expected: PASS.

- [ ] **Step 5: Full verification**

Run: `rtk cargo fmt --all -- --check` (fix with `rtk cargo fmt --all` if needed)
Run: `rtk cargo test --workspace -- --test-threads=1`
Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`
Run: `rtk git diff --check`
Expected: all PASS / no output.

- [ ] **Step 6: Commit**

```bash
rtk git add quecto-agent/src/main.rs quecto-agent/tests/cli.rs
rtk git commit -m "feat(agent): scaffold new flavor manifests"
```

---

## Final Acceptance Checklist

- [ ] A project flavor's `[verify]` / approval-loosening apply only when its content hash is trusted; safe fields always apply.
- [ ] Trust is remembered by SHA-256 content hash; an unchanged approved flavor loads silently; any change re-gates.
- [ ] Non-interactive (no TTY) denies the gate; `--yes`/`auto_approve` trusts and records.
- [ ] `$QUECTO_TRUST_FILE` overrides the trust path; tests never write the real state dir.
- [ ] `new <name>` writes `./.quecto/flavors/<name>.toml` and refuses to overwrite.
- [ ] All M7a behavior is preserved when no project flavor exists or it has only safe fields.
- [ ] `rtk cargo fmt --all -- --check`, `rtk cargo test --workspace -- --test-threads=1`, `rtk cargo clippy --workspace --all-targets -- -D warnings`, and `rtk git diff --check` all pass.

## Deferred Work

- `new <name> --crate` code-flavor scaffold; `init` install wizard.
- MCP (`[[mcp]]`) behind the `mcp` feature; `edit_format`/`tool_protocol` variants; `[render]` style options. Windows support.
