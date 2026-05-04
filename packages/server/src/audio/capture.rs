//! macOS audio capture via ScreenCaptureKit.
//!
//! Mixes two SCKit output streams (system audio + microphone) into a single
//! 16 kHz mono S16LE PCM stream feeding Soniox. Both sources fire at ~50 fps;
//! per-source ring buffers absorb the inevitable timing jitter and a tokio
//! mixer task wakes at 20 ms intervals to sum and forward.

#![cfg(target_os = "macos")]

use crate::audio::format::{floats_to_s16le_bytes, to_mono_16k_f32};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum AudioInitError {
    #[error("Screen Recording permission denied (TCC). Grant it in System Settings → Privacy & Security → Screen Recording, then restart the terminal.")]
    PermissionDenied,
    #[error("ScreenCaptureKit init failed: {0}")]
    Init(String),
}

/// 1 second of 16 kHz mono Float32 = 16,000 samples per ring. Drop-oldest
/// when full. With both sources at ~50 fps and the mixer also at 50 fps,
/// rings stay near-empty in steady state; the cap protects against drift
/// (e.g., one source briefly stalls).
const RING_CAP_SAMPLES: usize = 16_000;

/// 20 ms of 16 kHz mono Float32 = 320 samples per mixer tick. Matches the
/// frame size SCKit delivers per source, so steady-state has each ring
/// holding 0-1 frames.
const MIXER_FRAME_SAMPLES: usize = 320;

const MIXER_INTERVAL: Duration = Duration::from_millis(20);

/// Spawn the audio capture pipeline. Mixed PCM frames (16 kHz mono S16LE,
/// ~20 ms each, ~640 bytes each) land on the returned mpsc receiver. Cancel
/// the token to stop capture.
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

    // 2. Filter + config (audio + mic, both enabled; width=2/height=2 audio-only workaround)
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

    // 3. Output mpsc + per-source ring buffers
    let (tx, rx) = mpsc::channel::<Vec<u8>>(100);
    let system_ring: Arc<Mutex<VecDeque<f32>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAP_SAMPLES)));
    let mic_ring: Arc<Mutex<VecDeque<f32>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAP_SAMPLES)));

    // 4. Stream + handlers
    let mut stream = SCStream::new(&filter, &config);

    // System audio handler — pushes Float32 mono 16k samples to system_ring.
    {
        let ring = Arc::clone(&system_ring);
        stream.add_output_handler(
            move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Audio {
                    return;
                }
                push_into_ring(&sample, &ring);
            },
            SCStreamOutputType::Audio,
        );
    }

    // Microphone handler — same shape, into mic_ring.
    {
        let ring = Arc::clone(&mic_ring);
        stream.add_output_handler(
            move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Microphone {
                    return;
                }
                push_into_ring(&sample, &ring);
            },
            SCStreamOutputType::Microphone,
        );
    }

    // Discard video (required handler even for audio-only streams).
    stream.add_output_handler(
        |_sample: CMSampleBuffer, _output_type: SCStreamOutputType| {},
        SCStreamOutputType::Screen,
    );

    // 5. Start capture.
    stream
        .start_capture()
        .map_err(|e| AudioInitError::Init(format!("{e}")))?;

    // 6. Mixer task — drains both rings at MIXER_INTERVAL and forwards mixed PCM.
    // Per-frame counters are intentionally absent: the operator-meaningful
    // signal is the actual transcribed text (logged in the Soniox flush path),
    // not "PCM frames flowed". Sustained backpressure still surfaces via the
    // drop warning below.
    let mixer_cancel = cancel.clone();
    let mixer_tx = tx.clone();
    let mixer_system = Arc::clone(&system_ring);
    let mixer_mic = Arc::clone(&mic_ring);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(MIXER_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = mixer_cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let sys_samples = drain_n(&mixer_system, MIXER_FRAME_SAMPLES);
                    let mic_samples = drain_n(&mixer_mic, MIXER_FRAME_SAMPLES);
                    let mixed: Vec<f32> = (0..MIXER_FRAME_SAMPLES)
                        .map(|i| (sys_samples[i] + mic_samples[i]).clamp(-1.0, 1.0))
                        .collect();
                    let pcm = floats_to_s16le_bytes(&mixed);
                    match mixer_tx.try_send(pcm) {
                        Ok(()) => {}
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            // Output channel back-pressured; drop frame silently.
                            // Logged once + every 50 drops so sustained issues surface.
                            static DROPPED: AtomicU64 = AtomicU64::new(0);
                            let d = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
                            if d == 1 || d % 50 == 0 {
                                tracing::warn!(
                                    dropped = d,
                                    "audio: mixer output mpsc full; dropping frame"
                                );
                            }
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            // Receiver gone (e.g. session ended). Stop the loop.
                            break;
                        }
                    }
                }
            }
        }
    });

    // 7. Spawn supervisor: stop SCStream when cancel fires. SCStream is !Send,
    // so we use a std::thread to bridge from sync to the cancel future.
    let stop_handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        stop_handle.block_on(cancel.cancelled());
        let _ = stream.stop_capture();
    });

    Ok(rx)
}

/// Convert a CMSampleBuffer's Float32 samples to mono 16k Float32 and append
/// to `ring`. Drops oldest samples if the ring exceeds `RING_CAP_SAMPLES`.
/// Called from SCKit's GCD thread (no tokio context here).
fn push_into_ring(sample: &screencapturekit::cm::CMSampleBuffer, ring: &Mutex<VecDeque<f32>>) {
    let (sample_rate, channels) = match sample.format_description() {
        Some(fd) => (
            fd.audio_sample_rate().unwrap_or(48000.0) as u32,
            fd.audio_channel_count().unwrap_or(2) as u16,
        ),
        None => (48000, 2),
    };
    let abl = match sample.audio_buffer_list() {
        Some(a) => a,
        None => return,
    };
    let mut floats = Vec::<f32>::new();
    for ab in abl.iter() {
        let count = ab.data_byte_size() / std::mem::size_of::<f32>();
        if count == 0 {
            continue;
        }
        let bytes = ab.data();
        // SAFETY: SCKit guarantees Float32 LPCM in the buffer per the format
        // description we asked for.
        let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) };
        floats.extend_from_slice(slice);
    }
    if floats.is_empty() {
        return;
    }
    let mono = to_mono_16k_f32(&floats, sample_rate, channels);
    if let Ok(mut r) = ring.lock() {
        r.extend(mono);
        // Cap at RING_CAP_SAMPLES; drop oldest when full.
        while r.len() > RING_CAP_SAMPLES {
            r.pop_front();
        }
    }
}

/// Drain `n` samples from `ring`. Pads with zeros if fewer than `n` are
/// available (silence from this source for that frame).
fn drain_n(ring: &Mutex<VecDeque<f32>>, n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    if let Ok(mut r) = ring.lock() {
        let take = n.min(r.len());
        out.extend(r.drain(..take));
    }
    if out.len() < n {
        out.resize(n, 0.0);
    }
    out
}
