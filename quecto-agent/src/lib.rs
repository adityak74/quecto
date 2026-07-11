//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod agent;
mod approval;
mod model;
mod policy;
mod tools;

pub use agent::{Agent, Outcome};
pub use approval::{ApprovalMode, Approver, TerminalApprover};
pub use model::{
    messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall,
};
pub use policy::{Decision, Policy};
pub use tools::fs::{ListFiles, ReadFile, WriteFile};
pub use tools::git::{GitDiff, GitStatus};
pub use tools::patch::ApplyPatch;
pub use tools::search::SearchText;
pub use tools::{
    builtin_tools, cap_output, Context, FileChange, Registry, Tool, ToolError, ToolOutput,
    ToolResult,
};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
