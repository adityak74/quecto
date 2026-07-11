mod common;

use common::{mock, mock_capture};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_quecto-agent")
}

#[test]
fn oneshot_prints_model_answer() {
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
    let (base, request) = mock_capture(
        200,
        "application/json",
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
    );
    let out = Command::new(bin())
        .args(["--yes", "do", "it"])
        .env("QUECTO_BASE_URL", &base)
        .env("QUECTO_MODEL", "m")
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
fn yes_without_task_is_usage_error() {
    let out = Command::new(bin()).arg("--yes").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
