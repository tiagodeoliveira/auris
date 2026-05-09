// Audio capture surface for the mobile client. Phase 3 of MOBILE-PLAN.
//
// **Stubbed pending SDK upgrade.** `expo-audio`'s earliest version
// targets SDK 52+; we're on SDK 51. The module is a no-op:
// `requestMicPermission()` always returns "granted", `start`/`stop`
// are no-ops, peak stays at 0. Fixes the Android Gradle build that
// was breaking on the expo-audio / expo-modules-core peer mismatch.
//
// Restoration path (separate iteration):
//   1. Bump expo SDK to 52+ via `pnpm dlx expo install expo@^52 --fix`,
//      OR fall back to expo-av (deprecated but SDK-51 compatible).
//   2. Re-add the recorder + metering hookup (ten or twenty lines —
//      kept the API shape so call sites won't change).
//   3. Restore the config plugin entry in app.json.
//
// The interface intentionally matches what the meeting modal calls
// today so re-enabling is a one-file edit.

export type FrameHandler = (frame: Int16Array) => void;

export interface CaptureHandlers {
  onFrame?: FrameHandler;
}

export type MicPermission = "granted" | "denied" | "undetermined";

export async function requestMicPermission(): Promise<MicPermission> {
  // Stubbed: pretend permission is granted. The real prompt fires
  // when the recorder hookup is restored. Until then `start()` /
  // `stop()` no-op, so claiming "granted" doesn't actually attempt
  // any capture.
  return "granted";
}

export function useAudioCapture(_handlers: CaptureHandlers = {}) {
  return {
    peak: 0,
    isRecording: false,
    async start() {
      // no-op until expo-audio (or expo-av) is wired back in
    },
    async stop() {
      // no-op
    },
  };
}
