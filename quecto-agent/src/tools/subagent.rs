use crate::agent::{Agent, AgentConfig, Outcome};
use crate::tools::{Context, Tool, ToolError, ToolOutput, ToolResult};
use serde_json::{json, Value};

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
        
        let system_prompt = if role != "Subagent" {
            format!("{}\n\nYou are acting as a specialized subagent: {}", self.config.base_system_prompt, role)
        } else {
            self.config.base_system_prompt.clone()
        };

        let mut subagent = Agent::new(
            self.config.model.clone(),
            system_prompt,
            self.config.max_steps,
            self.config.repo_root.clone(),
            self.config.cancel.clone(),
            self.config.approval.clone(),
        ).register_builtins();
        
        subagent = subagent.register(Box::new(self.clone()));
        
        match subagent.run(prompt) {
            Outcome::Complete(result) => {
                Ok(ToolOutput::new(
                    format!("Subagent completed successfully:\n{}", result),
                    "subagent finished"
                ))
            }
            Outcome::StepLimit => {
                Ok(ToolOutput::new("Subagent stopped: Step limit reached", "step limit"))
            }
            Outcome::VerificationFailed { attempts } => {
                Ok(ToolOutput::new(format!("Subagent stopped: Verification failed after {} attempts", attempts), "verification failed"))
            }
            Outcome::Cancelled => {
                Ok(ToolOutput::new("Subagent was cancelled", "cancelled"))
            }
            Outcome::RepeatedAction => {
                Ok(ToolOutput::new("Subagent stopped: Repeated action detected", "repeated action"))
            }
            Outcome::Blocked => {
                Ok(ToolOutput::new("Subagent stopped: Blocked by policy or approval", "blocked"))
            }
            Outcome::Error(e) => {
                Err(ToolError::new(format!("Subagent error: {}", e)))
            }
        }
    }
}
