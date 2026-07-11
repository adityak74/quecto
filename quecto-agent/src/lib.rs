//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod agent;
mod approval;
mod context;
mod instructions;
mod model;
mod policy;
mod sandbox;
mod tools;
mod verify;

pub use agent::{Agent, Outcome};
pub use approval::{ApprovalMode, Approver, TerminalApprover};
pub use model::{
    messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall,
};
pub use policy::{Decision, Policy};
pub use sandbox::{cancel_token, CancelToken, CommandOutput, Sandbox};
pub use tools::fs::{ListFiles, ReadFile, WriteFile};
pub use tools::git::{GitDiff, GitStatus};
pub use tools::patch::ApplyPatch;
pub use tools::search::SearchText;
pub use tools::shell::RunCommand;
pub use tools::{
    builtin_tools, cap_output, Context, FileChange, Registry, Tool, ToolError, ToolOutput,
    ToolResult,
};
pub use context::seed as seed_context;
pub use instructions::load as load_instructions;
pub use verify::{VerifyReport, VerifyResult, Verifier};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
