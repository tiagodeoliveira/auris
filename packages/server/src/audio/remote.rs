//! `RemoteAudioSource` — accepts PCM frames over the `/audio`
//! WebSocket endpoint. The server itself does not capture; a separate
//! client (the Mac app, or `wscat` for testing) opens `/audio` and
//! streams binary frames of 16 kHz mono S16LE PCM, ~640 bytes each.
//!
//! Lifecycle:
//!   1. Server boot: a single `RemoteAudioSource` is created and
//!      stored on `ServerHandle`.
//!   2. A client connects to `/audio`: the WS handler creates an mpsc
//!      pair and *installs* the receiver into the source's slot.
//!   3. A meeting begins: `start()` *takes* the receiver out of the
//!      slot. The downstream STT consumes from it.
//!   4. WS client disconnects: the tx side drops; the receiver sees
//!      end-of-stream; meeting continues silent until reconnect.
//!
//! Edge cases:
//!   - Meeting begins with nothing in the slot: returns `NotConnected`.
//!   - Second client connects mid-meeting: the new receiver replaces
//!     the slot, but the active meeting still holds the old rx.
//!     Phase 2 introduces device registration that resolves "which
//!     device is bound to this meeting" cleanly.

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use super::source::AudioInitError;

#[derive(Clone, Default)]
pub struct RemoteAudioSource {
    /// Holds the receiver from the most-recently-connected `/audio`
    /// client. `start()` takes it out; subsequent connections replace
    /// it. Wrapped in `Arc<Mutex<...>>` so the WS handler and the
    /// meeting starter (different async tasks) can both reach it.
    inner: Arc<Mutex<Option<mpsc::Receiver<Vec<u8>>>>>,
}

impl RemoteAudioSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by the `/audio` WS handler when a client connects. Any
    /// previous (un-taken) receiver is dropped — first-mover loses if
    /// nobody claimed it.
    pub async fn install(&self, rx: mpsc::Receiver<Vec<u8>>) {
        let mut slot = self.inner.lock().await;
        *slot = Some(rx);
    }

    /// Called by `AudioSource::start` when a meeting begins. Takes the
    /// currently-installed receiver, or returns `NotConnected` if no
    /// client has connected yet.
    pub async fn start(
        &self,
        _cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<Vec<u8>>, AudioInitError> {
        let mut slot = self.inner.lock().await;
        slot.take().ok_or(AudioInitError::NotConnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_with_empty_slot_returns_not_connected() {
        let src = RemoteAudioSource::new();
        let cancel = CancellationToken::new();
        let result = src.start(cancel).await;
        assert!(matches!(result, Err(AudioInitError::NotConnected)));
    }

    #[tokio::test]
    async fn install_then_start_returns_receiver_and_empties_slot() {
        let src = RemoteAudioSource::new();
        let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
        src.install(rx).await;

        let cancel = CancellationToken::new();
        let mut taken = src.start(cancel.clone()).await.unwrap();

        // The receiver we got out should be the one we installed.
        tx.send(b"hello".to_vec()).await.unwrap();
        let frame = taken.recv().await.unwrap();
        assert_eq!(frame, b"hello");

        // Slot is now empty; second start fails.
        let result = src.start(cancel).await;
        assert!(matches!(result, Err(AudioInitError::NotConnected)));
    }

    #[tokio::test]
    async fn second_install_replaces_first() {
        let src = RemoteAudioSource::new();
        let (_tx_a, rx_a) = mpsc::channel::<Vec<u8>>(4);
        let (tx_b, rx_b) = mpsc::channel::<Vec<u8>>(4);
        src.install(rx_a).await;
        src.install(rx_b).await;

        let cancel = CancellationToken::new();
        let mut taken = src.start(cancel).await.unwrap();

        // The receiver we got out should be the second one (rx_b).
        tx_b.send(b"second".to_vec()).await.unwrap();
        let frame = taken.recv().await.unwrap();
        assert_eq!(frame, b"second");
        // rx_a is dropped — _tx_a sends would fail if we tried.
        drop(_tx_a);
    }
}
