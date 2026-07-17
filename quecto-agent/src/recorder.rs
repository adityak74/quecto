use crate::agent::RunRecorder;
use crate::model::{Message, MessageMetadata};
use crate::session::Store;
use crate::tools::FileChange;

/// A `RunRecorder` that appends the transcript and file changes to a `Store`,
/// assigning monotonically increasing per-session sequence numbers. Persistence
/// errors are logged to stderr and never propagate into the run.
pub struct SqliteRecorder {
    store: Store,
    session_id: String,
    msg_seq: i64,
    change_seq: i64,
}

impl SqliteRecorder {
    pub fn new(store: Store, session_id: String, msg_seq: i64, change_seq: i64) -> Self {
        SqliteRecorder {
            store,
            session_id,
            msg_seq,
            change_seq,
        }
    }
}

impl RunRecorder for SqliteRecorder {
    fn message(&mut self, m: &Message) {
        if let Err(e) = self.store.record_message(&self.session_id, self.msg_seq, m) {
            eprintln!("quecto-agent: failed to persist message: {e}");
        }
        self.msg_seq += 1;
    }

    fn message_with_metadata(&mut self, m: &Message, metadata: &MessageMetadata) {
        if let Err(e) = self.store.record_message_with_metadata(
            &self.session_id,
            self.msg_seq,
            m,
            metadata,
        ) {
            eprintln!("quecto-agent: failed to persist message: {e}");
        }
        self.msg_seq += 1;
    }

    fn change(&mut self, c: &FileChange) {
        if let Err(e) = self
            .store
            .record_change(&self.session_id, self.change_seq, c)
        {
            eprintln!("quecto-agent: failed to persist change: {e}");
        }
        self.change_seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_appends_messages_and_changes_with_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = Store::open_at(&path).unwrap();
        store.create_session("s1", "t", "/r", "m").unwrap();

        let mut rec = SqliteRecorder::new(Store::open_at(&path).unwrap(), "s1".into(), 0, 0);
        rec.message(&Message::user("hi"));
        rec.message(&Message::assistant("there"));
        rec.change(&FileChange {
            path: "a".into(),
            before: None,
            after: "x".into(),
        });

        let verify = Store::open_at(&path).unwrap();
        assert_eq!(verify.message_count("s1").unwrap(), 2);
        assert_eq!(verify.change_count("s1").unwrap(), 1);
        let loaded = verify.load_messages("s1").unwrap();
        assert_eq!(loaded[0].content, "hi");
        assert_eq!(loaded[1].content, "there");
    }
}
