//! agent-chat-share (`acs`): read a coding agent's chat session, convert it to a
//! canonical model, render a portable transcript, and emit a seed command that
//! continues the conversation in another agent.

pub mod adapters;
pub mod cli;
pub mod model;
pub mod render;
pub mod share;
pub mod skills;
pub mod tui;
