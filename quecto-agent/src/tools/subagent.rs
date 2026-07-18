use crate::agent::{Agent, AgentConfig, Outcome, RunRecorder};
use crate::model::Message;
use crate::tools::{Context, Tool, ToolError, ToolOutput, ToolResult, FileChange};
use serde_json::{json, Value};
use crate::sandbox::CancelToken;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub const MAX_CONCURRENT_SUBAGENTS: usize = 8;
const PROGRESS_CAP: usize = 50;

fn push_progress(buf: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut v = buf.lock().unwrap();
    v.push(line);
    let len = v.len();
    if len > PROGRESS_CAP {
        v.drain(0..len - PROGRESS_CAP);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunStatus {
    Running,
    Complete(String),
    Cancelled,
    Failed(String),
}

struct SubagentInfo {
    id: u32,
    role: String,
    prompt: String,
    started: Instant,
    cancel: CancelToken,
    progress: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<RunStatus>>,
}

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: u32,
    pub role: String,
    pub prompt: String,
    pub status: RunStatus,
    pub elapsed: Duration,
    pub progress: Vec<String>,
}

impl From<&SubagentInfo> for SubagentSnapshot {
    fn from(info: &SubagentInfo) -> Self {
        SubagentSnapshot {
            id: info.id,
            role: info.role.clone(),
            prompt: info.prompt.clone(),
            status: info.status.lock().unwrap().clone(),
            elapsed: info.started.elapsed(),
            progress: info.progress.lock().unwrap().clone(),
        }
    }
}

struct ProgressRecorder {
    buf: Arc<Mutex<Vec<String>>>,
}

impl RunRecorder for ProgressRecorder {
    fn message(&mut self, m: &Message) {
        match m.role.as_str() {
            "assistant" => {
                for call in &m.tool_calls {
                    push_progress(&self.buf, format!("called {}({})", call.name, call.arguments));
                }
                if m.tool_calls.is_empty() && !m.content.is_empty() {
                    let snippet: String = m.content.chars().take(160).collect();
                    push_progress(&self.buf, format!("said: {snippet}"));
                }
            }
            "tool" => {
                let snippet: String = m.content.chars().take(160).collect();
                push_progress(&self.buf, format!("-> {snippet}"));
            }
            _ => {}
        }
    }

    fn change(&mut self, _c: &FileChange) {}
}

#[derive(Clone)]
pub struct SubagentPool {
    next_id: Arc<AtomicU32>,
    handles: Arc<Mutex<HashMap<u32, SubagentInfo>>>,
}

impl SubagentPool {
    pub fn new() -> Self {
        SubagentPool {
            next_id: Arc::new(AtomicU32::new(1)),
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn allocate(
        &self,
        role: String,
        prompt: String,
        cancel: CancelToken,
    ) -> (u32, Arc<Mutex<Vec<String>>>, Arc<Mutex<RunStatus>>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let progress = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(RunStatus::Running));
        let info = SubagentInfo {
            id,
            role,
            prompt,
            started: Instant::now(),
            cancel,
            progress: progress.clone(),
            status: status.clone(),
        };
        self.handles.lock().unwrap().insert(id, info);
        (id, progress, status)
    }

    pub fn running_count(&self) -> usize {
        self.handles
            .lock()
            .unwrap()
            .values()
            .filter(|i| matches!(*i.status.lock().unwrap(), RunStatus::Running))
            .count()
    }

    pub fn set_status(&self, id: u32, status: RunStatus) {
        if let Some(info) = self.handles.lock().unwrap().get(&id) {
            *info.status.lock().unwrap() = status;
        }
    }

    /// `Some(true)` if a running subagent was signalled to stop, `Some(false)`
    /// if it had already finished, `None` if `id` is unknown.
    pub fn cancel(&self, id: u32) -> Option<bool> {
        let handles = self.handles.lock().unwrap();
        let info = handles.get(&id)?;
        let running = matches!(*info.status.lock().unwrap(), RunStatus::Running);
        if running {
            info.cancel.store(true, Ordering::SeqCst);
        }
        Some(running)
    }

    pub fn get(&self, id: u32) -> Option<SubagentSnapshot> {
        self.handles.lock().unwrap().get(&id).map(SubagentSnapshot::from)
    }

    pub fn all(&self) -> Vec<SubagentSnapshot> {
        let handles = self.handles.lock().unwrap();
        let mut v: Vec<SubagentSnapshot> = handles.values().map(SubagentSnapshot::from).collect();
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    }
}

fn status_label(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Complete(_) => "complete",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Failed(_) => "failed",
    }
}

fn render_summary_line(snap: &SubagentSnapshot) -> String {
    format!(
        "#{} [{}] role={} elapsed={:.1}s",
        snap.id,
        status_label(&snap.status),
        snap.role,
        snap.elapsed.as_secs_f64()
    )
}

fn render_snapshot(snap: &SubagentSnapshot) -> String {
    let mut out = render_summary_line(snap);
    out.push_str("\nprompt: ");
    out.push_str(&snap.prompt);
    if !snap.progress.is_empty() {
        out.push_str("\nrecent activity:\n");
        out.push_str(&snap.progress.join("\n"));
    }
    match &snap.status {
        RunStatus::Complete(text) => {
            out.push_str("\nresult:\n");
            out.push_str(text);
        }
        RunStatus::Failed(msg) => {
            out.push_str("\nfailure reason: ");
            out.push_str(msg);
        }
        _ => {}
    }
    out
}

#[derive(Clone)]
pub struct MonitorSubagents {
    pub pool: SubagentPool,
}

impl MonitorSubagents {
    pub fn new(pool: SubagentPool) -> Self {
        MonitorSubagents { pool }
    }
}

impl Tool for MonitorSubagents {
    fn name(&self) -> &str {
        "monitor_subagents"
    }

    fn description(&self) -> &str {
        "Reports status, elapsed time, and recent activity for subagents started with \
spawn_subagent. Pass an id to check one; omit it to list all spawned this session."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The id returned by spawn_subagent. Omit to list all spawned subagents."
                }
            },
            "required": []
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let id = args.get("id").and_then(|v| v.as_u64()).map(|v| v as u32);
        match id {
            Some(id) => {
                let snap = self
                    .pool
                    .get(id)
                    .ok_or_else(|| ToolError::new(format!("no subagent with id {id}")))?;
                Ok(ToolOutput::new(render_snapshot(&snap), "subagent status"))
            }
            None => {
                let all = self.pool.all();
                if all.is_empty() {
                    return Ok(ToolOutput::new("no subagents have been spawned yet", "no subagents"));
                }
                let lines: Vec<String> = all.iter().map(render_summary_line).collect();
                Ok(ToolOutput::new(lines.join("\n"), "subagent list"))
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn token() -> CancelToken {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn allocate_assigns_increasing_ids_and_starts_running() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "task one".into(), token());
        let (id2, ..) = pool.allocate("Subagent".into(), "task two".into(), token());
        assert!(id2 > id1);
        let snap = pool.get(id1).unwrap();
        assert_eq!(snap.status, RunStatus::Running);
        assert_eq!(snap.prompt, "task one");
    }

    #[test]
    fn running_count_only_counts_running() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (_id2, ..) = pool.allocate("Subagent".into(), "b".into(), token());
        assert_eq!(pool.running_count(), 2);
        pool.set_status(id1, RunStatus::Complete("done".into()));
        assert_eq!(pool.running_count(), 1);
    }

    #[test]
    fn cancel_unknown_id_returns_none() {
        let pool = SubagentPool::new();
        assert_eq!(pool.cancel(999), None);
    }

    #[test]
    fn cancel_running_flips_token_and_returns_some_true() {
        let pool = SubagentPool::new();
        let t = token();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), t.clone());
        assert_eq!(pool.cancel(id), Some(true));
        assert!(t.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_finished_returns_some_false_without_flipping_token() {
        let pool = SubagentPool::new();
        let t = token();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), t.clone());
        pool.set_status(id, RunStatus::Complete("done".into()));
        assert_eq!(pool.cancel(id), Some(false));
        assert!(!t.load(Ordering::SeqCst));
    }

    #[test]
    fn all_lists_every_spawned_subagent_newest_first() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (id2, ..) = pool.allocate("Reviewer".into(), "b".into(), token());
        let all = pool.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, id2);
        assert_eq!(all[1].id, id1);
    }

    #[test]
    fn push_progress_caps_at_50_lines() {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        for i in 0..60 {
            push_progress(&buf, format!("line {i}"));
        }
        let locked = buf.lock().unwrap();
        assert_eq!(locked.len(), 50);
        assert_eq!(locked[0], "line 10");
        assert_eq!(locked[49], "line 59");
    }

    #[test]
    fn progress_recorder_logs_tool_calls_and_results() {
        use crate::agent::RunRecorder;
        use crate::model::ToolCall;

        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut rec = ProgressRecorder { buf: buf.clone() };

        let mut assistant = Message::assistant_with_calls(
            String::new(),
            vec![ToolCall {
                id: "1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }],
        );
        assistant.role = "assistant".into();
        rec.message(&assistant);

        let tool_result = Message::tool_result("1", "42 lines");
        rec.message(&tool_result);

        let locked = buf.lock().unwrap();
        assert_eq!(locked.len(), 2);
        assert!(locked[0].contains("read_file"));
        assert!(locked[1].contains("42 lines"));
    }

    #[test]
    fn progress_recorder_caps_at_50_entries() {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut rec = ProgressRecorder { buf: buf.clone() };
        for i in 0..60 {
            let m = Message::tool_result("1", &format!("result {i}"));
            rec.message(&m);
        }
        assert_eq!(buf.lock().unwrap().len(), 50);
    }

    #[test]
    fn monitor_reports_single_subagent_by_id() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Reviewer".into(), "look for bugs".into(), token());
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("running"));
        assert!(out.content.contains("Reviewer"));
        assert!(out.content.contains("look for bugs"));
    }

    #[test]
    fn monitor_reports_complete_result() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Subagent".into(), "count files".into(), token());
        pool.set_status(id, RunStatus::Complete("42 files".to_string()));
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("complete"));
        assert!(out.content.contains("42 files"));
    }

    #[test]
    fn monitor_unknown_id_is_an_error() {
        let pool = SubagentPool::new();
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        assert!(tool.run(&json!({"id": 999}), &mut cx).is_err());
    }

    #[test]
    fn monitor_without_id_lists_all_newest_first() {
        let pool = SubagentPool::new();
        let (id1, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        let (id2, ..) = pool.allocate("Reviewer".into(), "b".into(), token());
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({}), &mut cx).unwrap();
        let id1_pos = out.content.find(&format!("#{id1}")).unwrap();
        let id2_pos = out.content.find(&format!("#{id2}")).unwrap();
        assert!(id2_pos < id1_pos, "newest (#{id2}) should be listed first");
    }

    #[test]
    fn monitor_without_id_and_no_subagents_says_so() {
        let pool = SubagentPool::new();
        let tool = MonitorSubagents::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({}), &mut cx).unwrap();
        assert!(out.content.contains("no subagents"));
    }
}
