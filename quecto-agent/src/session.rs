use crate::model::{Message, MessageMetadata, MessageRecord, ToolCall};
use crate::tools::FileChange;
use crate::BoxErr;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIGRATION_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    task TEXT NOT NULL,
    repo TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT,
    reasoning_content TEXT,
    requested_reasoning_mode TEXT,
    provider_reasoning_parameters TEXT,
    reasoning_parameters_sent INTEGER,
    reasoning_content_available INTEGER,
    actual_reasoning_tokens INTEGER
);
CREATE TABLE IF NOT EXISTS file_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    path TEXT NOT NULL,
    before TEXT,
    after TEXT NOT NULL
);";

/// A stored session's header row.
pub struct SessionRow {
    pub id: String,
    pub task: String,
    pub repo: String,
    pub model: String,
    pub status: String,
}

/// SQLite-backed session persistence.
pub struct Store {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A time-ordered, process-unique session id.
pub fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", nanos, std::process::id())
}

fn calls_to_json(calls: &[ToolCall]) -> Option<String> {
    if calls.is_empty() {
        return None;
    }
    let arr: Vec<Value> = calls
        .iter()
        .map(|c| json!({"id": c.id, "name": c.name, "arguments": c.arguments}))
        .collect();
    Some(Value::Array(arr).to_string())
}

fn calls_from_json(raw: Option<String>) -> Vec<ToolCall> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|v| ToolCall {
            id: v
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            arguments: v.get("arguments").cloned().unwrap_or(Value::Null),
        })
        .collect()
}

fn message_column_exists(conn: &Connection, column: &str) -> Result<bool, rusqlite::Error> {
    let mut statement = conn.prepare("PRAGMA table_info(messages)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_message_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    const COLUMNS: &[(&str, &str)] = &[
        ("reasoning_content", "TEXT"),
        ("requested_reasoning_mode", "TEXT"),
        ("provider_reasoning_parameters", "TEXT"),
        ("reasoning_parameters_sent", "INTEGER"),
        ("reasoning_content_available", "INTEGER"),
        ("actual_reasoning_tokens", "INTEGER"),
    ];

    for (column, sql_type) in COLUMNS {
        if !message_column_exists(conn, column)? {
            conn.execute(
                &format!("ALTER TABLE messages ADD COLUMN {column} {sql_type}"),
                [],
            )?;
        }
    }
    if message_column_exists(conn, "reasoning_mode_applied")? {
        conn.execute(
            "UPDATE messages SET reasoning_parameters_sent = reasoning_mode_applied \
             WHERE reasoning_parameters_sent IS NULL",
            [],
        )?;
    }
    Ok(())
}

impl Store {
    fn init(mut conn: Connection) -> Result<Store, BoxErr> {
        conn.busy_timeout(MIGRATION_BUSY_TIMEOUT)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        migrate_message_columns(&tx)?;
        tx.commit()?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Store, BoxErr> {
        Store::init(Connection::open_in_memory()?)
    }

    pub fn open_at(path: &Path) -> Result<Store, BoxErr> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Store::init(Connection::open(path)?)
    }

    pub fn default_path() -> PathBuf {
        crate::trust::state_path("QUECTO_STATE_DB", "sessions.db")
    }

    pub fn open_default() -> Result<Store, BoxErr> {
        Store::open_at(&Store::default_path())
    }

    pub fn create_session(
        &self,
        id: &str,
        task: &str,
        repo: &str,
        model: &str,
    ) -> Result<(), BoxErr> {
        let t = now();
        self.conn.execute(
            "INSERT INTO sessions (id, task, repo, model, status, created, updated) \
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
            (id, task, repo, model, t),
        )?;
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: &str) -> Result<(), BoxErr> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, updated = ?3 WHERE id = ?1",
            (id, status, now()),
        )?;
        Ok(())
    }

    pub fn record_message(&mut self, id: &str, seq: i64, m: &Message) -> Result<(), BoxErr> {
        self.record_message_with_metadata(id, seq, m, &MessageMetadata::default())
    }

    pub fn record_message_with_metadata(
        &mut self,
        id: &str,
        seq: i64,
        m: &Message,
        metadata: &MessageMetadata,
    ) -> Result<(), BoxErr> {
        let actual_reasoning_tokens = metadata
            .actual_reasoning_tokens
            .map(i64::try_from)
            .transpose()
            .map_err(|_| "actual reasoning token count exceeds SQLite INTEGER range")?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO messages (session_id, seq, role, content, tool_calls, tool_call_id, reasoning_content, requested_reasoning_mode, provider_reasoning_parameters, reasoning_parameters_sent, reasoning_content_available, actual_reasoning_tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            (
                id,
                seq,
                &m.role,
                &m.content,
                calls_to_json(&m.tool_calls),
                &m.tool_call_id,
                &m.reasoning_content,
                metadata
                    .requested_reasoning_mode
                    .map(|mode| mode.effort_str()),
                metadata
                    .provider_reasoning_parameters
                    .as_ref()
                    .map(Value::to_string),
                metadata.reasoning_parameters_sent,
                metadata.reasoning_content_available,
                actual_reasoning_tokens,
            ),
        )?;
        tx.execute(
            "UPDATE sessions SET updated = ?2 WHERE id = ?1",
            (id, now()),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn record_change(&self, id: &str, seq: i64, c: &FileChange) -> Result<(), BoxErr> {
        self.conn.execute(
            "INSERT INTO file_changes (session_id, seq, path, before, after) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (id, seq, &c.path, &c.before, &c.after),
        )?;
        Ok(())
    }

    pub fn message_count(&self, id: &str) -> Result<i64, BoxErr> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn change_count(&self, id: &str) -> Result<i64, BoxErr> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM file_changes WHERE session_id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn latest_session(&self) -> Result<Option<SessionRow>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task, repo, model, status FROM sessions \
             ORDER BY updated DESC, created DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SessionRow {
                id: row.get(0)?,
                task: row.get(1)?,
                repo: row.get(2)?,
                model: row.get(3)?,
                status: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn session_status(&self, id: &str) -> Result<Option<String>, BoxErr> {
        let mut stmt = self.conn.prepare("SELECT status FROM sessions WHERE id = ?1")?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            Ok(Some(status))
        } else {
            Ok(None)
        }
    }

    pub fn load_messages(&self, id: &str) -> Result<Vec<Message>, BoxErr> {
        Ok(self
            .load_message_records(id)?
            .into_iter()
            .map(|record| record.message)
            .collect())
    }

    pub fn load_message_records(&self, id: &str) -> Result<Vec<MessageRecord>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, tool_calls, tool_call_id, reasoning_content, requested_reasoning_mode, provider_reasoning_parameters, reasoning_parameters_sent, reasoning_content_available, actual_reasoning_tokens FROM messages \
             WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([id], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            let tool_calls: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            let reasoning_content: Option<String> = row.get(4)?;
            let requested_reasoning_mode: Option<String> = row.get(5)?;
            let provider_reasoning_parameters: Option<String> = row.get(6)?;
            let reasoning_parameters_sent: Option<bool> = row.get(7)?;
            let reasoning_content_available: Option<bool> = row.get(8)?;
            let actual_reasoning_tokens: Option<i64> = row.get(9)?;
            Ok(MessageRecord {
                message: Message {
                    role,
                    content,
                    tool_calls: calls_from_json(tool_calls),
                    tool_call_id,
                    reasoning_content,
                },
                metadata: MessageMetadata {
                    requested_reasoning_mode: requested_reasoning_mode
                        .and_then(|mode| mode.parse().ok()),
                    provider_reasoning_parameters: provider_reasoning_parameters
                        .and_then(|parameters| serde_json::from_str(&parameters).ok()),
                    reasoning_parameters_sent,
                    reasoning_content_available,
                    actual_reasoning_tokens: actual_reasoning_tokens
                        .and_then(|tokens| u64::try_from(tokens).ok()),
                },
            })
        })?;
        let mut out = Vec::new();
        for m in rows {
            out.push(m?);
        }
        Ok(out)
    }

    pub fn load_changes(&self, id: &str) -> Result<Vec<FileChange>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT path, before, after FROM file_changes \
             WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(FileChange {
                path: row.get(0)?,
                before: row.get(1)?,
                after: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for c in rows {
            out.push(c?);
        }
        Ok(out)
    }

    pub fn take_last_change(&self, id: &str) -> Result<Option<FileChange>, BoxErr> {
        let query_result: Result<(i64, String, Option<String>, String), rusqlite::Error> = self.conn.query_row(
            "SELECT id, path, before, after FROM file_changes \
             WHERE session_id = ?1 ORDER BY seq DESC, id DESC LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        );
        let (row_id, path, before, after) = match query_result {
            Ok(val) => val,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        self.conn
            .execute("DELETE FROM file_changes WHERE id = ?1", [row_id])?;
        Ok(Some(FileChange {
            path,
            before,
            after,
        }))
    }
}

/// A compact, git-free summary of the file changes recorded in a session.
pub fn render_change_summary(changes: &[FileChange]) -> String {
    if changes.is_empty() {
        return "no recorded changes".to_string();
    }
    let mut out = format!("{} file change(s)\n", changes.len());
    for c in changes {
        let now_lines = c.after.lines().count();
        match &c.before {
            None => out.push_str(&format!("  created   {}  ({} lines)\n", c.path, now_lines)),
            Some(before) => out.push_str(&format!(
                "  modified  {}  (was {} lines, now {} lines)\n",
                c.path,
                before.lines().count(),
                now_lines
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;
    use serde_json::json;

    const PRE_REASONING_MODE_SCHEMA: &str = "\
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    task TEXT NOT NULL,
    repo TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT,
    reasoning_content TEXT
);
CREATE TABLE file_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    path TEXT NOT NULL,
    before TEXT,
    after TEXT NOT NULL
);";

    fn create_pre_reasoning_mode_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(PRE_REASONING_MODE_SCHEMA).unwrap();
    }

    fn assistant_call() -> Message {
        Message::assistant_with_calls(
            "",
            vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }],
        )
    }

    #[test]
    fn messages_round_trip_with_tool_calls() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        store
            .record_message("s1", 0, &Message::system("sys"))
            .unwrap();
        store.record_message("s1", 1, &Message::user("hi")).unwrap();
        store.record_message("s1", 2, &assistant_call()).unwrap();
        store
            .record_message("s1", 3, &Message::tool_result("c1", "file body"))
            .unwrap();
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].role, "system");
        assert_eq!(loaded[2].tool_calls.len(), 1);
        assert_eq!(loaded[2].tool_calls[0].name, "read_file");
        assert_eq!(loaded[2].tool_calls[0].arguments, json!({"path": "a.rs"}));
        assert_eq!(loaded[3].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn opens_and_migrates_pre_reasoning_mode_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        create_pre_reasoning_mode_db(&path);

        let mut store = Store::open_at(&path).unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        let message = Message::assistant("response");
        let metadata = MessageMetadata {
            requested_reasoning_mode: Some(crate::reasoning::ReasoningMode::Low),
            provider_reasoning_parameters: Some(json!({"reasoning_effort": "low"})),
            reasoning_parameters_sent: Some(true),
            reasoning_content_available: Some(false),
            actual_reasoning_tokens: Some(7),
        };
        store
            .record_message_with_metadata("s1", 0, &message, &metadata)
            .unwrap();

        let loaded = store.load_message_records("s1").unwrap();
        assert_eq!(loaded[0].metadata.actual_reasoning_tokens, Some(7));
    }

    #[test]
    fn concurrent_opens_serialize_pre_reasoning_mode_migration() {
        const OPENERS: usize = 8;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        create_pre_reasoning_mode_db(&path);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(OPENERS));
        let handles: Vec<_> = (0..OPENERS)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open_at(&path).map(drop)
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let mut store = Store::open_at(&path).unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        let message = Message::assistant("response");
        let metadata = MessageMetadata {
            requested_reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
            ..MessageMetadata::default()
        };
        store
            .record_message_with_metadata("s1", 0, &message, &metadata)
            .unwrap();
        assert_eq!(
            store.load_message_records("s1").unwrap()[0]
                .metadata
                .requested_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::High)
        );
    }

    #[test]
    fn migration_propagates_non_duplicate_alter_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        create_pre_reasoning_mode_db(&path);
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

        assert!(Store::init(conn).is_err());
    }

    #[test]
    fn latest_session_picks_most_recent() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("a", "first", "/r", "m").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.create_session("b", "second", "/r", "m").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.set_status("b", "done").unwrap();
        assert_eq!(store.latest_session().unwrap().unwrap().id, "b");
    }

    #[test]
    fn session_status_retrieves_correct_status() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        assert_eq!(store.session_status("s1").unwrap(), Some("running".to_string()));
        store.set_status("s1", "done").unwrap();
        assert_eq!(store.session_status("s1").unwrap(), Some("done".to_string()));
        assert_eq!(store.session_status("nonexistent").unwrap(), None);
    }

    #[test]
    fn changes_persist_and_take_last_pops_in_reverse() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s1", "t", "/r", "m").unwrap();
        store
            .record_change(
                "s1",
                0,
                &FileChange {
                    path: "a".into(),
                    before: None,
                    after: "x".into(),
                },
            )
            .unwrap();
        store
            .record_change(
                "s1",
                1,
                &FileChange {
                    path: "b".into(),
                    before: Some("old".into()),
                    after: "new".into(),
                },
            )
            .unwrap();
        assert_eq!(store.change_count("s1").unwrap(), 2);
        let last = store.take_last_change("s1").unwrap().unwrap();
        assert_eq!(last.path, "b");
        assert_eq!(last.before.as_deref(), Some("old"));
        assert_eq!(store.change_count("s1").unwrap(), 1);
        let first = store.take_last_change("s1").unwrap().unwrap();
        assert_eq!(first.path, "a");
        assert!(store.take_last_change("s1").unwrap().is_none());
    }

    #[test]
    fn summary_labels_created_and_modified() {
        let changes = vec![
            FileChange {
                path: "new.rs".into(),
                before: None,
                after: "a\nb\n".into(),
            },
            FileChange {
                path: "old.rs".into(),
                before: Some("a\n".into()),
                after: "a\nb\nc\n".into(),
            },
        ];
        let s = render_change_summary(&changes);
        assert!(s.contains("created   new.rs"));
        assert!(s.contains("modified  old.rs"));
    }

    #[test]
    fn empty_summary_is_explicit() {
        assert_eq!(render_change_summary(&[]), "no recorded changes");
    }

    #[test]
    fn messages_round_trip_with_reasoning_content() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        let mut m = Message::assistant("response");
        m.reasoning_content = Some("thinking trace".to_string());
        store.record_message("s1", 0, &m).unwrap();
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].reasoning_content, Some("thinking trace".to_string()));
    }

    #[test]
    fn messages_round_trip_with_reasoning_metadata() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_session("s1", "task", "/repo", "m").unwrap();
        let m = Message::assistant("response");
        let metadata = MessageMetadata {
            requested_reasoning_mode: Some(crate::reasoning::ReasoningMode::Low),
            provider_reasoning_parameters: Some(json!({"reasoning_effort": "low"})),
            reasoning_parameters_sent: Some(true),
            reasoning_content_available: Some(false),
            actual_reasoning_tokens: Some(9),
        };
        store
            .record_message_with_metadata("s1", 0, &m, &metadata)
            .unwrap();
        let loaded = store.load_message_records("s1").unwrap();
        let loaded = &loaded[0].metadata;
        assert_eq!(
            loaded.requested_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::Low)
        );
        assert_eq!(loaded.actual_reasoning_tokens, Some(9));
        assert_eq!(loaded.reasoning_parameters_sent, Some(true));
        assert_eq!(loaded.reasoning_content_available, Some(false));
        assert_eq!(
            loaded.provider_reasoning_parameters,
            Some(json!({"reasoning_effort": "low"}))
        );
    }
}
