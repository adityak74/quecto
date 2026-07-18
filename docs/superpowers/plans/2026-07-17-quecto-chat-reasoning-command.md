# QuECTO Chat Reasoning Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/reasoning` chat command that shows and updates the current chat-session reasoning mode, persists that session default in storage, and restores it on `resume` without changing one-shot run behavior.

**Architecture:** Extend the chat command parser and REPL dispatcher with a session-scoped reasoning command, store the session-selected mode in the `sessions` header row, and thread that value into the configured model wrapper used by chat and resume. Keep provider semantics in the model layer and keep this feature confined to chat/resume flows.

**Tech Stack:** Rust 2021; existing `clap`, `serde`, `serde_json`, `rusqlite`, `crossterm`; existing `ConfiguredHttpModel`, `Store`, and chat REPL infrastructure; no new dependencies.

## Global Constraints

- `/reasoning` with no argument must print the current session reasoning mode.
- `/reasoning <mode>` must update the default reasoning mode for future turns in the current chat session.
- `/reasoning off` must clear the session default.
- The selected reasoning mode must persist across `resume`.
- One-shot runs and non-chat workflows must remain unchanged.
- This milestone does not include a broader chat settings framework.
- This milestone does not include per-message temporary overrides such as `/reasoning once high`.
- This milestone does not include mutation of flavor files or environment variables from the REPL.
- This milestone does not include reasoning-mode schedules or policy engines.
- This milestone does not include changing reasoning mode via plain-language prompts instead of explicit commands.

---

## File Structure

- Modify: `quecto-agent/src/chat.rs`
  Responsibility: parse `/reasoning` query and set forms into explicit command variants.
- Modify: `quecto-agent/src/main.rs`
  Responsibility: wire the chat command, display notices, update the running session reasoning default, persist it, and restore it in `resume`.
- Modify: `quecto-agent/src/model.rs`
  Responsibility: expose mutable/default reasoning-mode access on the configured model wrapper used by chat and resume.
- Modify: `quecto-agent/src/agent.rs`
  Responsibility: provide a narrow way for chat control flow to inspect and update the running model’s session reasoning default without rebuilding the agent.
- Modify: `quecto-agent/src/session.rs`
  Responsibility: persist session-level reasoning mode in the `sessions` table and restore it during resume.
- Modify: `README.md`
  Responsibility: document `/reasoning`, its accepted values, and resume persistence semantics.

### Task 1: Add `/reasoning` Command Parsing

**Files:**
- Modify: `quecto-agent/src/chat.rs`
- Test: `quecto-agent/src/chat.rs`

**Interfaces:**
- Consumes: existing `ChatCommand` parser contract
- Produces: `ChatCommand::Reasoning`
- Produces: `ReasoningCommand::{Show, Set(String)}`

- [ ] **Step 1: Write the failing parser tests**

```rust
#[test]
fn reasoning_without_argument_parses_as_show() {
    assert_eq!(
        parse_command("/reasoning"),
        ChatCommand::Reasoning(ReasoningCommand::Show)
    );
}

#[test]
fn reasoning_with_value_parses_as_set() {
    assert_eq!(
        parse_command("/reasoning high"),
        ChatCommand::Reasoning(ReasoningCommand::Set("high".to_string()))
    );
}

#[test]
fn reasoning_rejects_extra_arguments() {
    assert_eq!(
        parse_command("/reasoning high extra"),
        ChatCommand::Unknown("reasoning".to_string())
    );
}
```

- [ ] **Step 2: Run the focused parser tests and verify they fail**

Run: `cargo test -p quecto-agent reasoning_without_argument_parses_as_show reasoning_with_value_parses_as_set reasoning_rejects_extra_arguments`
Expected: FAIL with missing `ReasoningCommand` or missing `/reasoning` parser support.

- [ ] **Step 3: Implement explicit reasoning command parsing**

```rust
#[derive(Debug, PartialEq)]
pub enum ReasoningCommand {
    Show,
    Set(String),
}

#[derive(Debug, PartialEq)]
pub enum ChatCommand {
    Help,
    Model,
    Context,
    Diff,
    Status,
    Undo,
    Approve,
    Deny,
    Clear,
    Exit,
    Tools,
    Reasoning(ReasoningCommand),
    Say(String),
    Unknown(String),
}

pub fn parse_command(line: &str) -> ChatCommand {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ChatCommand::Say(trimmed.to_string());
    };
    let mut parts = rest.split_whitespace();
    let name = parts.next().unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
        "reasoning" => match (parts.next(), parts.next()) {
            (None, None) => ChatCommand::Reasoning(ReasoningCommand::Show),
            (Some(value), None) => ChatCommand::Reasoning(ReasoningCommand::Set(value.to_string())),
            _ => ChatCommand::Unknown("reasoning".to_string()),
        },
        // existing command cases unchanged
        other => ChatCommand::Unknown(other.to_string()),
    }
}
```

- [ ] **Step 4: Run the focused parser tests and verify they pass**

Run: `cargo test -p quecto-agent reasoning_without_argument_parses_as_show reasoning_with_value_parses_as_set reasoning_rejects_extra_arguments`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/chat.rs
git commit -m "feat(chat): parse reasoning command variants"
```

### Task 2: Persist Session Reasoning Mode in the Session Store

**Files:**
- Modify: `quecto-agent/src/session.rs`
- Test: `quecto-agent/src/session.rs`

**Interfaces:**
- Consumes: existing `sessions` table and additive migration style
- Produces: `session_reasoning_mode TEXT` column on `sessions`
- Produces: `Store::set_session_reasoning_mode(&self, id: &str, mode: Option<ReasoningMode>)`
- Produces: `Store::session_reasoning_mode(&self, id: &str) -> Result<Option<ReasoningMode>, BoxErr>`
- Produces: `Store::create_session_with_reasoning_mode(&self, id: &str, task: &str, repo: &str, model: &str, reasoning_mode: Option<ReasoningMode>) -> Result<(), BoxErr>`

- [ ] **Step 1: Write the failing persistence tests**

```rust
#[test]
fn session_reasoning_mode_round_trips() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session_with_reasoning_mode("s1", "chat", "/repo", "m", Some(crate::reasoning::ReasoningMode::High))
        .unwrap();
    assert_eq!(
        store.session_reasoning_mode("s1").unwrap(),
        Some(crate::reasoning::ReasoningMode::High)
    );
}

#[test]
fn session_reasoning_mode_can_be_cleared() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session_with_reasoning_mode("s1", "chat", "/repo", "m", Some(crate::reasoning::ReasoningMode::Low))
        .unwrap();
    store.set_session_reasoning_mode("s1", None).unwrap();
    assert_eq!(store.session_reasoning_mode("s1").unwrap(), None);
}
```

- [ ] **Step 2: Run the focused persistence tests and verify they fail**

Run: `cargo test -p quecto-agent session_reasoning_mode_round_trips session_reasoning_mode_can_be_cleared`
Expected: FAIL with missing column or missing store methods.

- [ ] **Step 3: Implement additive session-header persistence**

```rust
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    task TEXT NOT NULL,
    repo TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    session_reasoning_mode TEXT,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);";

fn migrate_session_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare("PRAGMA table_info(sessions)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut has_reasoning_mode = false;
    for column in columns {
        if column? == "session_reasoning_mode" {
            has_reasoning_mode = true;
            break;
        }
    }
    if !has_reasoning_mode {
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN session_reasoning_mode TEXT",
            [],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn create_session_with_reasoning_mode(
        &self,
        id: &str,
        task: &str,
        repo: &str,
        model: &str,
        reasoning_mode: Option<crate::reasoning::ReasoningMode>,
    ) -> Result<(), BoxErr> {
        let t = now();
        self.conn.execute(
            "INSERT INTO sessions (id, task, repo, model, status, session_reasoning_mode, created, updated) \
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?6)",
            (id, task, repo, model, reasoning_mode.map(|m| m.effort_str()), t),
        )?;
        Ok(())
    }

    pub fn set_session_reasoning_mode(
        &self,
        id: &str,
        mode: Option<crate::reasoning::ReasoningMode>,
    ) -> Result<(), BoxErr> {
        self.conn.execute(
            "UPDATE sessions SET session_reasoning_mode = ?2, updated = ?3 WHERE id = ?1",
            (id, mode.map(|m| m.effort_str()), now()),
        )?;
        Ok(())
    }

    pub fn session_reasoning_mode(
        &self,
        id: &str,
    ) -> Result<Option<crate::reasoning::ReasoningMode>, BoxErr> {
        let raw: Option<String> = self.conn.query_row(
            "SELECT session_reasoning_mode FROM sessions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        raw.map(|value| value.parse()).transpose()
    }
}
```

- [ ] **Step 4: Run the focused persistence tests and the package store tests**

Run: `cargo test -p quecto-agent session_reasoning_mode_round_trips session_reasoning_mode_can_be_cleared`
Expected: PASS

Run: `cargo test -p quecto-agent session::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/session.rs
git commit -m "feat(session): persist chat reasoning mode in session headers"
```

### Task 3: Add Mutable Session Reasoning Default to the Running Agent

**Files:**
- Modify: `quecto-agent/src/model.rs`
- Modify: `quecto-agent/src/agent.rs`
- Test: `quecto-agent/src/model.rs`
- Test: `quecto-agent/src/agent.rs`

**Interfaces:**
- Consumes: `ConfiguredHttpModel`, `ReasoningMode`
- Produces: `ConfiguredHttpModel::session_reasoning_mode(&self) -> Option<ReasoningMode>`
- Produces: `ConfiguredHttpModel::set_session_reasoning_mode(&mut self, mode: Option<ReasoningMode>)`
- Produces: `Agent::session_reasoning_mode(&self) -> Option<ReasoningMode>`
- Produces: `Agent::set_session_reasoning_mode(&mut self, mode: Option<ReasoningMode>) -> Result<(), BoxErr>`

- [ ] **Step 1: Write the failing runtime tests**

```rust
#[test]
fn configured_model_session_reasoning_mode_is_mutable() {
    let mut model = HttpModel {
        url: "http://example.test/v1/chat/completions".into(),
        api_key: None,
        model: "test-model".into(),
    }
    .with_default_reasoning_mode(Some(crate::reasoning::ReasoningMode::Low));
    assert_eq!(model.session_reasoning_mode(), Some(crate::reasoning::ReasoningMode::Low));
    model.set_session_reasoning_mode(Some(crate::reasoning::ReasoningMode::High));
    assert_eq!(model.session_reasoning_mode(), Some(crate::reasoning::ReasoningMode::High));
}
```

- [ ] **Step 2: Run the focused runtime tests and verify they fail**

Run: `cargo test -p quecto-agent configured_model_session_reasoning_mode_is_mutable`
Expected: FAIL with missing getter/setter support.

- [ ] **Step 3: Implement mutable session reasoning state on the configured model and agent**

```rust
impl ConfiguredHttpModel {
    pub fn session_reasoning_mode(&self) -> Option<crate::reasoning::ReasoningMode> {
        self.default_reasoning_mode
    }

    pub fn set_session_reasoning_mode(
        &mut self,
        mode: Option<crate::reasoning::ReasoningMode>,
    ) {
        self.default_reasoning_mode = mode;
    }
}

impl Agent {
    pub fn session_reasoning_mode(&self) -> Option<crate::reasoning::ReasoningMode> {
        self.model
            .as_any()
            .downcast_ref::<crate::model::ConfiguredHttpModel>()
            .and_then(|model| model.session_reasoning_mode())
    }

    pub fn set_session_reasoning_mode(
        &mut self,
        mode: Option<crate::reasoning::ReasoningMode>,
    ) -> Result<(), BoxErr> {
        self.model
            .as_any_mut()
            .downcast_mut::<crate::model::ConfiguredHttpModel>()
            .ok_or("reasoning mode updates are only supported for configured chat models")?
            .set_session_reasoning_mode(mode);
        Ok(())
    }
}
```

- [ ] **Step 4: Run the focused runtime tests and the package model/agent tests**

Run: `cargo test -p quecto-agent configured_model_session_reasoning_mode_is_mutable`
Expected: PASS

Run: `cargo test -p quecto-agent model::tests agent::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/model.rs quecto-agent/src/agent.rs
git commit -m "feat(agent): support mutable chat session reasoning defaults"
```

### Task 4: Wire `/reasoning` Through Chat and Resume

**Files:**
- Modify: `quecto-agent/src/main.rs`
- Modify: `quecto-agent/src/chat.rs`
- Modify: `quecto-agent/src/session.rs`
- Test: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: `ChatCommand::Reasoning`, `Store::session_reasoning_mode`, `Agent::set_session_reasoning_mode`
- Produces: `/reasoning` chat notices
- Produces: chat session creation seeded with stored/default reasoning mode
- Produces: `resume` restoring persisted reasoning mode before future turns

- [ ] **Step 1: Write the failing chat/resume behavior tests**

```rust
#[derive(Default)]
struct TestRenderer {
    notices: Vec<String>,
}

impl Renderer for TestRenderer {
    fn tool(&mut self, _name: &str, _summary: &str) {}
    fn verify(&mut self, _command: &str, _passed: bool) {}
    fn notice(&mut self, text: &str) {
        self.notices.push(text.to_string());
    }
    fn assistant(&mut self, _text: &str) {}
}

fn test_agent(mode: Option<quecto_agent::ReasoningMode>) -> Agent {
    let model = HttpModel {
        url: "http://example.test/v1/chat/completions".into(),
        api_key: None,
        model: "test-model".into(),
    }
    .with_default_reasoning_mode(mode);
    Agent::new(
        Box::new(model),
        "system".to_string(),
        4,
        std::env::current_dir().unwrap(),
        quecto_agent::cancel_token(),
        ApprovalMode::NonInteractive,
    )
}

#[test]
fn reasoning_query_reports_off_when_unset() {
    let mut agent = test_agent(None);
    let store = Some(Store::open_in_memory().unwrap());
    store
        .as_ref()
        .unwrap()
        .create_session("s1", "chat", "/repo", "test-model")
        .unwrap();
    let mut out = TestRenderer::default();

    let exit = handle_chat_command(
        "/reasoning",
        &mut agent,
        &store,
        "s1",
        std::path::Path::new("/repo"),
        "test-model",
        &mut out,
    );

    assert!(!exit);
    assert_eq!(out.notices, vec!["reasoning: off".to_string()]);
}

#[test]
fn reasoning_set_updates_agent_and_store() {
    let mut agent = test_agent(None);
    let store = Some(Store::open_in_memory().unwrap());
    store
        .as_ref()
        .unwrap()
        .create_session_with_reasoning_mode("s1", "chat", "/repo", "test-model", None)
        .unwrap();
    let mut out = TestRenderer::default();

    let exit = handle_chat_command(
        "/reasoning high",
        &mut agent,
        &store,
        "s1",
        std::path::Path::new("/repo"),
        "test-model",
        &mut out,
    );

    assert!(!exit);
    assert_eq!(
        agent.session_reasoning_mode(),
        Some(quecto_agent::ReasoningMode::High)
    );
    assert_eq!(
        store.as_ref().unwrap().session_reasoning_mode("s1").unwrap(),
        Some(quecto_agent::ReasoningMode::High)
    );
    assert_eq!(out.notices, vec!["reasoning set to high".to_string()]);
}

#[test]
fn resume_prefers_persisted_session_reasoning_mode() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_session_with_reasoning_mode(
            "s1",
            "chat",
            "/repo",
            "test-model",
            Some(quecto_agent::ReasoningMode::High),
        )
        .unwrap();
    assert_eq!(
        store.session_reasoning_mode("s1").unwrap(),
        Some(quecto_agent::ReasoningMode::High)
    );

    let resumed_mode = store.session_reasoning_mode("s1").unwrap();
    let model = HttpModel {
        url: "http://example.test/v1/chat/completions".into(),
        api_key: None,
        model: "test-model".into(),
    }
    .try_with_env_reasoning_mode(resumed_mode)
    .unwrap();

    assert_eq!(
        model.session_reasoning_mode(),
        Some(quecto_agent::ReasoningMode::High)
    );
}
```

- [ ] **Step 2: Run the focused chat/resume tests and verify they fail**

Run: `cargo test -p quecto-agent reasoning_query_reports_off_when_unset reasoning_set_updates_agent_and_store resume_prefers_persisted_session_reasoning_mode`
Expected: FAIL with missing `/reasoning` handling or missing session restoration.

- [ ] **Step 3: Implement chat command handling, persisted updates, and resume restoration**

```rust
const HELP: &str = "\
/commands            list available tools (same as /tools)
/exit, /quit, /q     leave chat
/help, /h, /?        show this help
/model               show the active model
/context             show transcript size
/diff                summarize this session's file changes
/status              show session id and status
/undo                revert the last recorded file change
/approve             auto-approve edits and commands this session
/deny                deny edits and commands this session
/clear               forget the conversation (keep system prompt)
/reasoning           show the active session reasoning mode
/reasoning <mode>    set reasoning mode for future turns in this session";

match parse_command(line) {
    ChatCommand::Reasoning(ReasoningCommand::Show) => {
        let mode = agent.session_reasoning_mode();
        out.notice(&format!("reasoning: {}", mode.map(|m| m.effort_str()).unwrap_or("off")));
    }
    ChatCommand::Reasoning(ReasoningCommand::Set(raw)) => {
        let mode = if raw.eq_ignore_ascii_case("off") {
            None
        } else {
            Some(raw.parse::<quecto_agent::ReasoningMode>().map_err(|_| {
                format!("unknown reasoning mode: {raw}")
            })?)
        };
        agent.set_session_reasoning_mode(mode)?;
        if let Some(store) = store {
            store.set_session_reasoning_mode(session_id, mode)?;
        }
        out.notice(match mode {
            Some(mode) => format!("reasoning set to {}", mode.effort_str()),
            None => "reasoning turned off".to_string(),
        });
    }
    // existing command handling unchanged
}

// chat session creation
let seeded_reasoning_mode = merged.reasoning_mode;
store.create_session_with_reasoning_mode(
    &session_id,
    "chat",
    &cwd.display().to_string(),
    "",
    seeded_reasoning_mode,
)?;

// resume
let persisted_reasoning_mode = store.session_reasoning_mode(id)?;
let model = HttpModel {
    url: join_url(&base_url, "chat/completions"),
    api_key: std::env::var("QUECTO_API_KEY")
        .ok()
        .filter(|value| !value.is_empty()),
    model: model_name.clone(),
}
    .try_with_env_reasoning_mode(persisted_reasoning_mode.or(merged.reasoning_mode))?;
```

- [ ] **Step 4: Run the focused chat/resume tests and the package test suite**

Run: `cargo test -p quecto-agent reasoning_query_reports_off_when_unset reasoning_set_updates_agent_and_store resume_prefers_persisted_session_reasoning_mode`
Expected: PASS

Run: `cargo test -p quecto-agent`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add quecto-agent/src/main.rs quecto-agent/src/chat.rs quecto-agent/src/session.rs
git commit -m "feat(chat): add persistent reasoning command for chat sessions"
```

### Task 5: Document the Command and Guard Non-Chat Isolation

**Files:**
- Modify: `README.md`
- Modify: `quecto-agent/src/main.rs`
- Test: `quecto-agent/src/main.rs`

**Interfaces:**
- Consumes: final `/reasoning` behavior
- Produces: operator docs for query/set/off/resume behavior
- Produces: regression coverage that one-shot non-chat runs remain unchanged

- [ ] **Step 1: Write the failing non-chat isolation regression test**

```rust
#[test]
fn chat_reasoning_updates_can_be_cleared_with_off() {
    let mut agent = test_agent(Some(quecto_agent::ReasoningMode::Medium));
    let store = Some(Store::open_in_memory().unwrap());
    store
        .as_ref()
        .unwrap()
        .create_session_with_reasoning_mode(
            "s1",
            "chat",
            "/repo",
            "test-model",
            Some(quecto_agent::ReasoningMode::Medium),
        )
        .unwrap();
    let mut out = TestRenderer::default();

    handle_chat_command(
        "/reasoning off",
        &mut agent,
        &store,
        "s1",
        std::path::Path::new("/repo"),
        "test-model",
        &mut out,
    );

    assert_eq!(agent.session_reasoning_mode(), None);
    assert_eq!(store.as_ref().unwrap().session_reasoning_mode("s1").unwrap(), None);
    assert_eq!(out.notices, vec!["reasoning turned off".to_string()]);
}

#[test]
fn one_shot_run_does_not_depend_on_session_reasoning_state() {
    let _lock = quecto_agent::model::tests::ENV_LOCK.lock().unwrap();
    let _env = quecto_agent::model::tests::EnvGuard::set(&[
        ("QUECTO_REASONING_MODE", None),
        ("QUECTO_BASE_URL", Some("http://localhost:1234/v1")),
        ("QUECTO_MODEL", Some("reasoning-model")),
    ]);
    let store = Store::open_in_memory().unwrap();
    store
        .create_session_with_reasoning_mode(
            "chat-session",
            "chat",
            "/repo",
            "reasoning-model",
            Some(quecto_agent::ReasoningMode::High),
        )
        .unwrap();

    let model = HttpModel::from_env().try_with_env_reasoning_mode(None).unwrap();

    assert_eq!(model.session_reasoning_mode(), None);
}
```

- [ ] **Step 2: Run the focused regression test and verify it fails**

Run: `cargo test -p quecto-agent one_shot_run_does_not_depend_on_session_reasoning_state`
Expected: FAIL until the final refactor makes the non-chat boundary explicit.

- [ ] **Step 3: Update docs and add the non-chat guard**

```md
<!-- README.md -->
/reasoning
/reasoning high
/reasoning off

The command changes the reasoning default for future turns in the current chat
session and persists across `quecto-agent resume <id>`. It does not mutate
environment variables, flavor files, or one-shot runs.
```

```rust
const HELP: &str = "\
/commands            list available tools (same as /tools)
/exit, /quit, /q     leave chat
/help, /h, /?        show this help
/model               show the active model
/context             show transcript size
/diff                summarize this session's file changes
/status              show session id and status
/undo                revert the last recorded file change
/approve             auto-approve edits and commands this session
/deny                deny edits and commands this session
/clear               forget the conversation (keep system prompt)
/reasoning           show the active session reasoning mode
/reasoning <mode>    set reasoning mode for future turns in this session";

#[test]
fn one_shot_run_does_not_depend_on_session_reasoning_state() {
    let _lock = quecto_agent::model::tests::ENV_LOCK.lock().unwrap();
    let _env = quecto_agent::model::tests::EnvGuard::set(&[
        ("QUECTO_REASONING_MODE", None),
        ("QUECTO_BASE_URL", Some("http://localhost:1234/v1")),
        ("QUECTO_MODEL", Some("reasoning-model")),
    ]);

    let configured = HttpModel::from_env()
        .try_with_env_reasoning_mode(None)
        .unwrap();

    assert_eq!(configured.session_reasoning_mode(), None);
}
```

- [ ] **Step 4: Run the focused regression test and the package test suite**

Run: `cargo test -p quecto-agent one_shot_run_does_not_depend_on_session_reasoning_state`
Expected: PASS

Run: `cargo test -p quecto-agent`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add README.md quecto-agent/src/main.rs
git commit -m "docs(chat): document persistent reasoning command"
```

## Self-Review

- Spec coverage: the plan covers `/reasoning` parsing, session persistence, mutable chat runtime state, resume restoration, output copy, and non-chat isolation.
- Placeholder scan: no `TODO`, `TBD`, or deferred “implement later” instructions remain.
- Type consistency: all tasks use the same names for `ReasoningCommand`, `session_reasoning_mode`, `set_session_reasoning_mode`, `ConfiguredFlavor`, and the persisted `session_reasoning_mode` field.
