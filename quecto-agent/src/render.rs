use crossterm::style::Stylize;
use std::io::{self, IsTerminal, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_SPINNER_VERBS: &[&str] = &[
    "Thinking",
    "Working",
    "Crafting",
    "Computing",
    "Pondering",
    "Wrangling",
];

pub fn parse_spinner_verbs(raw: Option<&str>) -> Vec<String> {
    let verbs: Vec<String> = raw
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    if verbs.is_empty() {
        DEFAULT_SPINNER_VERBS
            .iter()
            .map(|verb| (*verb).to_string())
            .collect()
    } else {
        verbs
    }
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_CLEAR: &str = "\r\x1b[2K";

fn format_spinner_frame(frame: &str, verb: &str) -> String {
    format!("\r{frame} {verb}…")
}

struct SpinnerState {
    verbs: Vec<String>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Receives an agent run's activity for display. Implementations format and
/// write; they never fail the run (write errors are ignored).
pub trait Renderer: Send {
    fn working(&mut self) {}
    fn working_done(&mut self) {}
    fn tool(&mut self, name: &str, summary: &str);
    fn verify(&mut self, command: &str, passed: bool);
    fn notice(&mut self, text: &str);
    fn assistant(&mut self, text: &str);
}

/// Line-based renderer over any writer. Colors are applied only when `color`
/// is true; with `color = false` the output is byte-identical to the agent's
/// historical plain output.
pub struct LineRenderer<W: Write> {
    out: W,
    color: bool,
}

impl<W: Write> LineRenderer<W> {
    pub fn new(out: W, color: bool) -> Self {
        LineRenderer { out, color }
    }

    fn bullet(&self) -> String {
        if self.color {
            format!("{}", "●".cyan())
        } else {
            "●".to_string()
        }
    }
}

impl<W: Write + Send> Renderer for LineRenderer<W> {
    fn tool(&mut self, name: &str, summary: &str) {
        let _ = writeln!(self.out, "{} {name}  {summary}", self.bullet());
    }

    fn verify(&mut self, command: &str, passed: bool) {
        let word = if passed { "passed" } else { "failed" };
        let shown = if self.color {
            if passed {
                format!("{}", word.green())
            } else {
                format!("{}", word.red())
            }
        } else {
            word.to_string()
        };
        let _ = writeln!(self.out, "{} verify {command}  {shown}", self.bullet());
    }

    fn notice(&mut self, text: &str) {
        let shown = if self.color {
            format!("{}", text.dark_grey())
        } else {
            text.to_string()
        };
        let _ = writeln!(self.out, "{shown}");
    }

    fn assistant(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }
}

struct SpinnerRenderer<W: Write + Send + 'static> {
    out: Arc<Mutex<W>>,
    color: bool,
    spinner: SpinnerState,
}

impl<W: Write + Send + 'static> SpinnerRenderer<W> {
    fn new(out: W, color: bool, verbs: Vec<String>) -> Self {
        Self {
            out: Arc::new(Mutex::new(out)),
            color,
            spinner: SpinnerState {
                verbs: if verbs.is_empty() {
                    parse_spinner_verbs(None)
                } else {
                    verbs
                },
                stop: None,
                thread: None,
            },
        }
    }

    fn write_raw(&self, text: &str) {
        if let Ok(mut out) = self.out.lock() {
            let _ = write!(out, "{text}");
            let _ = out.flush();
        }
    }

    fn stop_spinner(&mut self) {
        if let Some(thread) = self.spinner.thread.take() {
            if let Some(stop) = self.spinner.stop.take() {
                let _ = stop.send(());
            }
            let _ = thread.join();
            self.write_raw(SPINNER_CLEAR);
        }
    }

    fn start_spinner(&mut self) {
        if self.spinner.thread.is_some() {
            return;
        }
        let (stop, wakeup) = mpsc::channel();
        let (started, started_rx) = mpsc::sync_channel(0);
        let out = Arc::clone(&self.out);
        let verbs = self.spinner.verbs.clone();
        self.spinner.thread = Some(thread::spawn(move || {
            let mut frame = 0;
            let verb = verbs.first().expect("spinner verbs are never empty");
            loop {
                let text = format_spinner_frame(SPINNER_FRAMES[frame], verb);
                let wrote = out
                    .lock()
                    .ok()
                    .is_some_and(|mut out| write!(out, "{text}").and_then(|_| out.flush()).is_ok());
                if !wrote {
                    break;
                }
                frame = (frame + 1) % SPINNER_FRAMES.len();
                let _ = started.send(());
                match wakeup.recv_timeout(Duration::from_millis(120)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }));
        self.spinner.stop = Some(stop);
        let _ = started_rx.recv();
    }

    fn bullet(&self) -> String {
        if self.color {
            format!("{}", "●".cyan())
        } else {
            "●".to_string()
        }
    }
}

impl<W: Write + Send + 'static> Drop for SpinnerRenderer<W> {
    fn drop(&mut self) {
        self.stop_spinner();
    }
}

impl<W: Write + Send + 'static> Renderer for SpinnerRenderer<W> {
    fn working(&mut self) {
        self.start_spinner();
    }

    fn working_done(&mut self) {
        self.stop_spinner();
    }

    fn tool(&mut self, name: &str, summary: &str) {
        self.stop_spinner();
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{} {name}  {summary}", self.bullet());
        }
    }

    fn verify(&mut self, command: &str, passed: bool) {
        self.stop_spinner();
        let word = if passed { "passed" } else { "failed" };
        let shown = if self.color {
            if passed {
                format!("{}", word.green())
            } else {
                format!("{}", word.red())
            }
        } else {
            word.to_string()
        };
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{} verify {command}  {shown}", self.bullet());
        }
    }

    fn notice(&mut self, text: &str) {
        self.stop_spinner();
        let shown = if self.color {
            format!("{}", text.dark_grey())
        } else {
            text.to_string()
        };
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{shown}");
        }
    }

    fn assistant(&mut self, text: &str) {
        self.stop_spinner();
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{text}");
        }
    }
}

/// A boxed renderer over stderr, colored only when stderr is a TTY.
pub fn stderr_renderer() -> Box<dyn Renderer> {
    let color = io::stderr().is_terminal();
    Box::new(LineRenderer::new(io::stderr(), color))
}

/// A boxed renderer over stdout, colored only when stdout is a TTY.
pub fn stdout_renderer() -> Box<dyn Renderer> {
    let color = io::stdout().is_terminal();
    Box::new(LineRenderer::new(io::stdout(), color))
}

/// A spinner renderer for the interactive chat's TTY-backed stdout path.
pub fn chat_spinner_renderer(verbs: Vec<String>) -> Box<dyn Renderer> {
    Box::new(SpinnerRenderer::new(io::stdout(), true, verbs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn render_plain(f: impl FnOnce(&mut LineRenderer<&mut Vec<u8>>)) -> String {
        let mut buf = Vec::new();
        {
            let mut r = LineRenderer::new(&mut buf, false);
            f(&mut r);
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn plain_tool_line_matches_legacy_format() {
        let s = render_plain(|r| r.tool("read_file", "1 lines"));
        assert_eq!(s, "● read_file  1 lines\n");
    }

    #[test]
    fn plain_verify_line_reports_pass_and_fail() {
        assert_eq!(
            render_plain(|r| r.verify("cargo test", true)),
            "● verify cargo test  passed\n"
        );
        assert_eq!(
            render_plain(|r| r.verify("cargo test", false)),
            "● verify cargo test  failed\n"
        );
    }

    #[test]
    fn plain_notice_and_assistant_are_raw_text() {
        assert_eq!(render_plain(|r| r.notice("hello")), "hello\n");
        assert_eq!(render_plain(|r| r.assistant("answer")), "answer\n");
    }

    #[test]
    fn color_output_contains_ansi_escapes() {
        let mut buf = Vec::new();
        {
            let mut r = LineRenderer::new(&mut buf, true);
            r.tool("read_file", "x");
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains('\u{1b}'),
            "colored output should contain ANSI escapes"
        );
        assert!(s.contains("read_file"));
    }

    #[test]
    fn spinner_verbs_use_compact_defaults_when_unconfigured() {
        assert_eq!(
            parse_spinner_verbs(None),
            vec![
                "Thinking",
                "Working",
                "Crafting",
                "Computing",
                "Pondering",
                "Wrangling"
            ]
        );
    }

    #[test]
    fn spinner_verbs_trim_and_ignore_empty_custom_entries() {
        assert_eq!(
            parse_spinner_verbs(Some(" Brewing, , Refactoring ,, ")),
            vec!["Brewing", "Refactoring"]
        );
    }

    #[test]
    fn spinner_verbs_fall_back_when_custom_value_has_no_entries() {
        assert_eq!(parse_spinner_verbs(Some(" ,  , "))[0], "Thinking");
    }

    #[test]
    fn spinner_frame_contains_frame_and_verb() {
        assert_eq!(format_spinner_frame("⠋", "Brewing"), "\r⠋ Brewing…");
    }

    #[test]
    fn spinner_clear_sequence_erases_the_temporary_line() {
        assert_eq!(SPINNER_CLEAR, "\r\x1b[2K");
    }

    #[test]
    fn disabled_spinner_preserves_plain_output_after_lifecycle_hooks() {
        let mut buf = Vec::new();
        {
            let mut r = LineRenderer::new(&mut buf, false);
            r.working();
            r.tool("read_file", "1 lines");
            r.notice("hello");
            r.assistant("answer");
            r.working_done();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "● read_file  1 lines\nhello\nanswer\n"
        );
    }

    #[test]
    fn enabled_spinner_writes_to_its_renderer_sink_and_clears_before_output() {
        let capture = Capture::new();
        let mut renderer =
            SpinnerRenderer::new(capture.clone(), false, vec!["Brewing".to_string()]);

        renderer.working();
        renderer.notice("ready");

        assert_eq!(capture.contents(), "\r⠋ Brewing…\r\x1b[2Kready\n");
    }

    #[test]
    fn enabled_spinner_keeps_one_verb_for_the_whole_model_wait() {
        let capture = Capture::new();
        let mut renderer = SpinnerRenderer::new(
            capture.clone(),
            false,
            vec!["Brewing".to_string(), "Cooking".to_string()],
        );

        renderer.working();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !capture.contents().contains("⠙ Brewing…")
            && std::time::Instant::now() < deadline
        {
            thread::yield_now();
        }
        renderer.working_done();

        let output = capture.contents();
        assert!(output.contains("⠋ Brewing…"));
        assert!(output.contains("⠙ Brewing…"));
        assert!(!output.contains("Cooking"));
    }

    #[test]
    fn enabled_spinner_ignores_repeated_starts() {
        let capture = Capture::new();
        let mut renderer =
            SpinnerRenderer::new(capture.clone(), false, vec!["Brewing".to_string()]);

        renderer.working();
        let first_frame = capture.contents();
        renderer.working();
        renderer.working_done();

        assert_eq!(first_frame, "\r⠋ Brewing…");
        assert_eq!(capture.contents(), "\r⠋ Brewing…\r\x1b[2K");
    }

    #[test]
    fn dropping_an_active_spinner_stops_and_clears_its_sink() {
        let capture = Capture::new();
        {
            let mut renderer =
                SpinnerRenderer::new(capture.clone(), false, vec!["Brewing".to_string()]);
            renderer.working();
        }

        assert_eq!(capture.contents(), "\r⠋ Brewing…\r\x1b[2K");
    }

    #[test]
    fn spinner_worker_write_error_is_joined_during_cleanup() {
        let mut renderer = SpinnerRenderer::new(FailingWriter, false, vec!["Brewing".to_string()]);

        renderer.working();
        assert!(renderer.spinner.thread.as_ref().unwrap().is_finished());
        renderer.working_done();
        assert!(renderer.spinner.thread.is_none());
    }
}
