#[cfg(not(unix))]
compile_error!("quecto-agent M4 requires a Unix target");

use crate::tools::ToolError;
use std::collections::{HashSet, VecDeque};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
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
    reader_finalize_delay: Duration,
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
            reader_finalize_delay: Duration::ZERO,
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

    #[cfg(test)]
    fn with_reader_finalize_delay(mut self, delay: Duration) -> Self {
        self.reader_finalize_delay = delay;
        self
    }

    pub fn run(&self, command: &str) -> Result<CommandOutput, ToolError> {
        let secrets = Arc::new(secret_values());
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
        let stdout_probe = duplicate_fd(stdout.as_raw_fd())?;
        let stderr_probe = duplicate_fd(stderr.as_raw_fd())?;
        let output_cap = self.output_cap;
        let finalize_delay = self.reader_finalize_delay;
        let stdout_eof = Arc::new(AtomicBool::new(false));
        let stderr_eof = Arc::new(AtomicBool::new(false));
        let stdout_secrets = secrets.clone();
        let stdout_eof_reader = stdout_eof.clone();
        let out_reader = thread::spawn(move || {
            capture_stream(
                &mut stdout,
                output_cap,
                stdout_secrets,
                stdout_eof_reader,
                finalize_delay,
            )
        });
        let stderr_eof_reader = stderr_eof.clone();
        let err_reader = thread::spawn(move || {
            capture_stream(
                &mut stderr,
                output_cap,
                secrets,
                stderr_eof_reader,
                finalize_delay,
            )
        });
        let started = Instant::now();
        let (status, timed_out, cancelled) = loop {
            if child_exited_unreaped(pgid)? {
                let pipes_closed = stream_closed(&stdout_eof, &stdout_probe)?
                    && stream_closed(&stderr_eof, &stderr_probe)?;
                let kill_result = if pipes_closed {
                    Ok(())
                } else {
                    kill_process_group(pgid)
                };
                let status = child
                    .wait()
                    .map_err(|e| ToolError::new(format!("reap: {e}")))?;
                kill_result.map_err(|e| ToolError::new(format!("kill process group: {e}")))?;
                break (status.code(), false, false);
            }
            let cancelled = self.cancel.load(Ordering::SeqCst);
            let timed_out = started.elapsed() >= self.timeout;
            if cancelled || timed_out {
                let kill_result = kill_process_group(pgid);
                if let Err(error) = kill_result {
                    child.try_wait().map_err(|e| {
                        ToolError::new(format!("check child after failed kill: {e}"))
                    })?;
                    return Err(ToolError::new(format!("kill process group: {error}")));
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
            stdout: render_capture(&stdout, self.output_cap),
            stderr: render_capture(&stderr, self.output_cap),
            timed_out,
            cancelled,
        })
    }
}

fn duplicate_fd(fd: i32) -> Result<OwnedFd, ToolError> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated == -1 {
        return Err(ToolError::new(format!(
            "duplicate output pipe: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

fn stream_closed(eof: &AtomicBool, probe: &OwnedFd) -> Result<bool, ToolError> {
    if eof.load(Ordering::SeqCst) {
        return Ok(true);
    }
    let mut descriptor = libc::pollfd {
        fd: probe.as_raw_fd(),
        events: libc::POLLHUP,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result == -1 {
        return Err(ToolError::new(format!(
            "poll output pipe: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(descriptor.revents & libc::POLLHUP != 0)
}

fn child_exited_unreaped(pid: i32) -> Result<bool, ToolError> {
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(ToolError::new(format!(
            "observe child: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(unsafe { info.si_pid() } != 0)
}

fn kill_process_group(pgid: i32) -> io::Result<()> {
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn secret_values() -> Vec<Vec<u8>> {
    let mut seen = HashSet::new();
    let mut values = Vec::new();
    for (name, value) in std::env::vars_os() {
        let name = name.as_os_str().as_bytes();
        let value = value.as_os_str().as_bytes().to_vec();
        if !value.is_empty()
            && ["KEY", "TOKEN", "SECRET", "PASSWORD"]
                .iter()
                .any(|pattern| contains_ascii_case_insensitive(name, pattern.as_bytes()))
            && seen.insert(value.clone())
        {
            values.push(value);
        }
    }
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

#[derive(Debug)]
struct BoundedCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
    cap: usize,
}

impl BoundedCapture {
    fn new(cap: usize) -> Self {
        Self {
            head: Vec::with_capacity(cap / 2),
            tail: VecDeque::with_capacity(cap - cap / 2),
            total: 0,
            cap,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let head_cap = self.cap / 2;
        let tail_cap = self.cap - head_cap;
        self.total = self.total.saturating_add(bytes.len());
        let mut bytes = bytes;
        if self.head.len() < head_cap {
            let take = (head_cap - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        for byte in bytes {
            if tail_cap == 0 {
                continue;
            }
            if self.tail.len() == tail_cap {
                self.tail.pop_front();
            }
            self.tail.push_back(*byte);
        }
    }

    fn retained_len(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    fn omitted(&self) -> usize {
        self.total.saturating_sub(self.retained_len())
    }

    #[cfg(test)]
    fn retained_bytes(&self) -> Vec<u8> {
        self.head
            .iter()
            .copied()
            .chain(self.tail.iter().copied())
            .collect()
    }
}

fn capture_stream(
    mut reader: impl Read,
    cap: usize,
    secrets: Arc<Vec<Vec<u8>>>,
    eof: Arc<AtomicBool>,
    finalize_delay: Duration,
) -> io::Result<BoundedCapture> {
    let mut capture = BoundedCapture::new(cap);
    let mut pending = Vec::new();
    let overlap = secrets
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut buffer = [0u8; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            eof.store(true, Ordering::SeqCst);
            if !finalize_delay.is_zero() {
                thread::sleep(finalize_delay);
            }
            let process_len = pending.len();
            redact_pending(&mut pending, process_len, &secrets, &mut capture);
            break;
        }
        pending.extend_from_slice(&buffer[..count]);
        let process_len = pending.len().saturating_sub(overlap);
        redact_pending(&mut pending, process_len, &secrets, &mut capture);
    }
    Ok(capture)
}

fn redact_pending(
    pending: &mut Vec<u8>,
    process_len: usize,
    secrets: &[Vec<u8>],
    capture: &mut BoundedCapture,
) {
    let mut position = 0;
    while position < process_len {
        if let Some(secret) = secrets
            .iter()
            .find(|secret| pending[position..].starts_with(secret))
        {
            capture.push(b"[REDACTED]");
            position += secret.len();
        } else {
            capture.push(&pending[position..position + 1]);
            position += 1;
        }
    }
    pending.drain(..position);
}

fn render_capture(capture: &BoundedCapture, cap: usize) -> String {
    if capture.omitted() == 0 {
        let bytes: Vec<u8> = capture
            .head
            .iter()
            .copied()
            .chain(capture.tail.iter().copied())
            .collect();
        return cap_output_head_tail(&String::from_utf8_lossy(&bytes), cap);
    }
    let head = decode_truncated_head(&capture.head);
    let tail_bytes: Vec<u8> = capture.tail.iter().copied().collect();
    let tail = decode_truncated_tail(&tail_bytes);
    cap_separate_head_tail(&head, &tail, capture.omitted(), cap)
}

fn cap_output_head_tail(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let half = cap / 2;
    let head_end = nearest_char_boundary(text, half);
    let tail_start = next_char_boundary(text, text.len() - (cap - half));
    cap_separate_head_tail(
        &text[..head_end],
        &text[tail_start..],
        tail_start - head_end,
        cap,
    )
}

fn cap_separate_head_tail(head: &str, tail: &str, omitted: usize, cap: usize) -> String {
    if cap < "truncated".len() {
        return head[..nearest_char_boundary(head, cap)].to_string();
    }
    let marker = format!("\n[… {omitted} bytes truncated …]\n");
    let marker = if marker.len() <= cap {
        marker
    } else {
        "truncated".into()
    };
    let payload_cap = cap.saturating_sub(marker.len());
    let head_end = nearest_char_boundary(head, (payload_cap / 2).min(head.len()));
    let tail_bytes = (payload_cap - payload_cap / 2).min(tail.len());
    let tail_start = next_char_boundary(tail, tail.len() - tail_bytes);
    format!("{}{}{}", &head[..head_end], marker, &tail[tail_start..])
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
    use std::io::Cursor;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::Mutex;
    use std::thread;

    // Environment mutation is process-global, so every test that changes it
    // holds this lock for the mutation's full lifetime.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn streaming_redaction_retains_bounded_output_and_cross_chunk_secrets() {
        let mut input = vec![b'x'; 4095];
        input.extend_from_slice(b"cross-boundary-secret");
        input.extend(std::iter::repeat_n(b'y', 4096));
        let expected_redacted_len =
            input.len() - b"cross-boundary-secret".len() + b"[REDACTED]".len();
        let secrets = Arc::new(vec![b"cross-boundary-secret".to_vec()]);
        let eof = Arc::new(AtomicBool::new(false));

        let capture =
            capture_stream(Cursor::new(input), 64, secrets, eof.clone(), Duration::ZERO).unwrap();

        assert!(eof.load(Ordering::SeqCst));
        assert!(capture.retained_len() <= 64);
        assert!(capture.omitted() > 0);
        assert_eq!(capture.total, expected_redacted_len);
        assert!(!capture
            .retained_bytes()
            .windows(21)
            .any(|w| w == b"cross-boundary-secret"));
    }

    #[test]
    fn observes_child_exit_without_reaping_it() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !child_exited_unreaped(child.id() as i32).unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(child.wait().unwrap().code(), Some(7));
    }

    struct EnvVarGuard(&'static str, Option<std::ffi::OsString>);

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            Self::set_os(name, value)
        }

        fn set_os(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self(name, previous)
        }
    }

    #[test]
    fn snapshots_non_unicode_environment_best_effort() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let invalid = std::ffi::OsString::from_vec(vec![b'a', 0xff, b'b']);
        let _env = EnvVarGuard::set_os("QUECTO_TEST_SECRET", &invalid);
        let dir = tempfile::tempdir().unwrap();

        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .run("printf '%s' \"$QUECTO_TEST_SECRET\"")
            .unwrap();

        assert_eq!(out.stdout, "[REDACTED]");
        assert!(!out.stdout.contains('�'));
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
    fn eof_signal_precedes_delayed_reader_finalization() {
        let dir = tempfile::tempdir().unwrap();
        let out = Sandbox::new(dir.path().to_path_buf(), cancel_token())
            .with_timeout(Duration::from_secs(1))
            .with_reader_finalize_delay(Duration::from_millis(100))
            .run("printf done")
            .unwrap();

        assert_eq!(out.stdout, "done");
        assert!(!out.timed_out);
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
