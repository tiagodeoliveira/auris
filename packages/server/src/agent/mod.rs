//! Live chat agent — interactive, tool-calling, drives the chat
//! mode UI. Distinct from `workers/` (background fire-and-forget
//! tasks). One agent spawns per active meeting.

pub mod active;
pub mod blocks;
pub mod bootstrap;
pub mod chat;
pub mod prompts;
pub mod tools;

pub use active::spawn_active_extractor;
pub use chat::{spawn_meeting_agent, AgentKick, AgentKickReason, AttachmentPayload};
