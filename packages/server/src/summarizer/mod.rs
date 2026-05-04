//! Per-mode summarizer tasks.
//!
//! Four summarizers run in parallel during an active meeting:
//! - transcript: pass-through, no LLM (this task — task 4)
//! - highlights: rig Extractor on a 20s heartbeat (task 5)
//! - actions: rig Extractor on a 15s heartbeat (task 6)
//! - open_questions: rig Extractor on a 15s heartbeat (task 7)

pub mod actions;
pub mod highlights;
pub mod open_questions;
pub mod transcript;
