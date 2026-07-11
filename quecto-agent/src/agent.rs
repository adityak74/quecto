use crate::approval::ApprovalMode;
use crate::model::{Message, Model};
use crate::policy::{Decision, Policy};
use crate::sandbox::CancelToken;
use crate::tools::{builtin_tools, Context, Registry, Tool, ToolOutput};
use crate::BoxErr;
use std::path::PathBuf;

/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    Error(BoxErr),
}

/// The agent loop: reason -> call read-only tools -> observe -> answer.
pub struct Agent {
    model: Box<dyn Model>,
    registry: Registry,
    cx: Context,
    messages: Vec<Message>,
    max_steps: usize,
    policy: Policy,
    approval: ApprovalMode,
    #[allow(dead_code)]
    cancel: CancelToken,
}

impl Agent {
    /// Create an agent with a model, a system prompt, a step limit, and the
    /// repository root that filesystem tools are scoped to.
    pub fn new(
        model: Box<dyn Model>,
        system: impl Into<String>,
        max_steps: usize,
        repo_root: PathBuf,
        cancel: CancelToken,
        approval: ApprovalMode,
    ) -> Self {
        Agent {
            model,
            registry: Registry::new(),
            cx: Context::new(repo_root, cancel.clone()),
            messages: vec![Message::system(system.into())],
            max_steps,
            policy: Policy,
            approval,
            cancel,
        }
    }

    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        self.registry.register(tool);
        self
    }

    pub fn register_builtins(mut self) -> Self {
        for tool in builtin_tools() {
            self.registry.register(tool);
        }
        self
    }

    /// Run one task to completion (or a limit/error). Appends the task as a user
    /// message and loops: call the model with the available tool schemas, execute
    /// any tool calls, feed results back, and finish when the model stops
    /// requesting tools. Unknown tools are reported back as an error observation.
    pub fn run(&mut self, task: &str) -> Outcome {
        self.messages.push(Message::user(task));
        let schemas = self.registry.schemas();
        let mut step = 0;
        loop {
            if step >= self.max_steps {
                return Outcome::StepLimit;
            }
            let msg = match self.model.complete(&self.messages, &schemas) {
                Ok(m) => m,
                Err(e) => return Outcome::Error(e),
            };
            self.messages.push(Message::assistant_with_calls(
                msg.content.clone(),
                msg.tool_calls.clone(),
            ));
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                let out = match self.policy.decide(call) {
                    Decision::Allow => self.registry.dispatch(call, &mut self.cx),
                    Decision::Ask if self.approval.allows(call) => {
                        self.registry.dispatch(call, &mut self.cx)
                    }
                    Decision::Ask => ToolOutput::new("denied: approval required", "denied"),
                    Decision::Deny(reason) => {
                        ToolOutput::new(format!("denied: {reason}"), "denied")
                    }
                };
                eprintln!("● {}  {}", call.name, out.summary);
                self.messages
                    .push(Message::tool_result(&call.id, out.content));
            }
            step += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalMode;
    use crate::model::{AssistantMessage, ToolCall};
    use crate::sandbox::cancel_token;
    use crate::tools::{Context, Tool, ToolOutput, ToolResult};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct Scripted {
        replies: Mutex<Vec<AssistantMessage>>,
    }
    impl Scripted {
        fn new(replies: Vec<AssistantMessage>) -> Self {
            Scripted {
                replies: Mutex::new(replies),
            }
        }
    }
    impl Model for Scripted {
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<AssistantMessage, BoxErr> {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                return Err("no more scripted replies".into());
            }
            Ok(r.remove(0))
        }
    }

    fn text(c: &str) -> AssistantMessage {
        AssistantMessage {
            content: c.to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
        }
    }

    fn wants_tool(name: &str) -> AssistantMessage {
        AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".to_string(),
                name: name.to_string(),
                arguments: json!({}),
            }],
            finish_reason: "tool_calls".to_string(),
        }
    }

    fn configured_agent(model: Scripted, approval: ApprovalMode) -> Agent {
        Agent::new(
            Box::new(model),
            "sys",
            10,
            PathBuf::from("."),
            cancel_token(),
            approval,
        )
    }

    fn agent(model: Scripted) -> Agent {
        configured_agent(model, ApprovalMode::NonInteractive)
    }

    struct RecordingNamed {
        name: &'static str,
        ran: Arc<AtomicBool>,
    }

    impl Tool for RecordingNamed {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "records that it ran"
        }

        fn schema(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }

        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            self.ran.store(true, Ordering::SeqCst);
            Ok(ToolOutput::new("recorded", "ok"))
        }
    }

    #[test]
    fn completes_on_text_only_reply() {
        match agent(Scripted::new(vec![text("hello")])).run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn dispatches_a_registered_tool_then_completes() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        let mut a = agent(model).register(Box::new(RecordingNamed {
            name: "read_file",
            ran: ran.clone(),
        }));
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete"),
        }
        assert!(
            ran.load(Ordering::SeqCst),
            "the tool should have been dispatched"
        );
    }

    #[test]
    fn ask_tool_is_denied_without_interactivity() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
        let mut a = configured_agent(model, ApprovalMode::NonInteractive).register(Box::new(
            RecordingNamed {
                name: "write_file",
                ran: ran.clone(),
            },
        ));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn auto_approve_runs_ask_tool_but_not_hard_denies() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
        let mut a =
            configured_agent(model, ApprovalMode::AutoApprove).register(Box::new(RecordingNamed {
                name: "write_file",
                ran: ran.clone(),
            }));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn unknown_custom_tool_is_denied_even_if_registered() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("custom"), text("done")]);
        let mut a =
            configured_agent(model, ApprovalMode::AutoApprove).register(Box::new(RecordingNamed {
                name: "custom",
                ran: ran.clone(),
            }));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn unknown_tool_is_reported_then_completes() {
        let model = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        match agent(model).run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after error observation"),
        }
    }

    #[test]
    fn step_limit_stops_a_spinning_model() {
        let model = Scripted::new(vec![wants_tool("x"), wants_tool("x"), wants_tool("x")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            2,
            PathBuf::from("."),
            cancel_token(),
            ApprovalMode::NonInteractive,
        );
        assert!(matches!(a.run("hi"), Outcome::StepLimit));
    }

    #[test]
    fn agent_write_file_flows_through_the_loop() {
        use crate::tools::fs::WriteFile;

        let dir = tempfile::tempdir().unwrap();
        let call = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"hello.txt","content":"hi there\n"}),
            }],
            finish_reason: "tool_calls".into(),
        };
        let model = Scripted::new(vec![call, text("done")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            10,
            dir.path().to_path_buf(),
            cancel_token(),
            ApprovalMode::AutoApprove,
        )
        .register(Box::new(WriteFile));
        match a.run("make the file") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hi there\n"
        );
    }
}
