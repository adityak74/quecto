#[cfg(not(unix))]
compile_error!("quecto-agent M4 requires a Unix target");

use crate::tools::ToolError;
use std::collections::{HashSet, VecDeque};
use std::io::{self, Read};
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
        let output_cap = self.output_cap;
        let out_reader = thread::spawn(move || read_bounded(&mut stdout, output_cap));
        let err_reader = thread::spawn(move || read_bounded(&mut stderr, output_cap));
        let started = Instant::now();
        let (status, timed_out, cancelled) = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| ToolError::new(format!("wait: {e}")))?
            {
                break (status.code(), false, false);
            }
            let cancelled = self.cancel.load(Ordering::SeqCst);
            let timed_out = started.elapsed() >= self.timeout;
            if cancelled || timed_out {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
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
            stdout: self.clean(&stdout),
            stderr: self.clean(&stderr),
            timed_out,
            cancelled,
        })
    }

    fn clean(&self, capture: &BoundedCapture) -> String {
        let redact = |bytes: &[u8]| {
            let mut text = String::from_utf8_lossy(bytes).into_owned();
            redact_secrets(&mut text);
            text
        };
        if capture.omitted == 0 {
            let mut bytes = capture.head.clone();
            bytes.extend_from_slice(&capture.tail);
            return redact(&bytes);
        }
        let mut head = decode_truncated_head(&capture.head);
        let mut tail = decode_truncated_tail(&capture.tail);
        redact_secrets(&mut head);
        redact_secrets(&mut tail);
        format!("{head}\n[… {} bytes truncated …]\n{tail}", capture.omitted)
    }
}

fn decode_truncated_head(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(error) if error.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_truncated_tail(bytes: &[u8]) -> String {
    for start in 0..bytes.len().min(4) {
        if let Ok(text) = std::str::from_utf8(&bytes[start..]) {
            return text.to_string();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn redact_secrets(text: &mut String) {
    let mut seen = HashSet::new();
    for (name, value) in std::env::vars() {
        let upper = name.to_ascii_uppercase();
        if !value.is_empty()
            && ["KEY", "TOKEN", "SECRET", "PASSWORD"]
                .iter()
                .any(|pattern| upper.contains(pattern))
            && seen.insert(value.clone())
        {
            *text = text.replace(&value, "[REDACTED]");
        }
    }
}

#[derive(Debug)]
struct BoundedCapture {
    head: Vec<u8>,
    tail: Vec<u8>,
    omitted: usize,
}

impl BoundedCapture {
    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.head.len() + self.tail.len()
    }
}

fn read_bounded(mut reader: impl Read, cap: usize) -> io::Result<BoundedCapture> {
    let head_cap = cap / 2;
    let tail_cap = cap.saturating_sub(head_cap);
    let mut head = Vec::with_capacity(head_cap);
    let mut tail = VecDeque::with_capacity(tail_cap);
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        let mut chunk = &buffer[..count];
        if head.len() < head_cap {
            let take = (head_cap - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
        }
        for byte in chunk {
            if tail_cap == 0 {
                continue;
            }
            if tail.len() == tail_cap {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }

    let tail: Vec<u8> = tail.into_iter().collect();
    let retained = head.len() + tail.len();
    Ok(BoundedCapture {
        head,
        tail,
        omitted: total.saturating_sub(retained),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;
    use std::thread;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard(&'static str);

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            std::env::set_var(name, value);
            Self(name)
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    #[test]
    fn pipe_capture_retains_only_bounded_head_and_tail() {
        let input: Vec<u8> = (0..=255).cycle().take(4096).collect();
        let capture = read_bounded(Cursor::new(&input), 64).unwrap();

        assert_eq!(capture.retained_len(), 64);
        assert_eq!(capture.omitted, input.len() - 64);
        assert_eq!(capture.head, input[..32]);
        assert_eq!(capture.tail, input[input.len() - 32..]);
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
            .with_output_cap(10)
            .run("printf 'éééééééééé'")
            .unwrap();

        assert!(!out.stdout.contains('\u{fffd}'));
        assert_eq!(out.stdout.matches("truncated").count(), 1);
        assert!(out.stdout.starts_with("éé"));
        assert!(out.stdout.ends_with("éé"));
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
