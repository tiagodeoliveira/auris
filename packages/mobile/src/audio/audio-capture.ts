// Mic-only audio capture for the mobile client.
//
// SCOPE: microphone only. iOS exposes no public API for capturing
// audio from other apps (Zoom, FaceTime, phone calls), and Android's
// MediaProjection explicitly excludes `USAGE_VOICE_COMMUNICATION`
// apps from system-audio capture. If users want to record system
// audio for a meeting, they pick a Mac (via the audio-source picker)
// as the capture device; this hook is for the phone's own mic only.
//
// Wire format: matches Mac AudioStreamer + server `/audio` endpoint:
// binary PCM 16 kHz mono S16LE frames, ~640 bytes (20 ms) each.
// See packages/server/src/audio/remote.rs for the receive side.
//
// Background behavior:
//   - iOS: `UIBackgroundModes=audio` in Info.plist + AVAudioSession
//     `record` (which @siteed/expo-audio-studio configures
//     internally when recording starts) keeps the mic alive while
//     backgrounded.
//   - Android: expo-audio's config plugin with
//     `enableBackgroundRecording: true` adds the foreground service
//     + notification, so the recording survives the app being
//     swiped away from the recents view.

import { LegacyEventEmitter, type EventSubscription } from "expo-modules-core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as auth0 from "../auth/auth0";
import { serverUrl } from "../config";
import { AudioStreamer } from "./audio-streamer";
import {
  FRAME_STALL_MESSAGE,
  FRAME_STALL_THRESHOLD_MS,
  WATCHDOG_INTERVAL_MS,
  interruptionStatus,
  isFrameStalled,
  isInterruptionMessage,
} from "./interruption";
import { buildRecordingOptions, type RecordingOptions } from "./recording-options";

// ── Lazy module access ─────────────────────────────────────────────
//
// We `require()` the native module inside try/catch so that:
//   (a) the JS bundle still loads if the user hasn't yet run
//       `pnpm install` after adding the dep, and
//   (b) tests that don't link the native module can import this
//       file without blowing up.
//
// In practice both modules are linked in EAS builds; the fallback
// branch only matters for first-touch dev ergonomics.

/// Payload shape emitted by the @siteed/audio-studio iOS module via
/// `sendEvent("AudioData", ...)`. The base64-encoded PCM lives on the
/// `encoded` field (NOT `data` — that was the earlier guess that
/// silently dropped every frame). Float32 mode would deliver
/// `pcmFloat32` instead, but we keep streamFormat at the default
/// (S16LE) so we only handle `encoded` here. See:
/// node_modules/@siteed/audio-studio/ios/AudioStudioModule.swift:1190
type AudioDataEvent = {
  encoded?: string;
  position?: number;
};

interface ExpoAudioStudioModuleLike {
  requestPermissionsAsync(): Promise<{ status: "granted" | "denied" | "undetermined" }>;
  getPermissionsAsync?(): Promise<{ status: "granted" | "denied" | "undetermined" }>;
  startRecording(opts: RecordingOptions): Promise<{ uri?: string } | unknown>;
  stopRecording(): Promise<unknown>;
}

/// One emitter per native module, cached across hook mounts. The
/// native module is a singleton; multiple emitters on the same module
/// would each receive every event independently, wasting work. We
/// build the emitter the first time it's needed and reuse it.
///
/// LegacyEventEmitter (the lib uses it too — see
/// node_modules/@siteed/audio-studio/src/events.ts) wraps the native
/// module's auto-generated `addListener`/`removeListeners` so the
/// "AudioData" events emitted by ios/AudioStudioModule.swift:1197
/// reach JS-side handlers.
let cachedEmitter: LegacyEventEmitter | null = null;
function getAudioEmitter(studio: ExpoAudioStudioModuleLike): LegacyEventEmitter {
  if (!cachedEmitter) {
    // LegacyEventEmitter's NativeModule signature is structural; the
    // Expo-managed module satisfies it at runtime. TS can't see that
    // because we typed `studio` as a narrowed interface above.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    cachedEmitter = new LegacyEventEmitter(studio as any);
  }
  return cachedEmitter;
}

function loadStudio(): ExpoAudioStudioModuleLike | null {
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const mod = require("@siteed/expo-audio-studio");
    // The package re-exports both class-style `ExpoAudioStreamModule`
    // and a default. Prefer the module export; fall back to default.
    const candidate = mod.ExpoAudioStreamModule ?? mod.default ?? mod;
    if (
      candidate &&
      typeof candidate.startRecording === "function" &&
      typeof candidate.stopRecording === "function" &&
      typeof candidate.requestPermissionsAsync === "function"
    ) {
      return candidate as ExpoAudioStudioModuleLike;
    }
    return null;
  } catch {
    return null;
  }
}

// ── Public types ───────────────────────────────────────────────────

export type FrameHandler = (frame: Int16Array) => void;

export interface CaptureHandlers {
  /// Called for every PCM frame the mic produces. Mostly useful for
  /// tests; production code lets the hook stream straight to the WS.
  onFrame?: FrameHandler;
}

export type MicPermission = "granted" | "denied" | "undetermined";

export async function requestMicPermission(): Promise<MicPermission> {
  const studio = loadStudio();
  if (!studio) {
    // Module not linked yet (dev ergonomics; see file header). We
    // can't honestly claim permission, so we surface "undetermined"
    // so the caller's flow falls back to "ask later".
    return "undetermined";
  }
  try {
    const res = await studio.requestPermissionsAsync();
    return (res.status ?? "undetermined") as MicPermission;
  } catch (e) {
    console.warn("[audio-capture] permission request failed", e);
    return "denied";
  }
}

// ── Hook ───────────────────────────────────────────────────────────

interface UseAudioCaptureReturn {
  /// Linear 0..1 peak/level for the meter UI. Smoothed with a short
  /// decay so the bar doesn't flicker between frames.
  peak: number;
  isRecording: boolean;
  /// Non-null while a recoverable error is sticky on the UI (e.g.
  /// "permission denied", "mic unavailable"). Cleared on next start.
  error: string | null;
  start(): Promise<void>;
  stop(): Promise<void>;
  /// Re-exposed so callers don't have to import the standalone helper.
  requestMicPermission(): Promise<MicPermission>;
}

/// Fast linear blend for the meter. Rises instantly to a louder
/// sample, decays geometrically toward 0 between callbacks.
const PEAK_DECAY = 0.6;

export function useAudioCapture(handlers: CaptureHandlers = {}): UseAudioCaptureReturn {
  const [peak, setPeak] = useState(0);
  const [isRecording, setIsRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Stable refs so the captured-in-callback values don't go stale
  // across renders. `handlersRef` lets the consumer pass a fresh
  // closure each render without us having to restart the recorder.
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  // Audio event subscription. Held in a ref so start() can install it
  // and stop()/unmount can remove it without re-running the effect.
  const subscriptionRef = useRef<EventSubscription | null>(null);

  // Interruption lifecycle subscription ("onRecordingInterrupted").
  // Separate ref from subscriptionRef so audio frames and lifecycle
  // events tear down independently but symmetrically.
  const interruptionSubRef = useRef<EventSubscription | null>(null);

  // Frame-stall watchdog: lastFrameAtRef is bumped by every
  // AudioData frame (and seeded when recording starts); the interval
  // raises a sticky error if frames stop arriving while recording is
  // nominally live — catching auto-resume failures, route loss, or
  // any other silent mic death the lib mishandles. Note: if iOS
  // suspends the app (interrupted session no longer holds the
  // background-audio entitlement), JS timers stop too — at that
  // point the /audio WS drops and the server-side liveness reaper
  // takes over.
  const lastFrameAtRef = useRef<number | null>(null);
  const watchdogRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const stopWatchdog = useCallback(() => {
    if (watchdogRef.current) {
      clearInterval(watchdogRef.current);
      watchdogRef.current = null;
    }
    lastFrameAtRef.current = null;
  }, []);

  const startWatchdog = useCallback(() => {
    stopWatchdog();
    // Seed with "now" so a recording that never produces a single
    // frame still trips the stall threshold.
    lastFrameAtRef.current = Date.now();
    watchdogRef.current = setInterval(() => {
      const stalled = isFrameStalled(
        lastFrameAtRef.current,
        Date.now(),
        true, // this interval only runs while recording (see start/stop)
        FRAME_STALL_THRESHOLD_MS,
      );
      setError((prev) => {
        if (stalled) {
          // An interruption banner is more specific than the generic
          // stall message — keep it. (The interruption "ended" event
          // clears it; if frames then still don't return, the next
          // tick raises the stall message.)
          return isInterruptionMessage(prev) ? prev : FRAME_STALL_MESSAGE;
        }
        // Frames are flowing again: retract only our own message.
        return prev === FRAME_STALL_MESSAGE ? null : prev;
      });
    }, WATCHDOG_INTERVAL_MS);
  }, [stopWatchdog]);

  const streamer = useMemo(
    () =>
      new AudioStreamer({
        serverUrl,
        getAccessToken: () => auth0.getAccessToken(),
      }),
    [],
  );

  // Always stop on unmount — never want the mic hanging around after
  // the meeting screen unmounts.
  useEffect(() => {
    return () => {
      subscriptionRef.current?.remove();
      subscriptionRef.current = null;
      interruptionSubRef.current?.remove();
      interruptionSubRef.current = null;
      stopWatchdog();
      streamer.stop();
      const studio = loadStudio();
      if (studio) {
        // Best-effort; errors here are silent because there's
        // nothing the caller can do during teardown.
        void studio.stopRecording().catch(() => {});
      }
    };
  }, [streamer, stopWatchdog]);

  const start = useCallback(async () => {
    const studio = loadStudio();
    if (!studio) {
      setError(
        "Audio module not linked. Run `pnpm install` then rebuild the native app (eas build / expo run).",
      );
      return;
    }

    // Prompt for permission inline so callers don't have to. Mac/
    // PWA follow the same pattern: the start path is the one and
    // only permission entry point.
    let perm: MicPermission;
    try {
      perm = (await studio.requestPermissionsAsync()).status as MicPermission;
    } catch (e) {
      setError(`Mic permission error: ${(e as Error).message ?? String(e)}`);
      return;
    }
    if (perm !== "granted") {
      setError("Microphone access denied");
      return;
    }

    setError(null);
    streamer.start();

    // Subscribe BEFORE startRecording so we don't miss the first
    // frame. The native module emits "AudioData" events with a
    // base64 payload on `encoded`; the per-frame `onAudioStream`
    // option we used to pass was silently dropped by the lib's
    // JSON-clean step (functions can't cross the native bridge),
    // so frames never reached our handler before this fix.
    subscriptionRef.current?.remove();
    subscriptionRef.current = getAudioEmitter(studio).addListener<AudioDataEvent>(
      "AudioData",
      (e) => handleAudioStream(e, streamer, setPeak, handlersRef, lastFrameAtRef),
    );

    // Lifecycle events: the native module pauses on OS interruptions
    // (phone call, Siri, audio-focus loss) and emits
    // "onRecordingInterrupted" with { reason, isPaused }. We surface
    // a sticky banner while paused and clear it on resume. NOTE:
    // like onAudioStream above, the lib's `onRecordingInterrupted`
    // config *callback* cannot be passed through startRecording —
    // functions are stripped before crossing the bridge — so the
    // emitter subscription is the only mechanism that works.
    interruptionSubRef.current?.remove();
    interruptionSubRef.current = getAudioEmitter(studio).addListener<{
      reason?: string;
      isPaused?: boolean;
    }>("onRecordingInterrupted", (e) => setError((prev) => interruptionStatus(prev, e)));

    try {
      // Options (incl. autoResumeAfterInterruption: true) are pinned
      // by recording-options.test.ts — do not inline them here again.
      await studio.startRecording(buildRecordingOptions());
      setIsRecording(true);
      startWatchdog();
    } catch (e) {
      subscriptionRef.current?.remove();
      subscriptionRef.current = null;
      interruptionSubRef.current?.remove();
      interruptionSubRef.current = null;
      stopWatchdog();
      streamer.stop();
      setError(`Failed to start recording: ${(e as Error).message ?? String(e)}`);
      setIsRecording(false);
    }
  }, [streamer, startWatchdog, stopWatchdog]);

  const stop = useCallback(async () => {
    const studio = loadStudio();
    subscriptionRef.current?.remove();
    subscriptionRef.current = null;
    interruptionSubRef.current?.remove();
    interruptionSubRef.current = null;
    stopWatchdog();
    streamer.stop();
    setIsRecording(false);
    setPeak(0);
    if (!studio) return;
    try {
      await studio.stopRecording();
    } catch (e) {
      // Logged but not surfaced — stop should always look like it
      // succeeded from the UI's perspective.
      console.warn("[audio-capture] stopRecording failed", e);
    }
  }, [streamer, stopWatchdog]);

  return {
    peak,
    isRecording,
    error,
    start,
    stop,
    requestMicPermission,
  };
}

// ── Internals ──────────────────────────────────────────────────────

function handleAudioStream(
  e: AudioDataEvent,
  streamer: AudioStreamer,
  setPeak: (updater: (prev: number) => number) => void,
  handlersRef: { current: CaptureHandlers },
  lastFrameAtRef: { current: number | null },
): void {
  // The iOS native module emits the base64 PCM under `encoded`. If
  // we ever switch to `streamFormat="float32"` we'd need to read
  // `pcmFloat32` instead — but we stay at S16LE to match Mac + server.
  if (!e.encoded) return;

  // Feed the stall watchdog: any frame — even one that fails base64
  // decode below — proves the native capture side is alive.
  lastFrameAtRef.current = Date.now();

  let bytes: Uint8Array | null = null;
  try {
    bytes = base64ToBytes(e.encoded);
  } catch {
    bytes = null;
  }
  if (!bytes || bytes.byteLength === 0) return;

  // Forward to the streamer's WS unchanged. Server expects PCM16
  // LE @ 16 kHz mono; the recording config above matches.
  streamer.feed(bytes);

  // Hand a typed view to local handlers (tests / future on-device
  // VAD) without re-copying the buffer. Note: Int16Array view over a
  // Uint8Array buffer only works if the byteOffset is even, which it
  // always is here since we constructed `bytes` from base64.
  const onFrame = handlersRef.current.onFrame;
  if (onFrame) {
    const i16 = new Int16Array(bytes.buffer, bytes.byteOffset, bytes.byteLength >> 1);
    onFrame(i16);
  }

  // Update the meter. The "AudioData" event payload doesn't carry a
  // soundLevel field, so we compute peak directly from the samples.
  const next = peakFromSamples(bytes);
  setPeak((prev) => Math.max(next, prev * PEAK_DECAY));
}

function peakFromSamples(bytes: Uint8Array): number {
  // Sample-domain peak: scan Int16 samples for the abs-max.
  // ~3200 samples per 100 ms callback at 16 kHz mono — cheap.
  let max = 0;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (let i = 0; i + 1 < view.byteLength; i += 2) {
    const s = view.getInt16(i, true);
    const a = s < 0 ? -s : s;
    if (a > max) max = a;
  }
  return Math.min(1, max / 32_768);
}

/// RN doesn't ship `atob` reliably; this is the well-trodden
/// alphabet-table decoder. Faster than calling out to `Buffer` and
/// avoids pulling node polyfills.
const BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
let base64Lookup: Int8Array | null = null;
function base64ToBytes(b64: string): Uint8Array {
  if (!base64Lookup) {
    const tbl = new Int8Array(256).fill(-1);
    for (let i = 0; i < BASE64_ALPHABET.length; i++) {
      tbl[BASE64_ALPHABET.charCodeAt(i)] = i;
    }
    base64Lookup = tbl;
  }
  const tbl = base64Lookup;
  // Strip padding-only chars without allocating a new string when
  // there aren't any.
  let end = b64.length;
  while (end > 0 && b64.charCodeAt(end - 1) === 61 /* '=' */) end--;
  const outLen = (end * 3) >> 2;
  const out = new Uint8Array(outLen);
  let oi = 0;
  let buf = 0;
  let bits = 0;
  for (let i = 0; i < end; i++) {
    const v = tbl[b64.charCodeAt(i)];
    if (v < 0) continue; // skip whitespace / stray chars
    buf = (buf << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[oi++] = (buf >> bits) & 0xff;
    }
  }
  return out.subarray(0, oi);
}
