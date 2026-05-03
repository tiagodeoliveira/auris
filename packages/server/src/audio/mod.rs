//! Server-side audio capture.
//!
//! On macOS, captures system audio + mic via ScreenCaptureKit and emits
//! 16 kHz mono S16LE PCM frames (~20 ms each) on an mpsc channel.
//!
//! On other platforms, returns `Err(AudioInitError::Unsupported)` — the
//! server still runs, just without an audio source. Tests can use this
//! by setting `MEETING_COMPANION_AUDIO_DISABLED=1`.

pub mod format;

#[cfg(target_os = "macos")]
pub mod capture;

#[cfg(target_os = "macos")]
pub use capture::{spawn_audio_task, AudioInitError};

#[cfg(not(target_os = "macos"))]
pub async fn spawn_audio_task(
    _cancel: tokio_util::sync::CancellationToken,
) -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>, AudioInitError> {
    Err(AudioInitError::Unsupported)
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, thiserror::Error)]
pub enum AudioInitError {
    #[error("Audio capture is only supported on macOS in this build.")]
    Unsupported,
}
