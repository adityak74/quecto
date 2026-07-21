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

pub fn run_suite(
    manifest_path: &Path,
    tasks_dir: &Path,
    db_path: &Path,
    agent_binary: &Path,
) -> anyhow::Result<()> {
    let manifest = crate::manifest::load_manifest(manifest_path)?;
    let conn = init_db(db_path)?;

    let contracts: Vec<_> = manifest
        .contracts
        .critical
        .iter()
        .map(|id| {
            crate::contracts::load_contract(
                &Path::new(&manifest.contracts.suite_dir).join(format!("{id}.yaml")),
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap_or_default();

    let mut runtimes = vec![manifest.reference.clone()];
    runtimes.extend(manifest.candidates.clone());

    for entry in fs::read_dir(tasks_dir)? {
        let task_dir = entry?.path();
        if !task_dir.is_dir() {
            continue;
        }
        let task_id = task_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let prompt = fs::read_to_string(task_dir.join("prompt.md"))?;

        let backup_dir = tasks_dir.join(format!(".{task_id}.snapshot-backup"));
        crate::snapshot::snapshot_copy(&task_dir, &backup_dir)?;

        for runtime in &runtimes {
            for repetition in 0..manifest.experiment.repetitions {
                crate::snapshot::restore(&backup_dir, &task_dir)?;
                let snapshot_hash = crate::snapshot::snapshot_hash(&task_dir)?;
                let run_id = format!(
                    "{}-{}-{}-{}",
                    manifest.experiment.id, runtime.id, task_id, repetition
                );
                let trace_path = task_dir.join(format!(".trace-{run_id}.jsonl"));

                let status = std::process::Command::new(agent_binary)
                    .current_dir(&task_dir)
                    .arg("--yes")
                    .arg(&prompt)
                    .env("QUECTO_TRACE_FILE", &trace_path)
                    .env("QUECTO_EXPERIMENT_ID", &manifest.experiment.id)
                    .env("QUECTO_TASK_ID", &task_id)
                    .env("QUECTO_RUNTIME_ID", &runtime.id)
                    .env("QUECTO_RUN_ID", &run_id)
                    .env("QUECTO_REPETITION", repetition.to_string())
                    .env("QUECTO_SNAPSHOT_HASH", &snapshot_hash)
                    .env("QUECTO_REASONING_MODE", &runtime.reasoning_mode)
                    .status()?;

                let events = crate::contracts::load_trace(&trace_path).unwrap_or_default();
                for contract in &contracts {
                    let outcome = crate::contracts::evaluate_contract(contract, &events);
                    let (outcome_str, violated) = match &outcome {
                        crate::contracts::ContractOutcome::Pass => ("pass".to_string(), String::new()),
                        crate::contracts::ContractOutcome::Fail { violated } => {
                            ("fail".to_string(), violated.join(","))
                        }
                    };
                    conn.execute(
                        "INSERT INTO contract_results (run_id, contract_id, outcome, violated_predicates) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![run_id, contract.id, outcome_str, violated],
                    )?;
                }

                conn.execute(
                    "INSERT INTO runs (task_id, suite, passed, experiment_id, runtime_id, run_id, repetition) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        task_id,
                        "pilot",
                        status.success(),
                        manifest.experiment.id,
                        runtime.id,
                        run_id,
                        repetition
                    ],
                )?;
            }
        }
        fs::remove_dir_all(&backup_dir)?;
    }
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

    #[test]
    fn run_suite_executes_reference_and_candidate_per_repetition() {
        let root = tempdir().unwrap();
        let tasks_dir = root.path().join("tasks");
        let task_dir = tasks_dir.join("tb_fake");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join("prompt.md"), "do the thing").unwrap();

        // A fake agent binary: writes one trace event per invocation and exits 0.
        let fake_agent = root.path().join("fake_agent.sh");
        fs::write(
            &fake_agent,
            "#!/bin/sh\necho '{\"event_type\":\"run.start\",\"seq\":0}' >> \"$QUECTO_TRACE_FILE\"\necho '{\"event_type\":\"run.end\",\"seq\":1}' >> \"$QUECTO_TRACE_FILE\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agent).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agent, perms).unwrap();
        }

        let manifest_path = root.path().join("manifest.yaml");
        fs::write(
            &manifest_path,
            "schema_version: quecto.compat/v1\nexperiment:\n  id: test-exp\n  repetitions: 2\nreference:\n  id: reference-high\n  reasoning_mode: high\ncandidates:\n  - id: candidate-low\n    reasoning_mode: low\ncontracts:\n  suite_dir: NOT_USED\n  critical: []\n",
        )
        .unwrap();

        let db_path = root.path().join("telemetry.db");
        run_suite(&manifest_path, &tasks_dir, &db_path, &fake_agent).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        // 1 task * 2 runtimes (reference + 1 candidate) * 2 repetitions = 4 runs.
        assert_eq!(count, 4);
    }
}
