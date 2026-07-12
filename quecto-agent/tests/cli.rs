mod common;

use common::{mock, mock_capture, mock_script};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_quecto-agent")
}

#[test]
fn oneshot_prints_model_answer() {
    let dir = tempfile::tempdir().unwrap();
    let base = mock(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"42"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .arg("what")
        .arg("is")
        .arg("6x7")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");
}

#[test]
fn no_args_is_usage_error() {
    let out = Command::new(bin()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn yes_flag_is_removed_from_the_user_task() {
    // Run in a fresh directory so the seeded repository context (git diff of the
    // working tree) cannot vary the captured request body.
    let dir = tempfile::tempdir().unwrap();
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["--yes", "do", "it"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    let body = request
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body.contains("do it"));
    assert!(!body.contains("--yes"));
}

#[test]
fn no_verify_flag_is_removed_from_the_user_task() {
    let dir = tempfile::tempdir().unwrap();
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["--no-verify", "do", "it"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    let body = request
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body.contains("do it"));
    assert!(!body.contains("--no-verify"));
}

#[test]
fn one_shot_run_is_recorded_and_diff_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"note.txt\",\"content\":\"hello\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let run = Command::new(bin())
        .args(["--yes", "write", "note.txt"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", &db)
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "hello\n"
    );

    let diff = Command::new(bin())
        .arg("diff")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .output()
        .unwrap();
    assert!(diff.status.success());
    assert!(String::from_utf8_lossy(&diff.stdout).contains("note.txt"));
}

#[test]
fn undo_restores_prior_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    std::fs::write(dir.path().join("note.txt"), "old\n").unwrap();
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"note.txt\",\"content\":\"new\\n\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
    ]);
    let run = Command::new(bin())
        .args(["--yes", "overwrite note.txt"])
        .current_dir(dir.path())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STATE_DB", &db)
        .env_remove("QUECTO_API_KEY")
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "new\n"
    );

    let undo = Command::new(bin())
        .arg("undo")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .output()
        .unwrap();
    assert!(
        undo.status.success(),
        "undo failed: {}",
        String::from_utf8_lossy(&undo.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "old\n"
    );
}

#[test]
fn yes_without_task_is_usage_error() {
    let out = Command::new(bin()).arg("--yes").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn chat_help_and_exit_without_model() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(bin())
        .arg("chat")
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", dir.path().join("s.db"))
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_BASE_URL", "http://127.0.0.1:1") // unused: no plain-text turn
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/help\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/help"), "help listing expected: {stdout}");
}

#[test]
fn chat_runs_a_turn_and_records_it() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    let base = mock_script(vec![
        r#"{"choices":[{"message":{"content":"hello there"},"finish_reason":"stop"}]}"#,
    ]);
    let mut child = Command::new(bin())
        .args(["chat", "--yes"])
        .current_dir(dir.path())
        .env("QUECTO_STATE_DB", &db)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_BASE_URL", &base)
        .env_remove("QUECTO_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say hello\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello there"));
}
