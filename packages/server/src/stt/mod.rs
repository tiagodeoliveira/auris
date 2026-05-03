//! STT module — see `docs/specs/phase-2-step-15-live-pipeline.md` §7.
//!
//! This module ships incrementally. Task 2 only defines the shared
//! TranscriptChunk type; the actual STT clients land in tasks 3 (mock)
//! and 10 (Soniox).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptChunk {
    /// Stable chunk id (uuid v4).
    pub id: String,
    /// Finalized utterance text. Trimmed; non-empty.
    pub text: String,
    /// ms offset from meeting start at the start of this utterance.
    pub t_start_ms: u64,
    /// ms offset from meeting start at the end of this utterance.
    pub t_end_ms: u64,
    /// Optional speaker label from STT token metadata (often unavailable).
    pub speaker: Option<String>,
}

pub mod mock;

pub use mock::run_mock_stt;
