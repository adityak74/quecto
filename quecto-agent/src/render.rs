use crossterm::style::Stylize;
use std::io::{self, IsTerminal, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
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

pub(crate) fn parse_spinner_verbs(raw: Option<&str>) -> Vec<String> {
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
    stop: Arc<AtomicBool>,
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
    spinner: Option<SpinnerState>,
}

impl<W: Write> LineRenderer<W> {
    pub fn new(out: W, color: bool) -> Self {
        LineRenderer {
            out,
            color,
            spinner: None,
        }
    }

    /// Constructs a renderer with an enabled spinner. Callers should only
    /// use this for a TTY-backed stdout renderer.
    pub fn with_spinner(out: W, color: bool, verbs: Vec<String>) -> Self {
        LineRenderer {
            out,
            color,
            spinner: Some(SpinnerState {
                verbs: if verbs.is_empty() {
                    parse_spinner_verbs(None)
                } else {
                    verbs
                },
                stop: Arc::new(AtomicBool::new(false)),
                thread: None,
            }),
        }
    }

    fn stop_spinner(&mut self) {
        let Some(mut spinner) = self.spinner.take() else {
            return;
        };
        if let Some(thread) = spinner.thread.take() {
            spinner.stop.store(true, Ordering::Release);
            let _ = thread.join();
            let _ = write!(self.out, "{SPINNER_CLEAR}");
            let _ = self.out.flush();
        }
        self.spinner = Some(spinner);
    }

    fn start_spinner(&mut self, verbs: Vec<String>) {
        let Some(spinner) = self.spinner.as_mut() else {
            return;
        };
        if spinner.thread.is_some() {
            return;
        }
        spinner.stop.store(false, Ordering::Release);
        let stop = Arc::clone(&spinner.stop);
        let verbs = if verbs.is_empty() {
            spinner.verbs.clone()
        } else {
            verbs
        };
        spinner.thread = Some(thread::spawn(move || {
            let mut stdout = io::stdout();
            let mut frame = 0;
            let mut verb = 0;
            while !stop.load(Ordering::Acquire) {
                let text = format_spinner_frame(SPINNER_FRAMES[frame], &verbs[verb]);
                let _ = write!(stdout, "{text}");
                let _ = stdout.flush();
                frame = (frame + 1) % SPINNER_FRAMES.len();
                verb = (verb + 1) % verbs.len();
                thread::sleep(Duration::from_millis(120));
            }
        }));
    }

    fn bullet(&self) -> String {
        if self.color {
            format!("{}", "●".cyan())
        } else {
            "●".to_string()
        }
    }
}

impl<W: Write> Drop for LineRenderer<W> {
    fn drop(&mut self) {
        self.stop_spinner();
    }
}

impl<W: Write + Send> Renderer for LineRenderer<W> {
    fn working(&mut self) {
        if let Some(spinner) = self.spinner.as_ref() {
            self.start_spinner(spinner.verbs.clone());
        }
    }

    fn working_done(&mut self) {
        self.stop_spinner();
    }

    fn tool(&mut self, name: &str, summary: &str) {
        self.stop_spinner();
        let _ = writeln!(self.out, "{} {name}  {summary}", self.bullet());
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
        let _ = writeln!(self.out, "{} verify {command}  {shown}", self.bullet());
    }

    fn notice(&mut self, text: &str) {
        self.stop_spinner();
        let shown = if self.color {
            format!("{}", text.dark_grey())
        } else {
            text.to_string()
        };
        let _ = writeln!(self.out, "{shown}");
    }

    fn assistant(&mut self, text: &str) {
        self.stop_spinner();
        let _ = writeln!(self.out, "{text}");
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
