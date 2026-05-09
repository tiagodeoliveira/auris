// Audio capture surface for the mobile client. Phase 3 of MOBILE-PLAN.
//
// What this module provides today (foundational slice):
//   - Mic permission request flow.
//   - A recorder lifecycle (`start` / `stop`) backed by `expo-audio`.
//   - A subscription API that publishes the live peak amplitude
//     (0–1) so the UI can show a level meter.
//
// What's NOT here yet (deferred until device-test iteration):
//   - Raw PCM frame extraction. `expo-audio` records to a file and
//     publishes metering — it doesn't expose per-buffer PCM bytes
//     in its public JS API. To get raw frames we'll need either
//     (a) a config plugin around `react-native-live-audio-stream`,
//     or (b) a custom dev-client native module that bridges
//     AVAudioEngine's per-tap callbacks. Both require a fresh
//     dev-client build — easier to iterate on after first end-to-end
//     UI smoke test on a real device.
//   - Resampling 44.1k/48k → 16k mono. Lands alongside the PCM
//     frame extraction; the resampler's a pure JS function we can
//     port from the PWA when we have frames to feed it.
//   - VAD gating. Same — needs PCM input to gate.
//   - /stt WebSocket frame upload. The store / meeting screen will
//     wire it up once frames are available.
//
// The module's *interface* is shaped so the future PCM path slots
// in without changing call sites:
//   - `start(handlers)` accepts an `onFrame(pcm: Int16Array)` hook
//     that's currently a no-op. When real PCM lands, the same hook
//     fires with frames.

import { useEffect, useState } from "react";
import {
  AudioModule,
  RecordingPresets,
  setAudioModeAsync,
  useAudioRecorder,
  useAudioRecorderState,
} from "expo-audio";

export type FrameHandler = (frame: Int16Array) => void;

export interface CaptureHandlers {
  /// Fired on each PCM frame after VAD gating. **Not yet wired in
  /// Phase 3** — see module header. Provided for forward-compat so
  /// the call site doesn't change when streaming lands.
  onFrame?: FrameHandler;
}

/// Result of the permission request. Mirrors the Mac client's
/// permission state for cross-client review symmetry.
export type MicPermission = "granted" | "denied" | "undetermined";

export async function requestMicPermission(): Promise<MicPermission> {
  const status = await AudioModule.requestRecordingPermissionsAsync();
  if (status.granted) return "granted";
  if (status.canAskAgain) return "undetermined";
  return "denied";
}

/// React hook wrapping `expo-audio`'s recorder. Returns the live
/// peak amplitude (-160..0 dB scale, normalized to 0..1) plus the
/// recorder controls.
///
/// Caller responsibility: only call `start()` after
/// `requestMicPermission()` resolves to `"granted"`. The hook
/// doesn't auto-prompt — the meeting modal owns the timing of the
/// first prompt (right when the user taps Start).
export function useAudioCapture(_handlers: CaptureHandlers = {}) {
  // 16kHz mono is what the server's STT pipeline accepts. The OS
  // may resample under the hood (most iOS hardware records at 48k);
  // expo-audio's preset hides that detail. When raw-PCM streaming
  // lands the `_handlers.onFrame` hook will get frames at this
  // sample rate, post-resample.
  const recorder = useAudioRecorder({
    ...RecordingPresets.LOW_QUALITY,
    sampleRate: 16_000,
    numberOfChannels: 1,
    isMeteringEnabled: true,
  });
  const recorderState = useAudioRecorderState(recorder, /* updateInterval */ 100);

  // expo-audio's metering returns dB SPL on a -160..0 scale. Map to
  // a 0..1 amplitude for a peak meter; below -60 dB rounds to 0
  // (silence floor — anything quieter is below the typical mic
  // noise floor anyway).
  const [peak, setPeak] = useState(0);
  useEffect(() => {
    const m = recorderState.metering;
    if (m === undefined || m === null) {
      setPeak(0);
      return;
    }
    const SILENCE_DB = -60;
    if (m <= SILENCE_DB) {
      setPeak(0);
      return;
    }
    setPeak(Math.min(1, Math.max(0, (m - SILENCE_DB) / -SILENCE_DB)));
  }, [recorderState.metering]);

  return {
    peak,
    isRecording: recorderState.isRecording,
    /// Configure the iOS audio session for foreground recording +
    /// kick the recorder off. Idempotent — calling twice in a row
    /// no-ops the second start.
    async start() {
      if (recorderState.isRecording) return;
      // Allow recording + don't lower to bg-audio mode. The session
      // category = "playAndRecord" is what unlocks reading the mic
      // while the screen stays on. Lock-screen / background audio
      // continuation is a Phase 6 problem.
      await setAudioModeAsync({ allowsRecording: true });
      await recorder.prepareToRecordAsync();
      recorder.record();
    },
    async stop() {
      if (!recorderState.isRecording) return;
      await recorder.stop();
    },
  };
}
