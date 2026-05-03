//! Per-mode summarizer tasks. See `docs/specs/phase-2-step-15-live-pipeline.md` §8.
//!
//! Three summarizers run in parallel during an active meeting:
//! - transcript: pass-through, no LLM (this task — task 4)
//! - highlights: rig Extractor on a 20s heartbeat (task 5)
//! - actions: rig Extractor on a 15s heartbeat (task 6)

pub mod actions;
pub mod highlights;
pub mod transcript;
