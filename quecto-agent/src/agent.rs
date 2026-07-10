use crate::model::{Message, Model};
use crate::BoxErr;

/// Terminal state of an agent run.
pub enum Outcome {
    Complete(String),
    StepLimit,
    Error(BoxErr),
}

/// The agent loop. Milestone 1: reason -> (no tools yet) -> answer.
pub struct Agent {
    model: Box<dyn Model>,
    messages: Vec<Message>,
    max_steps: usize,
}

impl Agent {
    /// Create an agent with a model, a system prompt, and a step limit.
    pub fn new(model: Box<dyn Model>, system: impl Into<String>, max_steps: usize) -> Self {
        Agent {
            model,
            messages: vec![Message::system(system.into())],
            max_steps,
        }
    }

    /// Run one task to completion (or a limit/error). Appends the task as a user
    /// message and loops: call the model, record its reply, finish when it stops
    /// requesting tools. No tools are registered in M1, so any tool call is
    /// reported back as an error observation and the loop continues.
    pub fn run(&mut self, task: &str) -> Outcome {
        self.messages.push(Message::user(task));
        let mut step = 0;
        loop {
            if step >= self.max_steps {
                return Outcome::StepLimit;
            }
            let msg = match self.model.complete(&self.messages) {
                Ok(m) => m,
                Err(e) => return Outcome::Error(e),
            };
            self.messages.push(Message::assistant(msg.content.clone()));
            if msg.tool_calls.is_empty() {
                return Outcome::Complete(msg.content);
            }
            for call in &msg.tool_calls {
                self.messages.push(Message::tool(format!(
                    "error: tool '{}' is not available",
                    call.name
                )));
            }
            step += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantMessage, ToolCall};
    use serde_json::json;
    use std::sync::Mutex;

    /// A fake model that returns pre-scripted replies in order.
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
        fn complete(&self, _messages: &[Message]) -> Result<AssistantMessage, BoxErr> {
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

    #[test]
    fn completes_on_text_only_reply() {
        let m = Scripted::new(vec![text("hello")]);
        let mut a = Agent::new(Box::new(m), "sys", 10);
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn unknown_tool_is_reported_then_completes() {
        // No tools registered in M1: the tool call is answered with an error
        // observation, and the model's next (text) reply completes the run.
        let m = Scripted::new(vec![wants_tool("read_file"), text("done")]);
        let mut a = Agent::new(Box::new(m), "sys", 10);
        match a.run("hi") {
            Outcome::Complete(s) => assert_eq!(s, "done"),
            _ => panic!("expected Complete after error observation"),
        }
    }

    #[test]
    fn step_limit_stops_a_spinning_model() {
        let m = Scripted::new(vec![wants_tool("x"), wants_tool("x"), wants_tool("x")]);
        let mut a = Agent::new(Box::new(m), "sys", 2);
        assert!(matches!(a.run("hi"), Outcome::StepLimit));
    }
}
