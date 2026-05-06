//! Streaming speech-to-text adapters.
//!
//! Defines the `SttAdapter` trait and the shared `TranscriptChunk`
//! type. Production adapter is `soniox`; offline / CI uses `mock`.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

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
    /// Local `users.id` of the meeting that produced this chunk.
    /// Stamped by the spawn site of the STT provider so downstream
    /// summarizers/mnemo know which user's UserState to mutate.
    pub user_id: String,
}

/// Errors during STT provider initialization (env-var lookups, etc.).
/// Note: per-call errors during provider operation are reported via the
/// transcript channel or via internal logging — they don't propagate
/// through this enum.
#[derive(Debug, Error)]
pub enum SttInitError {
    #[error("Unknown STT provider: '{0}'. Accepted values: mock, soniox")]
    Unknown(String),

    #[error("Missing credentials for provider '{0}'. Check the required env var.")]
    MissingCredentials(String),
}

/// An STT provider runs for the duration of an active meeting. It receives
/// PCM audio frames (16 kHz mono S16LE, ~20 ms each) on `audio_rx`, performs
/// transcription, and emits finalized `TranscriptChunk`s via `transcript_tx`.
///
/// Implementations are expected to:
/// - Honor `cancel.cancelled()` for cooperative shutdown.
/// - Tolerate `audio_rx == None` (mock providers ignore audio entirely).
/// - Tolerate `audio_rx` ending early (graceful drain).
/// - Never panic on transient errors (e.g. WS disconnects); reconnect or
///   degrade silently with `tracing::warn!` logging.
pub trait SttProvider: Send {
    /// Run the provider until cancelled. Consumes `self` for the meeting's lifetime.
    ///
    /// `events_tx` lets providers emit any server [`crate::contract::Event`] (e.g.
    /// `Event::Status` for reconnect telemetry, `Event::TranscriptInterim` for
    /// in-flight transcript display) without needing extra side-channels.
    /// `user_id` is the local `users.id` of the meeting owner. The
    /// provider stamps it on every `TranscriptChunk` and on any
    /// status/error events it emits, so downstream summarizers and
    /// WS subscribers can route to the right user.
    fn run(
        self: Box<Self>,
        audio_rx: Option<mpsc::Receiver<Vec<u8>>>,
        transcript_tx: broadcast::Sender<TranscriptChunk>,
        events_tx: broadcast::Sender<crate::contract::UserEvent>,
        user_id: String,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    /// Stable display name for logs.
    fn name(&self) -> &'static str;
}

/// Construct an STT provider from a name string.
/// Reads provider-specific configuration from env vars at construction time.
pub fn make_provider(name: &str) -> Result<Box<dyn SttProvider>, SttInitError> {
    match name {
        "mock" => Ok(Box::new(mock::MockStt::from_env())),
        "soniox" => Ok(Box::new(soniox::SonioxStt::from_env()?)),
        other => Err(SttInitError::Unknown(other.to_string())),
    }
}

pub mod mock;
pub mod soniox;

pub use mock::MockStt;
pub use soniox::SonioxStt;
