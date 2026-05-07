//! Summarizer tasks spawned per active meeting.
//!
//! - `transcript`: pass-through, no LLM (raw STT chunks → items).
//! - `agent`: single tool-calling LLM agent that emits highlights /
//!   actions / open_questions across the meeting's lifetime.
//! - `moment` / `artifact`: one-shot LLM summaries, run on user
//!   action rather than on a heartbeat.

pub mod agent;
pub mod artifact;
pub mod moment;
pub mod transcript;
