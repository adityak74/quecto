use crate::model::{Message, ToolCall};
use crate::tools::FileChange;
use crate::BoxErr;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    tool_call_id TEXT
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

impl Store {
    fn init(conn: Connection) -> Result<Store, BoxErr> {
        conn.execute_batch(SCHEMA)?;
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
        if let Ok(p) = std::env::var("QUECTO_STATE_DB") {
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
        base.join("quecto").join("sessions.db")
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
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO messages (session_id, seq, role, content, tool_calls, tool_call_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id,
                seq,
                &m.role,
                &m.content,
                calls_to_json(&m.tool_calls),
                &m.tool_call_id,
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

    pub fn load_messages(&self, id: &str) -> Result<Vec<Message>, BoxErr> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, tool_calls, tool_call_id FROM messages \
             WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([id], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            let tool_calls: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            Ok(Message {
                role,
                content,
                tool_calls: calls_from_json(tool_calls),
                tool_call_id,
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
    use serde_json::json;

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
}
