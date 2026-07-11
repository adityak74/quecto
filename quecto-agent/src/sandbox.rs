#[cfg(not(unix))]
compile_error!("quecto-agent M4 requires a Unix target");

use crate::tools::ToolError;
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub type CancelToken = Arc<AtomicBool>;

pub fn cancel_token() -> CancelToken {
    Arc::new(AtomicBool::new(false))
}

pub struct Sandbox {
    repo_root: PathBuf,
    cancel: CancelToken,
    timeout: Duration,
    output_cap: usize,
}

#[derive(Debug)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl CommandOutput {
    pub fn render(&self) -> String {
        format!(
            "exit_status: {}\ntimed_out: {}\ncancelled: {}\nstdout:\n{}\nstderr:\n{}",
            self.status
                .map(|n| n.to_string())
                .unwrap_or_else(|| "signal".into()),
            self.timed_out,
            self.cancelled,
            self.stdout,
            self.stderr
        )
    }
}

impl Sandbox {
    pub fn new(repo_root: PathBuf, cancel: CancelToken) -> Self {
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        Self {
            repo_root,
            cancel,
            timeout: Duration::from_secs(120),
            output_cap: 32 * 1024,
        }
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[cfg(test)]
    fn with_output_cap(mut self, cap: usize) -> Self {
        self.output_cap = cap;
        self
    }

    pub fn run(&self, command: &str) -> Result<CommandOutput, ToolError> {
        let secrets = secret_values();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::new(format!("spawn: {e}")))?;
        let pgid = child.id() as i32;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::new("stdout pipe unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::new("stderr pipe unavailable"))?;
        let out_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let err_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let started = Instant::now();
        let (status, timed_out, cancelled) = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| ToolError::new(format!("wait: {e}")))?
            {
                kill_process_group(pgid);
                break (status.code(), false, false);
            }
            let cancelled = self.cancel.load(Ordering::SeqCst);
            let timed_out = started.elapsed() >= self.timeout;
            if cancelled || timed_out {
                kill_process_group(pgid);
                let status = child
                    .wait()
                    .map_err(|e| ToolError::new(format!("reap: {e}")))?;
                break (status.code(), timed_out, cancelled);
            }
            thread::sleep(Duration::from_millis(20));
        };
        let stdout = out_reader
            .join()
            .map_err(|_| ToolError::new("stdout reader panicked"))?
            .map_err(|e| ToolError::new(format!("read stdout: {e}")))?;
        let stderr = err_reader
            .join()
            .map_err(|_| ToolError::new("stderr reader panicked"))?
            .map_err(|e| ToolError::new(format!("read stderr: {e}")))?;
        Ok(CommandOutput {
            status,
            stdout: self.clean(&stdout, &secrets),
            stderr: self.clean(&stderr, &secrets),
            timed_out,
            cancelled,
        })
    }

    fn clean(&self, bytes: &[u8], secrets: &[String]) -> String {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        redact_secrets(&mut text, secrets);
        cap_output_head_tail(&text, self.output_cap)
    }
}

fn kill_process_group(pgid: i32) {
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

fn secret_values() -> Vec<String> {
    let mut seen = HashSet::new();
    let mut values = Vec::new();
    for (name, value) in std::env::vars() {
        let upper = name.to_ascii_uppercase();
        if !value.is_empty()
            && ["KEY", "TOKEN", "SECRET", "PASSWORD"]
                .iter()
                .any(|pattern| upper.contains(pattern))
            && seen.insert(value.clone())
        {
            values.push(value);
        }
    }
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values
}

fn redact_secrets(text: &mut String, secrets: &[String]) {
    for value in secrets {
        *text = text.replace(value, "[REDACTED]");
    }
}

fn cap_output_head_tail(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    if cap < "truncated".len() {
        return text[..nearest_char_boundary(text, cap)].to_string();
    }
    let mut marker = "truncated".to_string();
    for _ in 0..4 {
        let payload_cap = cap.saturating_sub(marker.len());
        let head_end = nearest_char_boundary(text, payload_cap / 2);
        let tail_start = next_char_boundary(text, text.len() - (payload_cap - payload_cap / 2));
        let next = format!("\n[… {} bytes truncated …]\n", tail_start - head_end);
        marker = if next.len() <= cap {
            next
        } else {
            "truncated".to_string()
        };
    }
    let payload_cap = cap.saturating_sub(marker.len());
    let head_end = nearest_char_boundary(text, payload_cap / 2);
    let tail_start = next_char_boundary(text, text.len() - (payload_cap - payload_cap / 2));
    format!("{}{}{}", &text[..head_end], marker, &text[tail_start..])
}

fn nearest_char_boundary(text: &str, at: usize) -> usize {
    (0..=at.min(text.len()))
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, at: usize) -> usize {
    (at.min(text.len())..=text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::thread;

    // Environment mutation is process-global, so every test that changes it
    // holds this lock for the mutation's full lifetime.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard(&'static str, Option<std::ffi::OsString>);

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self(name, previous)
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.1.take() {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }

    #[test]
    fn output_below_cap_is_not_dropped_after_head_fills() {
        let dir = tempfile::tempdir().unwrap();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_output_cap(10)
            .run("printf 12345678")
            .unwrap();

        assert_eq!(out.stdout, "12345678");
    }

    #[test]
    fn truncation_preserves_utf8_boundaries_and_uses_one_marker() {
        let dir = tempfile::tempdir().unwrap();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_output_cap(40)
            .run("printf 'éééééééééééééééééééééééééééé'")
            .unwrap();

        assert!(!out.stdout.contains('\u{fffd}'));
        assert_eq!(out.stdout.matches("truncated").count(), 1);
        assert!(out.stdout.starts_with("éé"));
        assert!(out.stdout.ends_with("éé"));
        assert!(out.stdout.len() <= 40);
    }

    #[test]
    fn completed_shell_kills_background_descendants_before_joining_readers() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("background-survived");
        let command = format!("(sleep 1; touch '{}') &", marker.display());
        let started = Instant::now();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_millis(150))
            .run(&command)
            .unwrap();

        assert!(!out.timed_out);
        assert!(started.elapsed() < Duration::from_millis(500));
        thread::sleep(Duration::from_millis(1100));
        assert!(!marker.exists());
    }

    #[test]
    fn runs_at_repo_root_and_captures_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_secs(2))
            .run("pwd; printf err >&2; exit 7")
            .unwrap();
        assert_eq!(out.status, Some(7));
        assert_eq!(
            out.stdout.trim(),
            dir.path().canonicalize().unwrap().display().to_string()
        );
        assert_eq!(out.stderr, "err");
    }

    #[test]
    fn caps_and_redacts_output() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvVarGuard::set("QUECTO_TEST_SECRET_TOKEN", "m4-secret-value");
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_output_cap(64)
            .run("printf '%s' \"$QUECTO_TEST_SECRET_TOKEN\"; yes x | head -c 256")
            .unwrap();
        assert!(!out.stdout.contains("m4-secret-value"));
        assert!(out.stdout.contains("[REDACTED]"));
        assert!(out.stdout.contains("truncated"));
    }

    #[test]
    fn snapshots_secrets_before_the_child_runs() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvVarGuard::set("QUECTO_TEST_SECRET_TOKEN", "snapshot-secret");
        let remover = thread::spawn(|| {
            thread::sleep(Duration::from_millis(40));
            std::env::remove_var("QUECTO_TEST_SECRET_TOKEN");
        });
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_secs(1))
            .run("sleep 0.1; printf snapshot-secret")
            .unwrap();
        remover.join().unwrap();

        assert_eq!(out.stdout, "[REDACTED]");
    }

    #[test]
    fn redacts_longest_overlapping_secret_before_truncation() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _short = EnvVarGuard::set("QUECTO_TEST_TOKEN", "abc");
        let _long = EnvVarGuard::set("QUECTO_TEST_SECRET", "abcdef");
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_output_cap(40)
            .run("printf xxabcdef0123456789")
            .unwrap();

        assert!(!out.stdout.contains("abc"));
        assert!(!out.stdout.contains("def"));
        assert!(out.stdout.contains("[REDACTED]"));
    }

    #[test]
    fn timeout_kills_descendant_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("late.txt");
        let command = format!("(sleep 1; touch '{}') & wait", marker.display());
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_millis(100))
            .run(&command)
            .unwrap();
        assert!(out.timed_out);
        thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists());
    }

    #[test]
    fn cancellation_kills_running_group() {
        let dir = tempfile::tempdir().unwrap();
        let token = cancel_token();
        let setter = token.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            setter.store(true, Ordering::SeqCst);
        });
        let out = Sandbox::new(dir.path().to_path_buf(), token)
            .with_timeout(Duration::from_secs(2))
            .run("sleep 10")
            .unwrap();
        assert!(out.cancelled);
    }
}
