pub mod fs;
pub mod git;
pub mod search;

use crate::model::ToolCall;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ToolOutput {
    pub content: String,
    pub summary: String,
}

impl ToolOutput {
    pub fn new(content: impl Into<String>, summary: impl Into<String>) -> Self {
        ToolOutput {
            content: content.into(),
            summary: summary.into(),
        }
    }
}

#[derive(Debug)]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError {
            message: message.into(),
        }
    }
}

pub type ToolResult = Result<ToolOutput, ToolError>;

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult;
}

pub struct Context {
    pub repo_root: PathBuf,
}

impl Context {
    pub fn new(repo_root: PathBuf) -> Self {
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        Context { repo_root }
    }

    pub fn resolve_existing(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let canon = self
            .repo_root
            .join(rel)
            .canonicalize()
            .map_err(|e| ToolError::new(format!("{rel}: {e}")))?;
        if !canon.starts_with(&self.repo_root) {
            return Err(ToolError::new(format!(
                "path '{rel}' escapes the repository root"
            )));
        }
        Ok(canon)
    }
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema()
                    }
                })
            })
            .collect()
    }

    pub fn dispatch(&self, call: &ToolCall, cx: &mut Context) -> ToolOutput {
        match self.tools.iter().find(|t| t.name() == call.name) {
            None => ToolOutput::new(
                format!("error: tool '{}' is not available", call.name),
                "unknown tool",
            ),
            Some(t) => match t.run(&call.arguments, cx) {
                Ok(out) => out,
                Err(e) => ToolOutput::new(format!("error: {}", e.message), "error"),
            },
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::ReadFile),
        Box::new(fs::ListFiles),
        Box::new(search::SearchText),
        Box::new(git::GitDiff),
        Box::new(git::GitStatus),
    ]
}

pub fn cap_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… {} bytes truncated …]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "echoes its text arg"
        }

        fn schema(&self) -> Value {
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
        }

        fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
            let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolOutput::new(t.to_string(), "echoed"))
        }
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn schemas_wrap_each_tool() {
        let mut r = Registry::new();
        r.register(Box::new(Echo));
        let s = r.schemas();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["type"], "function");
        assert_eq!(s[0]["function"]["name"], "echo");
        assert!(s[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn dispatch_routes_to_tool() {
        let mut r = Registry::new();
        r.register(Box::new(Echo));
        let mut cx = Context::new(PathBuf::from("."));
        let out = r.dispatch(&call("echo", json!({"text":"hi"})), &mut cx);
        assert_eq!(out.content, "hi");
    }

    #[test]
    fn dispatch_unknown_tool_is_error_output() {
        let r = Registry::new();
        let mut cx = Context::new(PathBuf::from("."));
        let out = r.dispatch(&call("nope", json!({})), &mut cx);
        assert!(out.content.contains("not available"));
    }

    #[test]
    fn resolve_rejects_escape() {
        let cx = Context::new(PathBuf::from("."));
        assert!(cx.resolve_existing("../../../etc/passwd").is_err());
    }

    #[test]
    fn cap_output_truncates() {
        let big = "x".repeat(100);
        let capped = cap_output(&big, 10);
        assert!(capped.len() < big.len());
        assert!(capped.contains("truncated"));
    }
}
