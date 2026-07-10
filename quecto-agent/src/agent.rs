use crate::model::{Message, Model};
use crate::tools::{builtin_tools, Context, Registry, Tool};
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
}

impl Agent {
    /// Create an agent with a model, a system prompt, a step limit, and the
    /// repository root that filesystem tools are scoped to.
    pub fn new(
        model: Box<dyn Model>,
        system: impl Into<String>,
        max_steps: usize,
        repo_root: PathBuf,
    ) -> Self {
        Agent {
            model,
            registry: Registry::new(),
            cx: Context::new(repo_root),
            messages: vec![Message::system(system.into())],
            max_steps,
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
            self.messages
                .push(Message::assistant_with_calls(msg.content.clone(), msg.tool_calls.clone()));
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                let out = self.registry.dispatch(call, &mut self.cx);
                eprintln!("● {}  {}", call.name, out.summary);
                self.messages.push(Message::tool_result(&call.id, out.content));
            }
            step += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantMessage, ToolCall};
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
            Scripted { replies: Mutex::new(replies) }
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

    fn agent(model: Scripted) -> Agent {
        Agent::new(Box::new(model), "sys", 10, PathBuf::from("."))
    }

    struct Recording {
        ran: Arc<AtomicBool>,
    }

    impl Tool for Recording {
        fn name(&self) -> &str {
            "rec"
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
        let model = Scripted::new(vec![wants_tool("rec"), text("done")]);
        let mut a = agent(model).register(Box::new(Recording { ran: ran.clone() }));
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete"),
        }
        assert!(ran.load(Ordering::SeqCst), "the tool should have been dispatched");
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
        let mut a = Agent::new(Box::new(model), "sys", 2, PathBuf::from("."));
        assert!(matches!(a.run("hi"), Outcome::StepLimit));
    }
}
