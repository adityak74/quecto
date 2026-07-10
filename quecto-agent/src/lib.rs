//! quecto-agent — a coding agent built on the tiny quecto core.
//! Milestone 1 (walking skeleton): normalized model turns + a bare agent loop.

mod model;
mod agent;

pub use agent::{Agent, Outcome};
pub use model::{messages_to_body, parse_assistant, AssistantMessage, HttpModel, Message, Model, ToolCall};

/// Shared boxed error, mirroring the core so `?` composes across both crates.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;
