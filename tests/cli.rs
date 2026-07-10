mod common;
use common::mock;
use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_quecto")
}

#[test]
fn oneshot_buffered_joins_args() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"hi there"}}]}"#);
    let out = Command::new(bin())
        .arg("say").arg("hi")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "0")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi there\n");
}

#[test]
fn oneshot_streaming_prints_deltas() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"str\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"eam\"}}]}\n\ndata: [DONE]\n\n";
    let base = mock(200, "text/event-stream", sse);
    let out = Command::new(bin())
        .arg("go")
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "1")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "stream\n");
}

#[test]
fn repl_answers_one_line_then_eof() {
    let base = mock(200, "application/json", r#"{"choices":[{"message":{"content":"reply"}}]}"#);
    let mut child = Command::new(bin())
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
        .env("QUECTO_STREAM", "0")
        .env_remove("QUECTO_API_KEY")
        .env_remove("QUECTO_SYSTEM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("reply"));
}

#[test]
fn init_prints_exports() {
    let mut child = Command::new(bin())
        .arg("--init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn().unwrap();
    child.stdin.take().unwrap()
        .write_all(b"http://localhost:11434/v1\n\nqwen\n\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("export QUECTO_BASE_URL=\"http://localhost:11434/v1\""));
    assert!(s.contains("export QUECTO_MODEL=\"qwen\""));
    assert!(!s.contains("QUECTO_API_KEY"));
}
