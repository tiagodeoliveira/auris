//! macOS audio capture via ScreenCaptureKit.
//! See `docs/specs/phase-2-step-15-live-pipeline.md` §6.

#![cfg(target_os = "macos")]

use crate::audio::format::convert_to_stt_pcm;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Debug, Error)]
pub enum AudioInitError {
    #[error("Screen Recording permission denied (TCC). Grant it in System Settings → Privacy & Security → Screen Recording, then restart the terminal.")]
    PermissionDenied,
    #[error("ScreenCaptureKit init failed: {0}")]
    Init(String),
}

/// Spawn the audio capture task. PCM frames (16 kHz mono S16LE, ~20 ms each)
/// land on the returned mpsc receiver. Cancel the token to stop capture.
///
/// Errors at init time only — frame-delivery errors are silent (the SCKit
/// handler closure can't propagate them; they'd be ignored by the kernel
/// anyway). If the capture stream silently dies mid-meeting, the receiver
/// will simply stop yielding values; the consumer should detect that via
/// timeout if it cares.
pub async fn spawn_audio_task(
    cancel: CancellationToken,
) -> Result<mpsc::Receiver<Vec<u8>>, AudioInitError> {
    use screencapturekit::prelude::*;
    use screencapturekit::stream::output_type::SCStreamOutputType;

    // 1. Permission check / display enumeration
    let content = SCShareableContent::get().map_err(|e| {
        let msg = format!("{e}");
        if msg.to_lowercase().contains("decline")
            || msg.to_lowercase().contains("permission")
            || msg.to_lowercase().contains("not authoriz")
        {
            AudioInitError::PermissionDenied
        } else {
            AudioInitError::Init(msg)
        }
    })?;
    let displays = content.displays();
    let display = displays
        .first()
        .ok_or_else(|| AudioInitError::Init("no displays available".into()))?;

    // 2. Filter + config (audio-only, width=2/height=2 workaround per spike)
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_captures_microphone(true)
        .with_sample_rate(48000)
        .with_channel_count(2);

    // 3. Channel for converted PCM frames
    let (tx, rx) = mpsc::channel::<Vec<u8>>(100);

    // 4. Stream + microphone handler that converts and forwards.
    //
    // We capture the MICROPHONE output, not the system Audio output. Reasons:
    //   - SCKit delivers Audio (system) and Microphone as two separate output
    //     types, each at ~50fps. Naively forwarding both into one mpsc would
    //     produce 100 fps of 20ms-each chunks, making the stream play at 2x
    //     real time and confusing Soniox's transcription.
    //   - Real-time mixing of the two streams (sample-buffer summer running
    //     at fixed 50fps) is the proper fix; deferred to a follow-up.
    //   - For most setups (laptop speakers, in-person meetings) the mic alone
    //     captures both the user's voice AND remote audio bleeding back via
    //     room acoustics — usable for STT.
    //   - Headphone users get only their own voice transcribed; documented
    //     limitation.
    //
    // A no-op Audio handler is still registered below — required by the
    // SCKit stream config when `with_captures_audio(true)`.
    let mut stream = SCStream::new(&filter, &config);
    let tx_audio = tx.clone();
    let frame_count = Arc::new(AtomicU64::new(0));
    let drop_count = Arc::new(AtomicU64::new(0));
    let frame_count_h = Arc::clone(&frame_count);
    let drop_count_h = Arc::clone(&drop_count);
    stream.add_output_handler(
        move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
            if output_type != SCStreamOutputType::Microphone {
                return;
            }
            // Pull format info
            let (sample_rate, channels) = match sample.format_description() {
                Some(fd) => (
                    fd.audio_sample_rate().unwrap_or(48000.0) as u32,
                    fd.audio_channel_count().unwrap_or(2) as u16,
                ),
                None => (48000, 2),
            };
            // Pull raw Float32 bytes
            let abl = match sample.audio_buffer_list() {
                Some(a) => a,
                None => return,
            };
            // Concat all AudioBuffers' data into a single Float32 slice
            let mut floats = Vec::<f32>::new();
            for ab in abl.iter() {
                let count = ab.data_byte_size() / std::mem::size_of::<f32>();
                if count == 0 {
                    continue;
                }
                let bytes = ab.data();
                // SAFETY: SCKit guarantees Float32 LPCM in the buffer per
                // the format description we asked for.
                let slice =
                    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) };
                floats.extend_from_slice(slice);
            }
            if floats.is_empty() {
                return;
            }
            // Convert to STT-ready PCM
            let pcm = convert_to_stt_pcm(&floats, sample_rate, channels);
            let pcm_len = pcm.len();
            // Best-effort send; if the receiver is full or gone, we drop the frame
            match tx_audio.try_send(pcm) {
                Ok(()) => {
                    let n = frame_count_h.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 50 == 0 {
                        info!(
                            frames = n,
                            last_pcm_bytes = pcm_len,
                            sample_rate,
                            channels,
                            "audio: forwarded PCM frames to mpsc"
                        );
                    }
                }
                Err(_) => {
                    let d = drop_count_h.fetch_add(1, Ordering::Relaxed) + 1;
                    if d == 1 || d % 50 == 0 {
                        info!(dropped = d, "audio: mpsc full or closed; dropping frame");
                    }
                }
            }
        },
        SCStreamOutputType::Microphone,
    );
    // Discard system audio (required by stream config; future mixer task can
    // consume both streams properly).
    stream.add_output_handler(
        |_sample: CMSampleBuffer, _output_type: SCStreamOutputType| {},
        SCStreamOutputType::Audio,
    );
    // Discard video (required handler even for audio-only streams).
    stream.add_output_handler(
        |_sample: CMSampleBuffer, _output_type: SCStreamOutputType| {},
        SCStreamOutputType::Screen,
    );

    // 5. Start
    stream
        .start_capture()
        .map_err(|e| AudioInitError::Init(format!("{e}")))?;

    // 6. Spawn supervisor task: when cancel fires, stop the stream.
    // SCStream is !Send, so we use std::thread to avoid a Send bound.
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        handle.block_on(cancel.cancelled());
        let _ = stream.stop_capture();
    });

    Ok(rx)
}
