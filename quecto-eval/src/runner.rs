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
    for (col, ty) in [
        ("experiment_id", "TEXT"),
        ("runtime_id", "TEXT"),
        ("run_id", "TEXT"),
        ("repetition", "INTEGER"),
    ] {
        ensure_column(&conn, "runs", col, ty)?;
    }
    conn.execute(
        "CREATE TABLE IF NOT EXISTS contract_results (
            id INTEGER PRIMARY KEY,
            run_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            outcome TEXT NOT NULL,
            violated_predicates TEXT
        )",
        (),
    )?;
    Ok(conn)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, ty: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
    }
    Ok(())
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

    #[test]
    fn init_db_adds_pairing_columns_and_contract_results_table() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("telemetry.db");
        let conn = init_db(&db_path).unwrap();

        conn.execute(
            "INSERT INTO runs (task_id, suite, passed, experiment_id, runtime_id, run_id, repetition) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["tb_01", "pilot", true, "exp-1", "reference", "exp-1-reference-tb_01-0", 0],
        ).unwrap();

        conn.execute(
            "INSERT INTO contract_results (run_id, contract_id, outcome, violated_predicates) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["exp-1-reference-tb_01-0", "verify_after_final_change", "pass", ""],
        ).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM contract_results", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Calling init_db again on the same file must not fail (idempotent migration).
        let conn2 = init_db(&db_path).unwrap();
        let count2: i64 = conn2
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count2, 1);
    }

}
