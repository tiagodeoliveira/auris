//! Spike: prove ScreenCaptureKit audio capture works from Rust.
//! Captures system audio + mic for ~5 seconds, prints frame stats to stderr.
//!
//! REQUIRES macOS Screen Recording permission. The first run will silently fail
//! (or prompt) — grant the permission in:
//!   System Settings → Privacy & Security → Screen Recording
//! then restart your terminal app and re-run this binary.
//!
//! Usage:
//!   cargo run -p meeting-companion-server --example screencapturekit_spike

#[cfg(target_os = "macos")]
fn main() {
    use meeting_companion_server::audio::format::convert_to_stt_pcm;
    use screencapturekit::prelude::*;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    eprintln!("[spike] ScreenCaptureKit audio-only spike starting…");
    eprintln!("[spike] Requesting shareable content (triggers TCC check)…");

    // -------------------------------------------------------------------------
    // 1. Enumerate available displays so we can build a content filter.
    //    SCShareableContent::get() is a synchronous call that will return an
    //    error if Screen Recording permission has not been granted yet.
    // -------------------------------------------------------------------------
    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[spike] ERROR: Could not get shareable content: {e}\n\
                 \n\
                 This usually means Screen Recording permission has not been granted.\n\
                 Fix:\n\
                   1. Open System Settings → Privacy & Security → Screen Recording\n\
                   2. Enable the toggle for your terminal app (Terminal, iTerm2, etc.)\n\
                   3. Restart your terminal and re-run:\n\
                      cargo run -p meeting-companion-server --example screencapturekit_spike"
            );
            std::process::exit(1);
        }
    };

    let displays = content.displays();
    if displays.is_empty() {
        eprintln!("[spike] ERROR: No displays found — cannot build a content filter.");
        std::process::exit(1);
    }
    let display = &displays[0];
    eprintln!(
        "[spike] Using display {} ({}x{})",
        display.display_id(),
        display.width(),
        display.height()
    );

    // -------------------------------------------------------------------------
    // 2. Build SCContentFilter — target the whole display (audio follows
    //    the display filter; ScreenCaptureKit 12.3+ always delivers system
    //    audio alongside whatever display is selected).
    // -------------------------------------------------------------------------
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();

    // -------------------------------------------------------------------------
    // 3. Build SCStreamConfiguration with audio enabled, video disabled.
    //    Setting width/height to 1 is the idiomatic way to suppress video
    //    frames while keeping the stream alive for audio delivery.
    //    48 kHz stereo Float32 LPCM is what ScreenCaptureKit delivers by default.
    // -------------------------------------------------------------------------
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_captures_microphone(true)
        .with_sample_rate(48000)
        .with_channel_count(2);

    eprintln!(
        "[spike] Stream config: captures_audio={}, captures_microphone={}, sample_rate={}Hz, channels={}",
        config.captures_audio(),
        config.captures_microphone(),
        config.sample_rate(),
        config.channel_count(),
    );

    // -------------------------------------------------------------------------
    // 4. Create SCStream and register output handlers — one for system audio,
    //    one for screen (required by ScreenCaptureKit even for audio-only use).
    //    The Microphone output type requires macOS 15+ and a separate
    //    with_captures_microphone(true) config flag; we skip it in this spike
    //    to stay compatible with macOS 13+.
    // -------------------------------------------------------------------------
    let audio_frame_count = Arc::new(AtomicUsize::new(0));

    // Accumulator for converted-to-Soniox PCM (16 kHz mono S16LE).
    // Writing this to a WAV file at the end lets us listen to exactly what
    // the production server would forward to Soniox during a real meeting.
    let pcm_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    let mut stream = SCStream::new(&filter, &config);

    // Process either an Audio (system) or Microphone sample buffer:
    // accumulate converted PCM bytes for WAV export and print stats.
    let process_sample = {
        let pcm_buffer_h = Arc::clone(&pcm_buffer);
        let audio_frame_count_clone = audio_frame_count.clone();
        move |sample: CMSampleBuffer, label: &'static str| {
            let idx = audio_frame_count_clone.fetch_add(1, Ordering::Relaxed);

            let total_bytes: usize = if let Some(abl) = sample.audio_buffer_list() {
                abl.iter().map(|ab| ab.data_byte_size()).sum()
            } else {
                sample.total_sample_size()
            };

            let (sample_rate, channels) = sample
                .format_description()
                .map(|fd| {
                    let sr = fd.audio_sample_rate().unwrap_or(0.0);
                    let ch = fd.audio_channel_count().unwrap_or(0);
                    (sr, ch)
                })
                .unwrap_or((0.0, 0));

            eprintln!(
                "[spike] {label} frame #{idx:04}: {total_bytes} bytes, \
                 {sample_rate:.0} Hz, {channels}ch"
            );

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
                // SAFETY: SCKit guarantees Float32 LPCM per the format description.
                let slice =
                    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) };
                floats.extend_from_slice(slice);
            }
            if floats.is_empty() {
                return;
            }
            let pcm = convert_to_stt_pcm(&floats, sample_rate as u32, channels as u16);
            if let Ok(mut buf) = pcm_buffer_h.lock() {
                buf.extend_from_slice(&pcm);
            }
        }
    };

    // Microphone-only capture for now. System audio + mic naive interleaving
    // produces 2x-time playback because both sources fire at ~50fps and we'd
    // concatenate all their frames into the same stream. Real mixing requires
    // a sample-buffer summer running at fixed 50fps — deferred to a follow-up.
    // Note: a no-op Audio handler is still required by ScreenCaptureKit for
    // streams with `with_captures_audio(true)`; we discard those frames.
    stream.add_output_handler(
        |_sample: CMSampleBuffer, _output_type: SCStreamOutputType| {
            // intentionally empty — system audio frames discarded for now
        },
        SCStreamOutputType::Audio,
    );
    let process_mic = process_sample;
    stream.add_output_handler(
        move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
            if output_type != SCStreamOutputType::Microphone {
                return;
            }
            process_mic(sample, "mic");
        },
        SCStreamOutputType::Microphone,
    );

    // Screen output handler — we must register one because ScreenCaptureKit
    // requires at least one Screen handler when a display filter is used.
    // We simply discard video frames here.
    stream.add_output_handler(
        |_sample: CMSampleBuffer, _output_type: SCStreamOutputType| {
            // intentionally empty — video frames discarded
        },
        SCStreamOutputType::Screen,
    );

    // -------------------------------------------------------------------------
    // 5. Start capture.
    // -------------------------------------------------------------------------
    eprintln!("[spike] Starting capture…");
    match stream.start_capture() {
        Ok(()) => eprintln!("[spike] Capture started successfully."),
        Err(e) => {
            eprintln!(
                "[spike] ERROR: start_capture failed: {e}\n\
                 \n\
                 If this is error -3801 (SCStreamErrorUserDeclined) or similar,\n\
                 Screen Recording permission was denied. See instructions above."
            );
            std::process::exit(1);
        }
    }

    // -------------------------------------------------------------------------
    // 6. Run for 5 seconds (no tokio required — plain std::thread::sleep).
    // -------------------------------------------------------------------------
    eprintln!("[spike] Capturing for 5 seconds…");
    std::thread::sleep(Duration::from_secs(5));

    // -------------------------------------------------------------------------
    // 7. Stop capture cleanly.
    // -------------------------------------------------------------------------
    eprintln!("[spike] Stopping capture…");
    match stream.stop_capture() {
        Ok(()) => eprintln!("[spike] Capture stopped cleanly."),
        Err(e) => eprintln!("[spike] Warning: stop_capture returned error: {e}"),
    }

    let total = audio_frame_count.load(Ordering::Relaxed);
    eprintln!("[spike] Total audio frames received: {total}");

    // -------------------------------------------------------------------------
    // 8. Dump accumulated PCM to a WAV file for sanity check.
    //    The bytes are exactly what production would forward to Soniox.
    // -------------------------------------------------------------------------
    let pcm_bytes = pcm_buffer.lock().unwrap().clone();
    let wav_path = "/tmp/spike-audio.wav";
    match write_wav_s16le_mono_16k(wav_path, &pcm_bytes) {
        Ok(()) => eprintln!(
            "[spike] Wrote {} bytes of PCM (S16LE 16k mono) to {}",
            pcm_bytes.len(),
            wav_path
        ),
        Err(e) => eprintln!("[spike] WAV write failed: {e}"),
    }

    eprintln!("[spike] Spike complete.");
}

/// Write a minimal RIFF/WAV header for S16LE PCM at 16 kHz mono, followed
/// by the raw PCM bytes. ~30 lines, no extra dependency.
#[cfg(target_os = "macos")]
fn write_wav_s16le_mono_16k(path: &str, pcm: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    let data_size = pcm.len() as u32;
    let total_size = data_size + 36; // 36 = header size minus the leading 8 bytes of RIFF
    file.write_all(b"RIFF")?;
    file.write_all(&total_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    file.write_all(&1u16.to_le_bytes())?; // PCM format code
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&16000u32.to_le_bytes())?; // sample rate
    file.write_all(&32000u32.to_le_bytes())?; // byte rate (16000 * 1 * 2)
    file.write_all(&2u16.to_le_bytes())?; // block align (1 * 2)
    file.write_all(&16u16.to_le_bytes())?; // bits per sample
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;
    file.write_all(pcm)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This spike only runs on macOS (ScreenCaptureKit is Apple-only).");
    std::process::exit(1);
}
