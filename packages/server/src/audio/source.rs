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

/// Errors returned by any `AudioSource::start`. Consolidated across
/// platforms — non-macOS builds will only ever produce `Unsupported`,
/// macOS builds may produce any variant.
#[derive(Debug, Error)]
pub enum AudioInitError {
    #[error("Audio capture is not supported on this platform.")]
    Unsupported,
    #[error("Screen Recording permission denied (TCC). Grant it in System Settings → Privacy & Security → Screen Recording, then restart the terminal.")]
    PermissionDenied,
    #[error("ScreenCaptureKit init failed: {0}")]
    Init(String),
}

/// Audio source kinds. Pick at boot.
pub enum AudioSource {
    Local(LocalAudioSource),
    // Remote(RemoteAudioSource) — added in Phase 1b
}

impl AudioSource {
    /// Construct from `MEETING_COMPANION_AUDIO_SOURCE`. Default `local`.
    pub fn from_env() -> Self {
        let kind =
            std::env::var("MEETING_COMPANION_AUDIO_SOURCE").unwrap_or_else(|_| "local".to_string());
        match kind.as_str() {
            "local" => Self::Local(LocalAudioSource),
            other => {
                tracing::warn!(
                    requested = %other,
                    "unknown audio source; falling back to local"
                );
                Self::Local(LocalAudioSource)
            }
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
        }
    }
}
