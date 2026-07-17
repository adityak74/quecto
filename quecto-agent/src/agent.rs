use crate::approval::ApprovalMode;
use crate::model::{Message, MessageMetadata, MessageRecord, Model};
use crate::policy::{Decision, Policy};
use crate::render::{stderr_renderer, Renderer};
use crate::sandbox::CancelToken;
use crate::tools::{Context, FileChange, Registry, Tool, ToolOutput};
use crate::verify::Verifier;
use crate::BoxErr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    VerificationFailed {
        attempts: usize,
    },
    Cancelled,
    RepeatedAction,
    /// Stopped early because several consecutive actions were denied by policy
    /// or approval, so the run cannot make progress unattended.
    Blocked,
    Error(BoxErr),
}

const VERIFY_NO_PROGRESS_ATTEMPTS: usize = 3;

/// Consecutive denied tool results that end a run early with `Outcome::Blocked`.
/// Distinct from the repeat guard: denials that vary in tool/arguments never
/// trip the repeat guard, but a run that keeps hitting the approval wall should
/// still stop promptly instead of grinding to the step limit.
const DENIAL_STREAK_LIMIT: usize = 3;

/// Consecutive identical tool call + result + change-count observations that
/// trip the repeat guard and end a run early.
const REPEAT_STREAK_LIMIT: usize = 3;

/// Receives the transcript and file mutations of a run in order, for
/// persistence. Recording is best-effort and must never fail the run.
pub trait RunRecorder: Send {
    fn message(&mut self, m: &Message);

    fn message_with_metadata(&mut self, m: &Message, _metadata: &MessageMetadata) {
        self.message(m);
    }

    fn change(&mut self, c: &FileChange);
}

#[derive(Default)]
struct RepeatGuard {
    fingerprint: Option<String>,
    changes: usize,
    streak: usize,
}

impl RepeatGuard {
    fn observe(&mut self, call: &crate::model::ToolCall, result: &str, changes: usize) -> bool {
        let fingerprint = format!("{}\n{}\n{}", call.name, call.arguments, result);
        if self.fingerprint.as_deref() == Some(&fingerprint) && self.changes == changes {
            self.streak += 1;
        } else {
            self.fingerprint = Some(fingerprint);
            self.changes = changes;
            self.streak = 1;
        }
        self.streak >= REPEAT_STREAK_LIMIT
    }
}

/// The agent loop: reason -> call read-only tools -> observe -> answer.
pub struct Agent {
    model: Box<dyn Model>,
    registry: Registry,
    cx: Context,
    pub messages: Vec<Message>,
    message_metadata: Vec<MessageMetadata>,
    max_steps: usize,
    policy: Policy,
    approval: ApprovalMode,
    verifier: Option<Verifier>,
    recorder: Option<Box<dyn RunRecorder>>,
    recorded_messages: usize,
    recorded_changes: usize,
    renderer: Box<dyn Renderer>,
    cancel: CancelToken,
}

#[derive(Clone)]
pub struct AgentConfig {
    pub model: Box<dyn Model>,
    pub base_system_prompt: String,
    pub max_steps: usize,
    pub repo_root: PathBuf,
    pub cancel: CancelToken,
    pub approval: ApprovalMode,
}

impl Agent {
    pub fn config(&self) -> AgentConfig {
        AgentConfig {
            model: self.model.clone(),
            base_system_prompt: self.messages.first().map(|m| m.content.clone()).unwrap_or_default(),
            max_steps: self.max_steps,
            repo_root: self.cx.repo_root.clone(),
            cancel: self.cancel.clone(),
            approval: self.approval.clone(),
        }
    }

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
            message_metadata: vec![MessageMetadata::default()],
            max_steps,
            policy: Policy::default(),
            approval,
            verifier: None,
            recorder: None,
            recorded_messages: 0,
            recorded_changes: 0,
            renderer: stderr_renderer(),
            cancel,
        }
    }

    /// Attach a completion-gate verifier. Its commands run (bypassing approval)
    /// whenever the model stops with edits present.
    pub fn with_verifier(mut self, verifier: Verifier) -> Self {
        self.verifier = Some(verifier);
        self
    }

    /// Replace the approval policy (default: read-only preset).
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Attach a recorder for session persistence.
    pub fn with_recorder(mut self, recorder: Box<dyn RunRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Replace the activity renderer (default: plain stderr).
    pub fn with_renderer(mut self, renderer: Box<dyn Renderer>) -> Self {
        self.renderer = renderer;
        self
    }

    /// Change the approval mode mid-session (used by the chat REPL).
    pub fn set_approval(&mut self, approval: ApprovalMode) {
        self.approval = approval;
    }

    /// Drop the conversation history, keeping only the system message. The
    /// recording cursor is reset so a fresh turn records from the new baseline.
    pub fn clear_history(&mut self) {
        self.messages.truncate(1);
        self.message_metadata.truncate(1);
        self.recorded_messages = self.messages.len();
        self.recorded_changes = 0;
        self.cx.clear_changes();
    }

    /// Replace the seed transcript (used by `resume`). The provided messages are
    /// treated as already recorded so `resume` only persists new turns.
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.recorded_messages = messages.len();
        self.message_metadata = vec![MessageMetadata::default(); messages.len()];
        self.messages = messages;
        self
    }

    /// Replace the seed transcript together with additive persistence metadata.
    pub fn with_message_records(mut self, records: Vec<MessageRecord>) -> Self {
        self.recorded_messages = records.len();
        self.messages = records
            .iter()
            .map(|record| record.message.clone())
            .collect();
        self.message_metadata = records
            .into_iter()
            .map(|record| record.metadata)
            .collect();
        self
    }

    /// Return additive metadata associated with a transcript message.
    pub fn message_metadata(&self, index: usize) -> Option<&MessageMetadata> {
        self.message_metadata.get(index)
    }

    fn push_message(&mut self, message: Message, metadata: MessageMetadata) {
        self.message_metadata
            .resize(self.messages.len(), MessageMetadata::default());
        self.messages.push(message);
        self.message_metadata.push(metadata);
    }

    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        self.registry.register(tool);
        self
    }

    pub fn register_builtins(self) -> Self {
        self.register_builtins_filtered(None)
    }

    /// Register the built-in tools filtered by an allow-list (`None` = all).
    pub fn register_builtins_filtered(mut self, enabled: Option<&[String]>) -> Self {
        for tool in crate::tools::builtin_tools_filtered(enabled) {
            self.registry.register(tool);
        }
        if enabled.map_or(true, |list| list.iter().any(|n| n == "invoke_subagent")) {
            let subagent_tool = crate::tools::subagent::InvokeSubagent::new(self.config());
            self.registry.register(Box::new(subagent_tool));
        }
        self
    }

    pub fn background_process_count(&mut self) -> usize {
        self.cx.background_process_count()
    }

    /// Return the names of registered tools (used by /commands in chat).
    pub fn tool_names(&self) -> Vec<String> {
        self.registry.tool_names()
    }

    /// Run one task to completion (or a limit/error). Appends the task as a user
    /// message and loops: call the model with the available tool schemas, execute
    /// any tool calls, feed results back, and finish when the model stops
    /// requesting tools. Unknown tools are reported back as an error observation.
    pub fn run(&mut self, task: &str) -> Outcome {
        #[cfg(feature = "otel")]
        let redacted_task = crate::sandbox::redact_secrets(task);
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "agent_run",
            quecto.task = redacted_task.as_str(),
            quecto.max_steps = self.max_steps
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

        self.push_message(Message::user(task), MessageMetadata::default());
        self.run_loop()
    }

    /// Continue a seeded transcript (from `with_messages`) without appending a
    /// new task.
    pub fn resume(&mut self) -> Outcome {
        #[cfg(feature = "otel")]
        let span = tracing::span!(
            tracing::Level::INFO,
            "agent_run",
            quecto.max_steps = self.max_steps
        );
        #[cfg(feature = "otel")]
        let _guard = span.enter();

        self.run_loop()
    }

    /// Flush any newly-appended messages and file changes to the recorder.
    fn sync(&mut self) {
        if self.recorder.is_none() {
            return;
        }
        while self.recorded_messages < self.messages.len() {
            let m = self.messages[self.recorded_messages].clone();
            let metadata = self
                .message_metadata
                .get(self.recorded_messages)
                .cloned()
                .unwrap_or_default();
            if let Some(r) = self.recorder.as_mut() {
                r.message_with_metadata(&m, &metadata);
            }
            self.recorded_messages += 1;
        }
        while self.recorded_changes < self.cx.changes().len() {
            let c = self.cx.changes()[self.recorded_changes].clone();
            if let Some(r) = self.recorder.as_mut() {
                r.change(&c);
            }
            self.recorded_changes += 1;
        }
    }

    fn run_loop(&mut self) -> Outcome {
        let schemas = self.registry.schemas();
        let mut step = 0;
        let mut repeats = RepeatGuard::default();
        let mut failed_verify_changes: Option<usize> = None;
        let mut failed_verify_attempts = 0;
        let mut denial_streak = 0usize;
        let outcome = loop {
            self.sync();
            if step >= self.max_steps {
                break Outcome::StepLimit;
            }
            if self.cancel.load(Ordering::SeqCst) {
                break Outcome::Cancelled;
            }

            #[cfg(feature = "otel")]
            let step_span = tracing::span!(
                tracing::Level::INFO,
                "agent_step",
                quecto.step_number = step
            );
            #[cfg(feature = "otel")]
            let _step_guard = step_span.enter();

            self.renderer.working();
            let completed = self.model.complete_with_options(
                &self.messages,
                &schemas,
                &crate::reasoning::CompletionOptions::default(),
            );
            self.renderer.working_done();
            let completion = match completed {
                Ok(completion) => completion,
                Err(e) => break Outcome::Error(e),
            };
            let msg = completion.message;
            let telemetry = completion.telemetry;
            let mut assistant_msg = Message::assistant_with_calls(
                msg.content.clone(),
                msg.tool_calls.clone(),
            );
            assistant_msg.reasoning_content = msg.reasoning_content.clone();
            let metadata = MessageMetadata::from(&telemetry);
            self.push_message(assistant_msg, metadata);

            if msg.tool_calls.is_empty() {
                if let Some(verifier) = &self.verifier {
                    if !verifier.is_empty() && !self.cx.changes().is_empty() {
                        let report = verifier.run(&self.cx);
                        for r in &report.results {
                            self.renderer.verify(&r.command, r.passed);
                        }
                        if !report.all_passed() {
                            let changes = self.cx.changes().len();
                            if failed_verify_changes == Some(changes) {
                                failed_verify_attempts += 1;
                            } else {
                                failed_verify_changes = Some(changes);
                                failed_verify_attempts = 1;
                            }
                            if failed_verify_attempts >= VERIFY_NO_PROGRESS_ATTEMPTS {
                                break Outcome::VerificationFailed {
                                    attempts: failed_verify_attempts,
                                };
                            }
                            self.push_message(
                                Message::user(report.observation()),
                                MessageMetadata::default(),
                            );
                            step += 1;
                            continue;
                        }
                    }
                }
                break Outcome::Complete(msg.content);
            }
            let mut stop: Option<Outcome> = None;
            for call in &msg.tool_calls {
                if self.cancel.load(Ordering::SeqCst) {
                    stop = Some(Outcome::Cancelled);
                    break;
                }

                #[cfg(feature = "otel")]
                let sanitized_args = sanitize_arguments(&call.name, &call.arguments);
                #[cfg(feature = "otel")]
                let redacted_args = crate::sandbox::redact_secrets(&sanitized_args);
                #[cfg(feature = "otel")]
                let tool_span = tracing::span!(
                    tracing::Level::INFO,
                    "tool_execute",
                    quecto.tool_name = call.name.as_str(),
                    quecto.tool_arguments = %redacted_args,
                    quecto.tool_summary = tracing::field::Empty
                );
                #[cfg(feature = "otel")]
                let _tool_guard = tool_span.enter();

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
                    stop = Some(Outcome::Cancelled);
                    break;
                }
                let display_name = match call.arguments.get("command").and_then(|v| v.as_str()) {
                    Some(cmd)
                        if matches!(
                            call.name.as_str(),
                            "run_command" | "start_background_process"
                        ) =>
                    {
                        format!("{}({cmd})", call.name)
                    }
                    _ => call.name.clone(),
                };
                self.renderer.tool(&display_name, &out.summary);

                #[cfg(feature = "otel")]
                {
                    tool_span.record("quecto.tool_summary", &out.summary);
                    let redacted_out = crate::sandbox::redact_secrets(&out.content);
                    tracing::event!(tracing::Level::INFO, name = "tool_output", content = %redacted_out);
                }

                if out.summary == "denied" {
                    denial_streak += 1;
                } else {
                    denial_streak = 0;
                }
                let repeated = repeats.observe(call, &out.content, self.cx.changes().len());
                self.push_message(
                    Message::tool_result(&call.id, out.content),
                    MessageMetadata::default(),
                );
                if repeated {
                    stop = Some(Outcome::RepeatedAction);
                    break;
                }
                if denial_streak >= DENIAL_STREAK_LIMIT {
                    stop = Some(Outcome::Blocked);
                    break;
                }
            }
            if let Some(outcome) = stop {
                break outcome;
            }
            step += 1;
        };
        self.sync();
        outcome
    }
}

#[cfg(feature = "otel")]
fn sanitize_arguments(name: &str, args: &serde_json::Value) -> String {
    match name {
        "run_command" | "write_file" | "apply_patch" => {
            if let Some(obj) = args.as_object() {
                let mut map = serde_json::Map::new();
                for (k, v) in obj {
                    if k == "command" || k == "content" || k == "patch" {
                        map.insert(k.clone(), serde_json::Value::String("<redacted>".to_string()));
                    } else {
                        map.insert(k.clone(), v.clone());
                    }
                }
                serde_json::Value::Object(map).to_string()
            } else {
                args.to_string()
            }
        }
        _ => args.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalMode;
    use crate::model::{AssistantMessage, ModelCompletion, ToolCall};
    use crate::sandbox::cancel_token;
    use crate::tools::{Context, Tool, ToolOutput, ToolResult};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Scripted {
        replies: Arc<Mutex<Vec<ModelCompletion>>>,
    }
    impl Scripted {
        fn new(replies: Vec<AssistantMessage>) -> Self {
            Scripted {
                replies: Arc::new(Mutex::new(
                    replies.into_iter().map(ModelCompletion::from).collect(),
                )),
            }
        }

        fn new_with_completions(replies: Vec<ModelCompletion>) -> Self {
            Scripted {
                replies: Arc::new(Mutex::new(replies)),
            }
        }

        fn pop(&self) -> Result<ModelCompletion, BoxErr> {
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err("no more scripted replies".into());
            }
            Ok(replies.remove(0))
        }
    }
    impl Model for Scripted {
        fn clone_box(&self) -> Box<dyn Model> {
            Box::new(self.clone())
        }
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<AssistantMessage, BoxErr> {
            self.pop().map(|completion| completion.message)
        }

        fn complete_with_options(
            &self,
            _messages: &[Message],
            _tools: &[Value],
            _options: &crate::reasoning::CompletionOptions,
        ) -> Result<ModelCompletion, BoxErr> {
            self.pop()
        }
    }

    fn text(c: &str) -> AssistantMessage {
        AssistantMessage {
            content: c.to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            reasoning_content: None,
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
            reasoning_content: None,
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

    struct CaptureRenderer {
        tools: Arc<Mutex<Vec<String>>>,
        events: Arc<Mutex<Vec<String>>>,
    }
    impl crate::render::Renderer for CaptureRenderer {
        fn working(&mut self) {
            self.events.lock().unwrap().push("working".to_string());
        }
        fn working_done(&mut self) {
            self.events.lock().unwrap().push("working_done".to_string());
        }
        fn tool(&mut self, name: &str, summary: &str) {
            self.tools.lock().unwrap().push(format!("{name}:{summary}"));
        }
        fn verify(&mut self, _command: &str, _passed: bool) {}
        fn notice(&mut self, _text: &str) {}
        fn assistant(&mut self, _text: &str) {}
    }

    #[test]
    fn renderer_receives_tool_activity() {
        let tools = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let model = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        let mut a = agent(model)
            .register(Box::new(RecordingNamed {
                name: "read_file",
                ran: Arc::new(AtomicBool::new(false)),
            }))
            .with_renderer(Box::new(CaptureRenderer {
                tools: tools.clone(),
                events,
            }));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert_eq!(
            tools.lock().unwrap().clone(),
            vec!["read_file:ok".to_string()]
        );
    }

    #[test]
    fn model_call_brackets_renderer_working_state() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut a = Agent::new(
            Box::new(Scripted::new(vec![text("done")])),
            "sys",
            10,
            PathBuf::from("."),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
        .with_renderer(Box::new(CaptureRenderer {
            tools: Arc::new(Mutex::new(Vec::new())),
            events: events.clone(),
        }));

        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["working", "working_done"]
        );
    }

    #[test]
    fn model_error_stops_renderer_working_state() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut a = Agent::new(
            Box::new(Scripted::new(vec![])),
            "sys",
            10,
            PathBuf::from("."),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
        .with_renderer(Box::new(CaptureRenderer {
            tools: Arc::new(Mutex::new(Vec::new())),
            events: events.clone(),
        }));

        assert!(matches!(a.run("hi"), Outcome::Error(_)));
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["working", "working_done"]
        );
    }

    #[test]
    fn set_approval_switches_gate_behavior() {
        let ran = Arc::new(AtomicBool::new(false));
        let model = Scripted::new(vec![wants_tool("write_file"), text("done")]);
        let mut a = configured_agent(model, ApprovalMode::NonInteractive).register(Box::new(
            RecordingNamed {
                name: "write_file",
                ran: ran.clone(),
            },
        ));
        a.set_approval(ApprovalMode::AutoApprove);
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn clear_history_keeps_only_the_system_message() {
        let mut a = agent(Scripted::new(vec![text("done"), text("again")]));
        assert!(matches!(a.run("first"), Outcome::Complete(_)));
        a.clear_history();
        // Second run starts fresh from the system-only baseline and still completes.
        assert!(matches!(a.run("second"), Outcome::Complete(_)));
    }

    #[test]
    fn clear_history_resets_recorded_changes() {
        let mut a = agent(Scripted::new(vec![text("done")]));
        a.cx.record_change("foo.rs", None, "content".to_string());
        a.recorded_changes = 1;
        assert_eq!(a.cx.changes().len(), 1);
        assert_eq!(a.recorded_changes, 1);

        a.clear_history();
        assert_eq!(a.messages.len(), 1); // only system message
        assert_eq!(a.recorded_messages, 1);
        assert_eq!(a.cx.changes().len(), 0);
        assert_eq!(a.recorded_changes, 0);
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

    // This test is what makes it safe to rely on `serde_json::Value::to_string()` for
    // key-sorted output instead of a hand-rolled `canonical_json`: it fails immediately
    // if `serde_json/preserve_order` is ever enabled via feature unification.
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
    fn varying_denials_stop_early_with_blocked() {
        // Edits vary (write_file, apply_patch, write_file), so the repeat guard
        // never trips, but three consecutive approval denials should still stop
        // the run promptly instead of grinding to the step limit.
        let replies = vec![
            wants_tool("write_file"),
            wants_tool("apply_patch"),
            wants_tool("write_file"),
            wants_tool("write_file"),
        ];
        let mut a = configured_agent(Scripted::new(replies), ApprovalMode::NonInteractive);
        assert!(matches!(a.run("hi"), Outcome::Blocked));
    }

    #[test]
    fn a_successful_action_resets_the_denial_streak() {
        // Two denials, then an allowed read, then a text answer: the allowed
        // action resets the streak so the run completes normally.
        let replies = vec![
            wants_tool("write_file"),
            wants_tool("apply_patch"),
            wants_tool("read_file"),
            text("done"),
        ];
        let mut a = configured_agent(Scripted::new(replies), ApprovalMode::NonInteractive)
            .register(Box::new(RecordingNamed {
                name: "read_file",
                ran: Arc::new(AtomicBool::new(false)),
            }));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
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
            reasoning_content: None,
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
            reasoning_content: None,
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
    fn verify_gate_failure_stops_after_bounded_no_progress_attempts() {
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
            reasoning_content: None,
        };
        // After the edit the model keeps trying to stop; the failing gate
        // should stop cleanly before the step limit.
        let model = Scripted::new(vec![write, text("done"), text("still"), text("more")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            10,
            dir.path().to_path_buf(),
            cancel_token(),
            ApprovalMode::AutoApprove,
        )
        .register(Box::new(WriteFile))
        .with_verifier(crate::verify::Verifier::new(vec!["exit 1".into()]));
        assert!(matches!(
            a.run("edit"),
            Outcome::VerificationFailed {
                attempts: VERIFY_NO_PROGRESS_ATTEMPTS
            }
        ));
    }

    #[test]
    fn verify_gate_that_passes_after_an_edit_returns_complete() {
        use crate::tools::fs::WriteFile;
        let dir = tempfile::tempdir().unwrap();
        let write_bad = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a.txt","content":"bad\n"}),
            }],
            finish_reason: "tool_calls".into(),
            reasoning_content: None,
        };
        let write_good = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "2".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a.txt","content":"good\n"}),
            }],
            finish_reason: "tool_calls".into(),
            reasoning_content: None,
        };
        let model = Scripted::new(vec![write_bad, text("not yet"), write_good, text("done")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            10,
            dir.path().to_path_buf(),
            cancel_token(),
            ApprovalMode::AutoApprove,
        )
        .register(Box::new(WriteFile))
        .with_verifier(crate::verify::Verifier::new(vec![
            "grep -q good a.txt".into()
        ]));
        match a.run("edit") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after verification passes"),
        }
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

    #[derive(Default)]
    struct FakeRecorder {
        roles: Arc<Mutex<Vec<String>>>,
        changed: Arc<Mutex<Vec<String>>>,
    }
    impl RunRecorder for FakeRecorder {
        fn message(&mut self, m: &Message) {
            self.roles.lock().unwrap().push(m.role.clone());
        }
        fn change(&mut self, c: &FileChange) {
            self.changed.lock().unwrap().push(c.path.clone());
        }
    }

    #[test]
    fn recorder_captures_seed_task_and_turns() {
        let roles = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Mutex::new(Vec::new()));
        let model = Scripted::new(vec![text("done")]);
        let mut a = Agent::new(
            Box::new(model),
            "sys",
            10,
            PathBuf::from("."),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
        .with_recorder(Box::new(FakeRecorder {
            roles: roles.clone(),
            changed: changed.clone(),
        }));
        assert!(matches!(a.run("hi"), Outcome::Complete(_)));
        let got = roles.lock().unwrap().clone();
        assert_eq!(got, vec!["system", "user", "assistant"]);
        assert!(changed.lock().unwrap().is_empty());
    }

    #[test]
    fn recorder_captures_file_changes() {
        use crate::tools::fs::WriteFile;
        let changed = Arc::new(Mutex::new(Vec::new()));
        let dir = tempfile::tempdir().unwrap();
        let write = AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a.txt","content":"hi\n"}),
            }],
            finish_reason: "tool_calls".into(),
            reasoning_content: None,
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
        .with_recorder(Box::new(FakeRecorder {
            roles: Arc::new(Mutex::new(Vec::new())),
            changed: changed.clone(),
        }));
        assert!(matches!(a.run("edit"), Outcome::Complete(_)));
        assert_eq!(changed.lock().unwrap().clone(), vec!["a.txt".to_string()]);
    }

    #[test]
    fn resume_continues_a_seeded_transcript_without_re_recording() {
        let roles = Arc::new(Mutex::new(Vec::new()));
        let seed = vec![
            Message::system("sys"),
            Message::user("original"),
            Message::assistant_with_calls("partial", vec![]),
        ];
        let model = Scripted::new(vec![text("resumed")]);
        let mut a = Agent::new(
            Box::new(model),
            "unused",
            10,
            PathBuf::from("."),
            cancel_token(),
            ApprovalMode::NonInteractive,
        )
        .with_messages(seed)
        .with_recorder(Box::new(FakeRecorder {
            roles: roles.clone(),
            changed: Arc::new(Mutex::new(Vec::new())),
        }));
        match a.resume() {
            Outcome::Complete(s) => assert_eq!(s, "resumed"),
            _ => panic!("expected Complete"),
        }
        // Only the new assistant turn is recorded; the three seeded messages are not.
        assert_eq!(roles.lock().unwrap().clone(), vec!["assistant"]);
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
            reasoning_content: None,
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

    #[test]
    fn propagates_reasoning_content() {
        let msg = AssistantMessage {
            content: "hello".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            reasoning_content: Some("I am thinking".to_string()),
        };
        let model = Scripted::new(vec![msg]);
        let mut a = agent(model);
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Complete"),
        }
        assert_eq!(a.messages.len(), 3);
        assert_eq!(
            a.messages[2].reasoning_content,
            Some("I am thinking".to_string())
        );
    }

    #[test]
    fn propagates_completion_reasoning_metadata() {
        let model = Scripted::new_with_completions(vec![ModelCompletion {
            message: AssistantMessage {
                content: "done".to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: Some("thinking".to_string()),
            },
            telemetry: crate::reasoning::CompletionTelemetry {
                requested_reasoning_mode: Some(crate::reasoning::ReasoningMode::High),
                provider_reasoning_parameters: Some(json!({"reasoning_effort": "high"})),
                reasoning_parameters_sent: true,
                reasoning_content_available: true,
                actual_reasoning_tokens: Some(17),
            },
        }]);
        let mut a = configured_agent(model, ApprovalMode::NonInteractive);
        let _ = a.run("task");
        let metadata = a.message_metadata(2).unwrap();
        assert_eq!(metadata.actual_reasoning_tokens, Some(17));
        assert_eq!(
            metadata.requested_reasoning_mode,
            Some(crate::reasoning::ReasoningMode::High)
        );
    }

    #[test]
    fn direct_legacy_message_append_does_not_shift_completion_metadata() {
        let model = Scripted::new_with_completions(vec![ModelCompletion {
            message: text("done"),
            telemetry: crate::reasoning::CompletionTelemetry {
                actual_reasoning_tokens: Some(17),
                ..crate::reasoning::CompletionTelemetry::default()
            },
        }]);
        let mut agent = configured_agent(model, ApprovalMode::NonInteractive);
        agent.messages.push(Message::user("legacy append"));

        let _ = agent.run("task");

        assert_eq!(
            agent.message_metadata(3).unwrap().actual_reasoning_tokens,
            Some(17)
        );
    }
}
