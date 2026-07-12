use crossterm::style::Stylize;
use std::io::{self, IsTerminal, Write};

/// Receives an agent run's activity for display. Implementations format and
/// write; they never fail the run (write errors are ignored).
pub trait Renderer: Send {
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
        assert!(s.contains('\u{1b}'), "colored output should contain ANSI escapes");
        assert!(s.contains("read_file"));
    }
}
