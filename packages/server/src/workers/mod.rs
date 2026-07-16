//! Background event-driven workers. Each spawns once per active
//! meeting (or on demand for `metadata`) and exits when the meeting
//! cancellation token fires. Distinct from `agent/` (interactive,
//! tool-calling).

pub mod artifact;
pub mod backfill;
pub mod chat_context;
pub mod finalize;
pub mod metadata;
pub mod moment;
pub mod summarize;
pub mod sweep;
pub mod transcript;
pub mod wrap_up;
