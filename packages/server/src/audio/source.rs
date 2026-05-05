//! `AudioSource` — pluggable audio input. Selected at server boot via
//! `MEETING_COMPANION_AUDIO_SOURCE` (default `local`).
//!
//! Variants:
//! - `Local` — wraps the macOS ScreenCaptureKit pipeline. The
//!   server captures system audio + microphone in-process and feeds
//!   the STT.
//! - `Remote` (Phase 1b) — accepts PCM frames over the `/audio`
//!   WebSocket endpoint from a separate client (the Mac app). The
//!   server itself does not capture; it relays.
//!
//! Frames produced by either variant: 16 kHz mono S16LE, ~640 bytes
//! (~20 ms) each, on an mpsc channel. Same shape downstream pipeline
//! consumes regardless of source.

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::local::LocalAudioSource;
use super::remote::RemoteAudioSource;

/// Errors returned by any `AudioSource::start`. Consolidated across
/// platforms — non-macOS builds will only ever produce `Unsupported`,
/// macOS builds may produce any variant. `NotConnected` is specific
/// to `Remote`: meeting started but no `/audio` client is paired.
#[derive(Debug, Error)]
pub enum AudioInitError {
    #[error("Audio capture is not supported on this platform.")]
    Unsupported,
    #[error("Screen Recording permission denied (TCC). Grant it in System Settings → Privacy & Security → Screen Recording, then restart the terminal.")]
    PermissionDenied,
    #[error("ScreenCaptureKit init failed: {0}")]
    Init(String),
    #[error(
        "No audio client is connected to /audio. Start the Mac app or wscat-stream PCM frames."
    )]
    NotConnected,
}

/// Audio source kinds. One instance is created at server boot
/// (lives as long as the process); meetings call `start()` against
/// it to begin a per-meeting capture.
pub enum AudioSource {
    Local(LocalAudioSource),
    Remote(RemoteAudioSource),
}

impl AudioSource {
    /// Construct from `MEETING_COMPANION_AUDIO_SOURCE`. Default `local`.
    pub fn from_env() -> Self {
        let kind =
            std::env::var("MEETING_COMPANION_AUDIO_SOURCE").unwrap_or_else(|_| "local".to_string());
        match kind.as_str() {
            "local" => Self::Local(LocalAudioSource),
            "remote" => Self::Remote(RemoteAudioSource::new()),
            other => {
                tracing::warn!(
                    requested = %other,
                    "unknown audio source; falling back to local"
                );
                Self::Local(LocalAudioSource)
            }
        }
    }

    /// Returns `Some(&RemoteAudioSource)` when this source is the
    /// `Remote` variant. The `/audio` WebSocket handler uses this to
    /// install incoming PCM frames; for any other variant the handler
    /// rejects the connection.
    pub fn as_remote(&self) -> Option<&RemoteAudioSource> {
        match self {
            Self::Remote(r) => Some(r),
            _ => None,
        }
    }

    /// Begin producing PCM frames into the returned receiver. Cancel
    /// the token to stop. Frames: 16 kHz mono S16LE, ~640 bytes each.
    pub async fn start(
        &self,
        cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<Vec<u8>>, AudioInitError> {
        match self {
            Self::Local(s) => s.start(cancel).await,
            Self::Remote(s) => s.start(cancel).await,
        }
    }
}
