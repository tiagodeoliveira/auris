//! Server-side audio input.
//!
//! Audio is supplied by an external client (the Mac app) over the
//! `/audio` WebSocket endpoint. The server itself does not capture —
//! that's the client's job. Frames arrive as 16 kHz mono S16LE PCM
//! (~20 ms each), get forwarded into the active meeting's mpsc
//! channel, and feed the STT pipeline.
//!
//! Earlier phases also shipped an in-process macOS ScreenCaptureKit
//! capture (`LocalAudioSource`) for single-machine demos before the
//! Mac app existed. Removed once the Mac client became the only
//! audio source — kept the server cross-platform and dropped the
//! `screencapturekit` dependency.

pub mod format;
pub mod remote;

pub use remote::RemoteAudioSource;
