use crate::agent::{Agent, AgentConfig, Outcome};
use crate::model::Message;
use crate::tools::{Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

const SUBAGENT_DIRECTIVE: &str = "You are a subagent delegated a single, bounded task by another agent. \
Work autonomously: there is no user to ask for clarification, so pick the most direct tool for the goal \
and act on it rather than re-checking the same state repeatedly. Stop as soon as you have completed the \
task or concluded it cannot be completed, and report back concisely.";

/// Render the last few transcript entries (assistant tool calls and their
/// results) so a caller can see what a subagent actually attempted even when
/// it didn't reach `Outcome::Complete`. Without this, a stalled subagent's
/// failure is indistinguishable from it never having tried anything.
fn progress_summary(messages: &[Message], limit: usize) -> Option<String> {
    let mut lines = Vec::new();
    for m in messages {
        match m.role.as_str() {
            "assistant" => {
                for call in &m.tool_calls {
                    lines.push(format!("called {}({})", call.name, call.arguments));
                }
            }
            "tool" => {
                let snippet: String = m.content.chars().take(160).collect();
                lines.push(format!("-> {}", snippet));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(limit);
    Some(lines[start..].join("\n"))
}

#[derive(Clone)]
pub struct InvokeSubagent {
    pub config: AgentConfig,
}

impl InvokeSubagent {
    pub fn new(config: AgentConfig) -> Self {
        InvokeSubagent { config }
    }
}

impl Tool for InvokeSubagent {
    fn name(&self) -> &str {
        "invoke_subagent"
    }

    fn description(&self) -> &str {
        "Delegates a task to a subagent running in the same repository. Use this to branch off complex sub-tasks."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The instruction or task description for the subagent to complete."
                },
                "role": {
                    "type": "string",
                    "description": "Optional specific role or persona for the subagent (e.g., Debugger, Researcher)."
                }
            },
            "required": ["prompt"]
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("missing \"prompt\" parameter"))?;
        
        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("Subagent");
        
        let mut system_prompt = format!("{}\n\n{}", SUBAGENT_DIRECTIVE, self.config.base_system_prompt);
        if role != "Subagent" {
            system_prompt.push_str(&format!("\n\nYou are acting as a specialized subagent: {}", role));
        }

        let mut subagent = Agent::new(
            self.config.model.clone(),
            system_prompt,
            self.config.max_steps,
            self.config.repo_root.clone(),
            self.config.cancel.clone(),
            self.config.approval.clone(),
        ).register_builtins();

        subagent = subagent.register(Box::new(self.clone()));

        let outcome = subagent.run(prompt);
        let progress = || progress_summary(&subagent.messages, 6);
        match outcome {
            Outcome::Complete(result) => {
                Ok(ToolOutput::new(
                    format!("Subagent completed successfully:\n{}", result),
                    "subagent finished"
                ))
            }
            Outcome::StepLimit => {
                let mut msg = "Subagent stopped: step limit reached before finishing.".to_string();
                if let Some(p) = progress() {
                    msg.push_str("\nLast steps attempted:\n");
                    msg.push_str(&p);
                }
                Ok(ToolOutput::new(msg, "step limit"))
            }
            Outcome::VerificationFailed { attempts } => {
                Ok(ToolOutput::new(format!("Subagent stopped: Verification failed after {} attempts", attempts), "verification failed"))
            }
            Outcome::Cancelled => {
                Ok(ToolOutput::new("Subagent was cancelled", "cancelled"))
            }
            Outcome::RepeatedAction => {
                let mut msg = "Subagent stopped: it got stuck repeating the same tool call without making progress.".to_string();
                if let Some(p) = progress() {
                    msg.push_str("\nLast steps attempted:\n");
                    msg.push_str(&p);
                }
                msg.push_str("\nConsider retrying with a narrower, more concrete prompt, or completing this task directly.");
                Ok(ToolOutput::new(msg, "repeated action"))
            }
            Outcome::Blocked => {
                let mut msg = "Subagent stopped: blocked by policy or approval.".to_string();
                if let Some(p) = progress() {
                    msg.push_str("\nLast steps attempted:\n");
                    msg.push_str(&p);
                }
                Ok(ToolOutput::new(msg, "blocked"))
            }
            Outcome::Error(e) => {
                Err(ToolError::new(format!("Subagent error: {}", e)))
            }
        }
    }
}
