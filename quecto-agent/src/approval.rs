use crate::model::ToolCall;
use std::io::{self, IsTerminal, Write};

pub trait Approver: Send + Sync {
    fn confirm(&self, call: &ToolCall) -> bool;
}

use std::sync::Arc;

#[derive(Clone)]
pub enum ApprovalMode {
    Interactive(Arc<dyn Approver>),
    NonInteractive,
    AutoApprove,
}

impl ApprovalMode {
    pub fn allows(&self, call: &ToolCall) -> bool {
        match self {
            Self::Interactive(a) => a.confirm(call),
            Self::NonInteractive => false,
            Self::AutoApprove => true,
        }
    }

    pub fn terminal(auto_approve: bool) -> Self {
        if auto_approve {
            Self::AutoApprove
        } else if io::stdin().is_terminal() {
            Self::Interactive(Arc::new(TerminalApprover))
        } else {
            Self::NonInteractive
        }
    }
}

pub struct TerminalApprover;
impl Approver for TerminalApprover {
    fn confirm(&self, call: &ToolCall) -> bool {
        eprint!("{}", approval_prompt(call));
        if io::stderr().flush().is_err() {
            return false;
        }
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

const SUMMARY_MAX_BYTES: usize = 200;
const COMMAND_MAX_BYTES: usize = 120;
const TOOL_NAME_MAX_BYTES: usize = 40;

fn approval_prompt(call: &ToolCall) -> String {
    format!(
        "Approve {} {}? [y/N] ",
        cap_summary(&call.name, TOOL_NAME_MAX_BYTES),
        summarize_call(call)
    )
}

fn summarize_call(call: &ToolCall) -> String {
    let value = |key: &str| call.arguments.get(key).and_then(|v| v.as_str());
    let summary = match call.name.as_str() {
        "write_file" => format!(
            "path={} content={} bytes",
            value("path").unwrap_or("<missing>"),
            value("content").map(str::len).unwrap_or(0)
        ),
        "apply_patch" => format!("patch={} bytes", value("patch").map(str::len).unwrap_or(0)),
        "run_command" => format!(
            "command={}",
            cap_summary(value("command").unwrap_or("<missing>"), COMMAND_MAX_BYTES)
        ),
        "read_file" | "list_files" => format!(
            "path={} args={}",
            value("path").unwrap_or("."),
            call.arguments
        ),
        "search_text" => format!(
            "pattern={} path={}",
            value("pattern").unwrap_or("<missing>"),
            value("path").unwrap_or(".")
        ),
        _ => call.arguments.to_string(),
    };
    cap_summary(&summary, SUMMARY_MAX_BYTES)
}

fn cap_summary(value: &str, max_bytes: usize) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    let marker = "…";
    let mut end = max_bytes.saturating_sub(marker.len());
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &sanitized[..end], marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Stub {
        answer: bool,
        calls: AtomicUsize,
    }
    impl Approver for Stub {
        fn confirm(&self, _call: &ToolCall) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer
        }
    }
    fn call() -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({}),
        }
    }

    #[test]
    fn modes_resolve_ask_safely() {
        assert!(!ApprovalMode::NonInteractive.allows(&call()));
        assert!(ApprovalMode::AutoApprove.allows(&call()));
        assert!(ApprovalMode::Interactive(Arc::new(Stub {
            answer: true,
            calls: AtomicUsize::new(0)
        }))
        .allows(&call()));
        assert!(!ApprovalMode::Interactive(Arc::new(Stub {
            answer: false,
            calls: AtomicUsize::new(0)
        }))
        .allows(&call()));
    }

    #[test]
    fn mutation_summaries_never_include_file_or_patch_contents() {
        let secret = "private-content".repeat(100);
        let write = ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({"path":"src/main.rs", "content":secret}),
        };
        let patch = ToolCall {
            id: "2".into(),
            name: "apply_patch".into(),
            arguments: json!({"patch":format!("*** Begin Patch\n{secret}\n*** End Patch")}),
        };

        let write_summary = summarize_call(&write);
        let patch_summary = summarize_call(&patch);
        assert!(write_summary.contains("src/main.rs"));
        assert!(write_summary.contains("1500 bytes"));
        assert!(!write_summary.contains("private-content"));
        assert!(patch_summary.contains(&format!(
            "{} bytes",
            patch.arguments["patch"].as_str().unwrap().len()
        )));
        assert!(!patch_summary.contains("private-content"));
    }

    #[test]
    fn summaries_are_utf8_safe_bounded_and_tool_specific() {
        let command = "echo é".repeat(100);
        let run = ToolCall {
            id: "1".into(),
            name: "run_command".into(),
            arguments: json!({"command":command}),
        };
        let read = ToolCall {
            id: "2".into(),
            name: "read_file".into(),
            arguments: json!({"path":"src/lib.rs", "start_line":10, "end_line":20}),
        };

        let run_summary = summarize_call(&run);
        assert!(run_summary.contains("…"));
        assert!(run_summary.len() <= 240);
        assert!(summarize_call(&read).contains("src/lib.rs"));
    }

    #[test]
    fn complete_prompt_has_a_hard_bound() {
        let call = ToolCall {
            id: "1".into(),
            name: "untrusted-tool-name".repeat(100),
            arguments: json!({"untrusted":"private-content".repeat(100)}),
        };

        let prompt = approval_prompt(&call);
        assert!(prompt.len() <= 263);
        assert!(!prompt.contains(&"private-content".repeat(20)));
    }
}
