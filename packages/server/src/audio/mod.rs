//! Server-side audio input.
//!
//! Audio enters the server through an `AudioSource`, picked at boot
//! via `MEETING_COMPANION_AUDIO_SOURCE`. Two variants are planned:
//!   - `local`  — in-process macOS ScreenCaptureKit capture (default).
//!   - `remote` — accepts PCM frames from a separate client over the
//!     `/audio` WebSocket endpoint (Phase 1b — not yet wired).
//!
//! Both variants produce 16 kHz mono S16LE PCM frames (~20 ms each)
//! on an mpsc channel — the downstream STT pipeline doesn't know
//! which one is feeding it.

pub mod format;
pub mod local;
pub mod remote;
pub mod source;

#[cfg(target_os = "macos")]
pub(crate) mod capture;

pub use local::LocalAudioSource;
pub use remote::RemoteAudioSource;
pub use source::{AudioInitError, AudioSource};
