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

pub fn run_suite(suite: &str, db_path: &Path) -> anyhow::Result<()> {
    // 1. Workspace creation (dummy)
    let workspace_dir = Path::new("evals/workspaces").join(suite);
    fs::create_dir_all(&workspace_dir)?;
    
    // 2. Agent invocation (dummy)
    println!("Invoking agent in workspace {:?}", workspace_dir);
    
    // 3. DB writing (dummy)
    let conn = init_db(db_path)?;
    conn.execute(
        "INSERT INTO runs (task_id, suite, passed, tokens, turns, latency) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        ("dummy_task", suite, true, 100, 5, 500),
    )?;
    
    Ok(())
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

    #[test]
    fn test_run_suite() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        
        // Running suite should create workspace, insert a row
        run_suite("test_suite", &db_path).unwrap();
        
        // Verify DB was written
        let conn = Connection::open(&db_path).unwrap();
        let mut stmt = conn.prepare("SELECT suite, passed FROM runs").unwrap();
        let mut rows = stmt.query([]).unwrap();
        
        let row = rows.next().unwrap().unwrap();
        let suite: String = row.get(0).unwrap();
        let passed: bool = row.get(1).unwrap();
        
        assert_eq!(suite, "test_suite");
        assert!(passed);
    }
}
