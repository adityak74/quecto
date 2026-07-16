use super::{Context, Tool, ToolError, ToolResult, ToolOutput};
use serde_json::{json, Value};

pub struct RunCommand;

impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a shell command at the repository root with timeout, cancellation, bounded output, and approval."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string","description":"Command passed to /bin/sh -c"}},"required":["command"],"additionalProperties":false})
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::new("run_command requires a non-empty string 'command'"))?;
        cx.run_command(command)
    }
}


pub struct StartBackgroundProcess;

impl Tool for StartBackgroundProcess {
    fn name(&self) -> &str {
        "start_background_process"
    }

    fn description(&self) -> &str {
        "Starts a detached background process and returns its PID."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string","description":"Command passed to /bin/sh -c"}},"required":["command"],"additionalProperties":false})
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::new("requires a non-empty string 'command'"))?;
        let pid = cx.start_background_process(command)?;
        Ok(ToolOutput::new(format!("Started background process with PID {}", pid), "started"))
    }
}

pub struct KillBackgroundProcess;

impl Tool for KillBackgroundProcess {
    fn name(&self) -> &str {
        "kill_background_process"
    }

    fn description(&self) -> &str {
        "Kills a background process by its PID."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"pid":{"type":"integer","description":"The PID of the background process to kill"}},"required":["pid"],"additionalProperties":false})
    }

    fn run(&self, args: &Value, cx: &mut Context) -> ToolResult {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::new("requires an integer 'pid'"))? as u32;
        cx.kill_background_process(pid)?;
        Ok(ToolOutput::new(format!("Killed background process {}", pid), "killed"))
    }
}

pub struct ListBackgroundProcesses;

impl Tool for ListBackgroundProcesses {
    fn name(&self) -> &str {
        "list_background_processes"
    }

    fn description(&self) -> &str {
        "Lists all running background processes."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","properties":{},"required":[]})
    }

    fn run(&self, _args: &Value, cx: &mut Context) -> ToolResult {
        let list = cx.list_background_processes();
        Ok(ToolOutput::new(list, "listed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use serde_json::json;

    #[test]
    fn schema_requires_only_command() {
        let schema = RunCommand.schema();
        assert_eq!(schema["required"], json!(["command"]));
        assert!(schema["properties"].get("cwd").is_none());
    }

    #[test]
    fn runs_and_reports_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        let out = RunCommand
            .run(&json!({"command":"printf hello"}), &mut cx)
            .unwrap();
        assert!(out.content.contains("exit_status: 0"));
        assert!(out.content.contains("hello"));
    }

    #[test]
    fn missing_command_is_a_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cx = Context::new(dir.path().to_path_buf(), cancel_token());
        assert!(RunCommand.run(&json!({}), &mut cx).is_err());
    }
}
