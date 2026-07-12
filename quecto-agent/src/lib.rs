//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod agent;
mod approval;
mod chat;
mod context;
mod flavor;
mod instructions;
mod model;
mod policy;
mod recorder;
mod render;
mod sandbox;
mod session;
mod tools;
mod verify;

pub use agent::{Agent, Outcome, RunRecorder};
pub use approval::{ApprovalMode, Approver, TerminalApprover};
pub use chat::{parse_command, ChatCommand};
pub use context::seed as seed_context;
pub use flavor::{
    layer_paths, resolve, resolve_scoped, ApprovalSection, Flavor, Scope, ToolsSection,
    VerifySection,
};
pub use instructions::load as load_instructions;
pub use model::{
    messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall,
};
pub use policy::{Decision, Policy, Preset};
pub use recorder::SqliteRecorder;
pub use render::{stderr_renderer, stdout_renderer, LineRenderer, Renderer};
pub use sandbox::{cancel_token, CancelToken, CommandOutput, Sandbox};
pub use session::{new_session_id, render_change_summary, SessionRow, Store};
pub use tools::fs::{ListFiles, ReadFile, WriteFile};
pub use tools::git::{GitDiff, GitStatus};
pub use tools::patch::ApplyPatch;
pub use tools::search::SearchText;
pub use tools::shell::RunCommand;
pub use tools::{
    builtin_tools, cap_output, Context, FileChange, Registry, Tool, ToolError, ToolOutput,
    ToolResult,
};
pub use verify::{Verifier, VerifyReport, VerifyResult};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
