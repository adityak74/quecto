use rusqlite::Connection;
use std::path::Path;
use std::fs;

pub fn init_db(db_path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let conn = Connection::open(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY,
            task_id TEXT,
            suite TEXT,
            passed BOOLEAN,
            tokens INTEGER,
            turns INTEGER,
            latency INTEGER
        )",
        (),
    )?;
    Ok(conn)
}

pub fn run_suite(_suite: &str, _db_path: &Path) -> anyhow::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_db_creates_dir_and_schema() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("subdir").join("test.db");
        
        // Should succeed and create the directory and schema
        let conn = init_db(&db_path).unwrap();
        assert!(db_path.exists());
        
        // Running it twice should also succeed (IF NOT EXISTS logic)
        let _conn2 = init_db(&db_path).unwrap();
    }


}
