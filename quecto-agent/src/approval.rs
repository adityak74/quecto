use crate::model::ToolCall;
use std::io::{self, IsTerminal, Write};

pub trait Approver: Send + Sync {
    fn confirm(&self, call: &ToolCall) -> bool;
}

pub enum ApprovalMode {
    Interactive(Box<dyn Approver>),
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
            Self::Interactive(Box::new(TerminalApprover))
        } else {
            Self::NonInteractive
        }
    }
}

pub struct TerminalApprover;
impl Approver for TerminalApprover {
    fn confirm(&self, call: &ToolCall) -> bool {
        eprint!("Approve {} {}? [y/N] ", call.name, call.arguments);
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
        assert!(ApprovalMode::Interactive(Box::new(Stub {
            answer: true,
            calls: AtomicUsize::new(0)
        }))
        .allows(&call()));
        assert!(!ApprovalMode::Interactive(Box::new(Stub {
            answer: false,
            calls: AtomicUsize::new(0)
        }))
        .allows(&call()));
    }
}
