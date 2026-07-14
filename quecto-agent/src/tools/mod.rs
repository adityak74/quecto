pub mod fs;
pub mod git;
pub mod patch;
pub mod search;
pub mod shell;

use crate::model::ToolCall;
use crate::sandbox::{CancelToken, CommandOutput, Sandbox};
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

/// A recorded file mutation for in-session summaries and later undo support.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub before: Option<String>,
    pub after: String,
}

pub struct Context {
    pub repo_root: PathBuf,
    sandbox: Sandbox,
    changes: Vec<FileChange>,
}

impl Context {
    pub fn new(repo_root: PathBuf, cancel: CancelToken) -> Self {
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        Context {
            sandbox: Sandbox::new(repo_root.clone(), cancel),
            repo_root,
            changes: Vec::new(),
        }
    }

    pub fn run_command(&self, command: &str) -> ToolResult {
        let output = self.sandbox.run(command)?;
        let summary = if output.cancelled {
            "cancelled"
        } else if output.timed_out {
            "timed out"
        } else {
            "command finished"
        };
        Ok(ToolOutput::new(output.render(), summary))
    }

    /// Run a pre-declared verification command through the sandbox, exposing the
    /// raw exit status. Unlike `run_command`, this does not wrap the output for a
    /// tool result; the verification gate reads `status` directly.
    pub fn run_verify(&self, command: &str) -> Result<CommandOutput, ToolError> {
        self.sandbox.run(command)
    }

    /// Resolve a repo-relative path that must already exist, rejecting escapes.
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

    /// Resolve a repo-relative path for creation/overwrite, rejecting parent escapes.
    pub fn resolve_for_create(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let joined = self.repo_root.join(rel);
        let parent = joined
            .parent()
            .ok_or_else(|| ToolError::new(format!("invalid path '{rel}'")))?;
        let parent_canon = parent
            .canonicalize()
            .map_err(|e| ToolError::new(format!("{rel}: parent {e}")))?;
        if !parent_canon.starts_with(&self.repo_root) {
            return Err(ToolError::new(format!(
                "path '{rel}' escapes the repository root"
            )));
        }
        let file_name = joined
            .file_name()
            .ok_or_else(|| ToolError::new(format!("invalid path '{rel}'")))?;
        Ok(parent_canon.join(file_name))
    }

    /// Record a file mutation in order of application.
    pub fn record_change(
        &mut self,
        path: impl Into<String>,
        before: Option<String>,
        after: String,
    ) {
        self.changes.push(FileChange {
            path: path.into(),
            before,
            after,
        });
    }

    /// Return the file mutations recorded in this session.
    pub fn changes(&self) -> &[FileChange] {
        &self.changes
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

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
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
        Box::new(fs::WriteFile),
        Box::new(search::SearchText),
        Box::new(patch::ApplyPatch),
        Box::new(git::GitDiff),
        Box::new(git::GitStatus),
        Box::new(shell::RunCommand),
    ]
}

/// Built-in tools filtered by an optional allow-list of tool names. `None`
/// enables all; `Some(list)` keeps only the named ones.
pub fn builtin_tools_filtered(enabled: Option<&[String]>) -> Vec<Box<dyn Tool>> {
    match enabled {
        None => builtin_tools(),
        Some(list) => builtin_tools()
            .into_iter()
            .filter(|t| list.iter().any(|n| n == t.name()))
            .collect(),
    }
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
    use crate::sandbox::cancel_token;
    use tempfile::tempdir;

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
        let mut cx = Context::new(PathBuf::from("."), cancel_token());
        let out = r.dispatch(&call("echo", json!({"text":"hi"})), &mut cx);
        assert_eq!(out.content, "hi");
    }

    #[test]
    fn dispatch_unknown_tool_is_error_output() {
        let r = Registry::new();
        let mut cx = Context::new(PathBuf::from("."), cancel_token());
        let out = r.dispatch(&call("nope", json!({})), &mut cx);
        assert!(out.content.contains("not available"));
    }

    #[test]
    fn resolve_rejects_escape() {
        let cx = Context::new(PathBuf::from("."), cancel_token());
        assert!(cx.resolve_existing("../../../etc/passwd").is_err());
    }

    #[test]
    fn resolve_for_create_allows_new_file_in_repo() {
        let dir = tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let p = cx.resolve_for_create("new.txt").unwrap();
        assert!(p.starts_with(&cx.repo_root));
        assert!(p.ends_with("new.txt"));
    }

    #[test]
    fn resolve_for_create_rejects_escape() {
        let dir = tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(cx.resolve_for_create("../evil.txt").is_err());
    }

    #[test]
    fn record_change_is_logged() {
        let dir = tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        cx.record_change("a.txt", None, "hi".to_string());
        assert_eq!(cx.changes().len(), 1);
        assert_eq!(cx.changes()[0].path, "a.txt");
        assert_eq!(cx.changes()[0].before, None);
        assert_eq!(cx.changes()[0].after, "hi");
    }

    #[test]
    fn run_verify_exposes_exit_status() {
        let dir = tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let ok = cx.run_verify("exit 0").unwrap();
        assert_eq!(ok.status, Some(0));
        let bad = cx.run_verify("exit 3").unwrap();
        assert_eq!(bad.status, Some(3));
    }

    #[test]
    fn cap_output_truncates() {
        let big = "x".repeat(100);
        let capped = cap_output(&big, 10);
        assert!(capped.len() < big.len());
        assert!(capped.contains("truncated"));
    }

    #[test]
    fn filtered_builtins_default_to_all() {
        let all = builtin_tools().len();
        let same = builtin_tools_filtered(None).len();
        assert_eq!(all, same);
    }

    #[test]
    fn filtered_builtins_respect_allow_list() {
        let enabled = vec!["read_file".to_string(), "search_text".to_string()];
        let tools = builtin_tools_filtered(Some(&enabled));
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"search_text"));
        assert!(!names.contains(&"run_command"));
    }
}
