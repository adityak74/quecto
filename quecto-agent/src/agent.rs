use crate::approval::ApprovalMode;
use crate::model::{Message, Model};
use crate::policy::{Decision, Policy};
use crate::sandbox::CancelToken;
use crate::tools::{builtin_tools, Context, Registry, Tool, ToolOutput};
use crate::verify::Verifier;
use crate::BoxErr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    Cancelled,
    RepeatedAction,
    Error(BoxErr),
}

#[derive(Default)]
struct RepeatGuard {
    fingerprint: Option<String>,
    changes: usize,
    streak: usize,
}

impl RepeatGuard {
    fn observe(&mut self, call: &crate::model::ToolCall, result: &str, changes: usize) -> bool {
        let fingerprint = format!(
            "{}\n{}\n{}",
            call.name,
            canonical_json(&call.arguments),
            result
        );
        if self.fingerprint.as_deref() == Some(&fingerprint) && self.changes == changes {
            self.streak += 1;
        } else {
            self.fingerprint = Some(fingerprint);
            self.changes = changes;
            self.streak = 1;
        }
        self.streak >= 3
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let fields = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(key.clone()),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{fields}}}")
        }
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => value.to_string(),
    }
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
    verifier: Option<Verifier>,
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
            verifier: None,
            cancel,
        }
    }

    /// Attach a completion-gate verifier. Its commands run (bypassing approval)
    /// whenever the model stops with edits present.
    pub fn with_verifier(mut self, verifier: Verifier) -> Self {
        self.verifier = Some(verifier);
        self
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
        let mut repeats = RepeatGuard::default();
        loop {
            if step >= self.max_steps {
                return Outcome::StepLimit;
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Outcome::Cancelled;
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
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            eprintln!(
                                "● verify {}  {}",
                                r.command,
                                if r.passed { "passed" } else { "failed" }
                            );
                        }
                        if !report.all_passed() {
                            self.messages.push(Message::user(report.observation()));
                            step += 1;
                            continue;
                        }
                    }
                }
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                if self.cancel.load(Ordering::SeqCst) {
                    return Outcome::Cancelled;
                }
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
                if self.cancel.load(Ordering::SeqCst) {
                    return Outcome::Cancelled;
                }
                eprintln!("● {}  {}", call.name, out.summary);
                if repeats.observe(call, &out.content, self.cx.changes().len()) {
                    self.messages
                        .push(Message::tool_result(&call.id, out.content));
                    return Outcome::RepeatedAction;
                }
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

    struct StaticNamed {
        name: &'static str,
        content: &'static str,
    }

    struct CancelOnRun {
        token: CancelToken,
    }

    impl Tool for CancelOnRun {
        fn name(&self) -> &str {
            "read_file"
        }

        fn description(&self) -> &str {
            "cancels the run"
        }

        fn schema(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }

        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            self.token.store(true, Ordering::SeqCst);
            Ok(ToolOutput::new("cancelled", "cancelled"))
        }
    }

    impl Tool for StaticNamed {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "returns static content"
        }

        fn schema(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }

        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            Ok(ToolOutput::new(self.content, "same"))
        }
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
    fn pre_cancelled_agent_stops_before_model_call() {
        let token = cancel_token();
        token.store(true, Ordering::SeqCst);
        let mut a = Agent::new(
            Box::new(Scripted::new(vec![text("unused")])),
            "sys",
            10,
            PathBuf::from("."),
            token,
            ApprovalMode::NonInteractive,
        );
        assert!(matches!(a.run("hi"), Outcome::Cancelled));
    }

    #[test]
    fn three_identical_no_change_observations_stop() {
        let replies = vec![
            wants_tool("read_file"),
            wants_tool("read_file"),
            wants_tool("read_file"),
        ];
        let mut a = configured_agent(Scripted::new(replies), ApprovalMode::NonInteractive)
            .register(Box::new(StaticNamed {
                name: "read_file",
                content: "same",
            }));
        assert!(matches!(a.run("hi"), Outcome::RepeatedAction));
    }

    #[test]
    fn file_change_resets_repeat_streak() {
        let mut guard = RepeatGuard::default();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({"path":"a"}),
        };
        assert!(!guard.observe(&call, "same", 0));
        assert!(!guard.observe(&call, "same", 0));
        assert!(!guard.observe(&call, "same", 1));
        assert!(!guard.observe(&call, "same", 1));
        assert!(guard.observe(&call, "same", 1));
    }

    #[test]
    fn fingerprint_uses_canonical_nested_json_and_result() {
        let mut guard = RepeatGuard::default();
        let first = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::from_str(r#"{"outer":{"b":2,"a":1},"path":"a"}"#).unwrap(),
        };
        let reordered = ToolCall {
            id: "2".into(),
            name: "read_file".into(),
            arguments: serde_json::from_str(r#"{"path":"a","outer":{"a":1,"b":2}}"#).unwrap(),
        };
        assert!(!guard.observe(&first, "same", 0));
        assert!(!guard.observe(&reordered, "same", 0));
        assert!(guard.observe(&first, "same", 0));
        assert!(!guard.observe(&first, "different", 0));
    }

    #[test]
    fn repeated_denials_are_guarded() {
        let replies = vec![
            wants_tool("custom"),
            wants_tool("custom"),
            wants_tool("custom"),
        ];
        let mut a = configured_agent(Scripted::new(replies), ApprovalMode::AutoApprove);
        assert!(matches!(a.run("hi"), Outcome::RepeatedAction));
    }

    #[test]
    fn cancellation_set_during_dispatch_stops_immediately() {
        let token = cancel_token();
        let call = AssistantMessage {
            content: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "1".into(),
                    name: "read_file".into(),
                    arguments: json!({}),
                },
                ToolCall {
                    id: "2".into(),
                    name: "read_file".into(),
                    arguments: json!({}),
                },
            ],
            finish_reason: "tool_calls".into(),
        };
        let mut a = Agent::new(
            Box::new(Scripted::new(vec![call])),
            "sys",
            10,
            PathBuf::from("."),
            token.clone(),
            ApprovalMode::NonInteractive,
        )
        .register(Box::new(CancelOnRun { token }));
        assert!(matches!(a.run("hi"), Outcome::Cancelled));
    }

    #[test]
    fn verify_gate_passes_returns_complete() {
        use crate::tools::fs::WriteFile;
        let dir = tempfile::tempdir().unwrap();
        let write = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a.txt","content":"hi\n"}),
            }],
            finish_reason: "tool_calls".into(),
        };
        let model = Scripted::new(vec![write, text("done")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            10,
            dir.path().to_path_buf(),
            cancel_token(),
            ApprovalMode::AutoApprove,
        )
        .register(Box::new(WriteFile))
        .with_verifier(crate::verify::Verifier::new(vec!["exit 0".into()]));
        match a.run("edit") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after passing verification"),
        }
    }

    #[test]
    fn verify_gate_failure_loops_until_step_limit() {
        use crate::tools::fs::WriteFile;
        let dir = tempfile::tempdir().unwrap();
        let write = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a.txt","content":"hi\n"}),
            }],
            finish_reason: "tool_calls".into(),
        };
        // After the edit the model keeps trying to stop; the failing gate
        // re-prompts each time until max_steps is hit.
        let model = Scripted::new(vec![write, text("done"), text("still"), text("more")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            3,
            dir.path().to_path_buf(),
            cancel_token(),
            ApprovalMode::AutoApprove,
        )
        .register(Box::new(WriteFile))
        .with_verifier(crate::verify::Verifier::new(vec!["exit 1".into()]));
        assert!(matches!(a.run("edit"), Outcome::StepLimit));
    }

    #[test]
    fn verify_gate_skipped_without_edits() {
        let model = Scripted::new(vec![text("hi")]);
        let mut a = configured_agent(model, ApprovalMode::NonInteractive)
            .with_verifier(crate::verify::Verifier::new(vec!["exit 1".into()]));
        match a.run("nothing to change") {
            Outcome::Complete(s) => assert_eq!(s, "hi"),
            _ => panic!("no edits means the gate must not run"),
        }
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
