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
//!
//! This is a one-off spike for Task 9b1 — see plan
//! docs/superpowers/plans/2026-05-03-phase-2-step-15.md.

#[cfg(target_os = "macos")]
fn main() {
    use screencapturekit::prelude::*;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
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
        .with_sample_rate(48000)
        .with_channel_count(2);

    eprintln!(
        "[spike] Stream config: captures_audio={}, sample_rate={}Hz, channels={}",
        config.captures_audio(),
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
    let audio_frame_count_clone = audio_frame_count.clone();

    let mut stream = SCStream::new(&filter, &config);

    // Audio output handler — prints per-buffer stats to stderr.
    stream.add_output_handler(
        move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
            if output_type != SCStreamOutputType::Audio {
                return;
            }

            let idx = audio_frame_count_clone.fetch_add(1, Ordering::Relaxed);

            // Compute total bytes across all AudioBuffers in this sample.
            let total_bytes: usize = if let Some(abl) = sample.audio_buffer_list() {
                abl.iter().map(|ab| ab.data_byte_size()).sum()
            } else {
                // Fallback: use total_sample_size if audio_buffer_list is unavailable.
                sample.total_sample_size()
            };

            // Extract sample rate + channel count from the CMFormatDescription.
            let (sample_rate, channels) = sample
                .format_description()
                .map(|fd| {
                    let sr = fd.audio_sample_rate().unwrap_or(0.0);
                    let ch = fd.audio_channel_count().unwrap_or(0);
                    (sr, ch)
                })
                .unwrap_or((0.0, 0));

            eprintln!(
                "[spike] audio frame #{idx:04}: {total_bytes} bytes, \
                 {sample_rate:.0} Hz, {channels}ch"
            );
        },
        SCStreamOutputType::Audio,
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
    eprintln!("[spike] Spike complete.");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This spike only runs on macOS (ScreenCaptureKit is Apple-only).");
    std::process::exit(1);
}
