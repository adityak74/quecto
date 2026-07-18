use crate::agent::{Agent, AgentConfig, Outcome, RunRecorder};
use crate::model::Message;
use crate::tools::{Context, Tool, ToolError, ToolOutput, ToolResult, FileChange};
use serde_json::{json, Value};
use crate::sandbox::CancelToken;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
pub struct CancelSubagent {
    pub pool: SubagentPool,
}

impl CancelSubagent {
    pub fn new(pool: SubagentPool) -> Self {
        CancelSubagent { pool }
    }
}

impl Tool for CancelSubagent {
    fn name(&self) -> &str {
        "cancel_subagent"
    }

    fn description(&self) -> &str {
        "Stops a subagent started with spawn_subagent before it finishes."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The id returned by spawn_subagent."
                }
            },
            "required": ["id"]
        })
    }

    fn run(&self, args: &Value, _cx: &mut Context) -> ToolResult {
        let id = args
            .get("id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .ok_or_else(|| ToolError::new("missing \"id\" parameter"))?;
        match self.pool.cancel(id) {
            Some(true) => Ok(ToolOutput::new(
                format!("cancel requested for subagent #{id}"),
                "cancel requested",
            )),
            Some(false) => Ok(ToolOutput::new(
                format!("subagent #{id} already finished"),
                "already finished",
            )),
            None => Err(ToolError::new(format!("no subagent with id {id}"))),
        }
    }
}

#[derive(Clone)]
pub struct InvokeSubagent {
    pub config: AgentConfig,
    pub pool: SubagentPool,
}

impl InvokeSubagent {
    pub fn new(config: AgentConfig, pool: SubagentPool) -> Self {
        InvokeSubagent { config, pool }
    }
}

impl Tool for InvokeSubagent {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Delegates a task to a subagent running concurrently in the background. \
Returns an ID immediately; use monitor_subagents to check progress and cancel_subagent to stop it."
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
        if self.pool.running_count() >= MAX_CONCURRENT_SUBAGENTS {
            return Err(ToolError::new(format!(
                "Cannot spawn subagent: at capacity ({} running). Wait for one to finish or cancel one.",
                MAX_CONCURRENT_SUBAGENTS
            )));
        }

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

        let child_cancel = Arc::new(AtomicBool::new(false));
        let (id, progress_buf, _status_token) =
            self.pool.allocate(role.into(), prompt.into(), child_cancel.clone());

        let config = self.config.clone();
        let pool = self.pool.clone();
        let prompt_clone = prompt.to_string();
        let child_tool = self.clone();

        std::thread::spawn(move || {
            let mut subagent = Agent::new(
                config.model,
                system_prompt,
                config.max_steps,
                config.repo_root,
                child_cancel,
                config.approval,
            )
            .register_builtins()
            .register(Box::new(child_tool))
            .with_recorder(Box::new(ProgressRecorder { buf: progress_buf }));

            let outcome = subagent.run(&prompt_clone);
            let result_status = match outcome {
                Outcome::Complete(text) => RunStatus::Complete(text),
                Outcome::Cancelled => RunStatus::Cancelled,
                Outcome::StepLimit => {
                    let mut text = "Subagent stopped: step limit reached before finishing.".to_string();
                    if let Some(p) = progress_summary(&subagent.messages, 6) {
                        text.push_str("\nLast steps attempted:\n");
                        text.push_str(&p);
                    }
                    RunStatus::Failed(text)
                }
                Outcome::RepeatedAction => {
                    let mut text = "Subagent stopped: got stuck repeating the same tool call without making progress.".to_string();
                    if let Some(p) = progress_summary(&subagent.messages, 6) {
                        text.push_str("\nLast steps attempted:\n");
                        text.push_str(&p);
                    }
                    RunStatus::Failed(text)
                }
                Outcome::VerificationFailed { attempts } => {
                    RunStatus::Failed(format!("Subagent stopped: Verification failed after {} attempts", attempts))
                }
                Outcome::Error(e) => RunStatus::Failed(format!("Subagent error: {}", e)),
                Outcome::Blocked => RunStatus::Failed("blocked by policy or approval".into()),
            };

            pool.set_status(id, result_status);
        });

        Ok(ToolOutput::new(
            format!("spawned subagent #{id}"),
            format!("spawned #{id}")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::thread;

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

    #[derive(Clone)]
    struct ImmediateReply {
        text: &'static str,
    }
    impl crate::model::Model for ImmediateReply {
        fn clone_box(&self) -> Box<dyn crate::model::Model> {
            Box::new(self.clone())
        }
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<crate::model::AssistantMessage, crate::BoxErr> {
            Ok(crate::model::AssistantMessage {
                content: self.text.to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                reasoning_content: None,
            })
        }
        fn complete_with_options(
            &self,
            messages: &[Message],
            tools: &[Value],
            _options: &crate::reasoning::CompletionOptions,
        ) -> Result<crate::model::ModelCompletion, crate::BoxErr> {
            self.complete(messages, tools).map(crate::model::ModelCompletion::from)
        }
    }

    #[derive(Clone)]
    struct AlwaysWantsTool {
        replies_left: Arc<AtomicU32>,
    }
    impl crate::model::Model for AlwaysWantsTool {
        fn clone_box(&self) -> Box<dyn crate::model::Model> {
            Box::new(self.clone())
        }
        fn complete(
            &self,
            _messages: &[Message],
            _tools: &[Value],
        ) -> Result<crate::model::AssistantMessage, crate::BoxErr> {
            let n = self.replies_left.fetch_sub(1, Ordering::SeqCst);
            if n == 0 {
                return Ok(crate::model::AssistantMessage {
                    content: "gave up".to_string(),
                    tool_calls: vec![],
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                });
            }
            Ok(crate::model::AssistantMessage {
                content: String::new(),
                tool_calls: vec![crate::model::ToolCall {
                    id: n.to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({}),
                }],
                finish_reason: "tool_calls".to_string(),
                reasoning_content: None,
            })
        }
        fn complete_with_options(
            &self,
            messages: &[Message],
            tools: &[Value],
            _options: &crate::reasoning::CompletionOptions,
        ) -> Result<crate::model::ModelCompletion, crate::BoxErr> {
            self.complete(messages, tools).map(crate::model::ModelCompletion::from)
        }
    }

    struct SlowCounter {
        count: Arc<AtomicU32>,
    }
    impl Tool for SlowCounter {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            "test-only tool that sleeps briefly and returns a changing value"
        }
        fn schema(&self) -> Value {
            json!({"type": "object", "properties": {}, "required": []})
        }
        fn run(&self, _args: &Value, _cx: &mut Context) -> ToolResult {
            std::thread::sleep(Duration::from_millis(30));
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::new(format!("tick {n}"), "tick"))
        }
    }

    fn test_config(model: impl crate::model::Model + 'static) -> AgentConfig {
        AgentConfig {
            model: Box::new(model),
            base_system_prompt: "you are a test agent".to_string(),
            max_steps: 30,
            repo_root: std::env::current_dir().unwrap(),
            cancel: token(),
            approval: crate::approval::ApprovalMode::AutoApprove,
        }
    }

    fn wait_until_finished(pool: &SubagentPool, id: u32) -> SubagentSnapshot {
        for _ in 0..200 {
            let snap = pool.get(id).unwrap();
            if snap.status != RunStatus::Running {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("subagent #{id} did not finish within 2s");
    }

    #[test]
    fn cancel_subagent_stops_a_running_subagent() {
        let model = AlwaysWantsTool {
            replies_left: Arc::new(AtomicU32::new(30)),
        };
        let config = test_config(model);
        let pool = SubagentPool::new();

        let child_cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let (id, progress, _status) =
            pool.allocate("Subagent".into(), "count forever".into(), child_cancel.clone());
        let pool_for_thread = pool.clone();
        thread::spawn(move || {
            let mut subagent = Agent::new(
                config.model.clone(),
                config.base_system_prompt.clone(),
                config.max_steps,
                config.repo_root.clone(),
                child_cancel,
                config.approval.clone(),
            )
            .register(Box::new(SlowCounter {
                count: Arc::new(AtomicU32::new(0)),
            }))
            .with_recorder(Box::new(ProgressRecorder { buf: progress }));
            let outcome = subagent.run("count forever");
            let status = match outcome {
                Outcome::Cancelled => RunStatus::Cancelled,
                Outcome::Complete(t) => RunStatus::Complete(t),
                _ => RunStatus::Failed("unexpected outcome".to_string()),
            };
            pool_for_thread.set_status(id, status);
        });

        // Give the thread a moment to start looping before cancelling it.
        std::thread::sleep(Duration::from_millis(300));
        let tool = CancelSubagent::new(pool.clone());
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("cancel requested"));

        let snap = wait_until_finished(&pool, id);
        assert_eq!(snap.status, RunStatus::Cancelled);
    }

    #[test]
    fn cancel_subagent_unknown_id_is_an_error() {
        let pool = SubagentPool::new();
        let tool = CancelSubagent::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        assert!(tool.run(&json!({"id": 999}), &mut cx).is_err());
    }

    #[test]
    fn cancel_subagent_already_finished_says_so() {
        let pool = SubagentPool::new();
        let (id, ..) = pool.allocate("Subagent".into(), "a".into(), token());
        pool.set_status(id, RunStatus::Complete("done".into()));
        let tool = CancelSubagent::new(pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let out = tool.run(&json!({"id": id}), &mut cx).unwrap();
        assert!(out.content.contains("already finished"));
    }

    #[test]
    fn spawn_subagent_rejects_new_spawns_if_at_capacity() {
        let config = test_config(ImmediateReply { text: "done" });
        let pool = SubagentPool::new();
        // fill capacity
        for _ in 0..MAX_CONCURRENT_SUBAGENTS {
            pool.allocate("role".into(), "prompt".into(), token());
        }
        let tool = InvokeSubagent::new(config, pool);
        let mut cx = Context::new(std::env::current_dir().unwrap(), token());
        let res = tool.run(
            &json!({"role": "Agent", "prompt": "do work"}),
            &mut cx,
        );
        match res {
            Err(e) => assert!(e.message.contains("capacity")),
            Ok(_) => panic!("expected error"),
        }
    }
}
