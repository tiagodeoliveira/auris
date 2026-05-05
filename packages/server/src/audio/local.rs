//! `LocalAudioSource` — captures from local hardware via macOS
//! ScreenCaptureKit. On non-macOS targets, returns
//! `AudioInitError::Unsupported`.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::source::AudioInitError;

/// Audio source that captures system audio + microphone in-process
/// via ScreenCaptureKit. macOS only; on other platforms `start` returns
/// `Unsupported`.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalAudioSource;

impl LocalAudioSource {
    pub async fn start(
        &self,
        cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<Vec<u8>>, AudioInitError> {
        #[cfg(target_os = "macos")]
        {
            super::capture::spawn_audio_task(cancel).await
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cancel;
            Err(AudioInitError::Unsupported)
        }
    }
}
