//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod model;
mod agent;
mod tools;

pub use agent::{Agent, Outcome};
pub use model::{messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall};
pub use tools::fs::{ListFiles, ReadFile};
pub use tools::git::{GitDiff, GitStatus};
pub use tools::search::SearchText;
pub use tools::{builtin_tools, cap_output, Context, Registry, Tool, ToolError, ToolOutput, ToolResult};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
