// startRecording options for @siteed/audio-studio, extracted from
// useAudioCapture so they are pure and pinned by unit tests
// (recording-options.test.ts).

/// Subset of the lib's RecordingConfig that this app uses. Kept as a
/// local structural type (rather than importing the lib's types) so
/// this module stays importable in the node-side vitest runner and
/// resilient to lib type churn.
export interface RecordingOptions {
  sampleRate: number;
  channels: number;
  encoding: string;
  interval?: number;
  keepAwake?: boolean;
  /// The native module pauses recording on OS audio-session
  /// interruptions (incoming call, Siri, audio-focus loss) and only
  /// auto-resumes afterwards when this is true. The lib default is
  /// FALSE on both platforms (ios/RecordingSettings.swift:171,
  /// android RecordingConfig.kt:151), which is how a phone call
  /// used to silently kill the rest of the meeting (#196).
  autoResumeAfterInterruption?: boolean;
}

export function buildRecordingOptions(): RecordingOptions {
  return {
    // Wire format must match Mac AudioStreamer + the server's
    // /audio endpoint: PCM 16 kHz mono S16LE. See
    // packages/server/src/audio/remote.rs for the receive side.
    sampleRate: 16_000,
    channels: 1,
    encoding: "pcm_16bit",
    interval: 100, // ~10 callbacks/s → low bridge overhead
    keepAwake: true,
    autoResumeAfterInterruption: true,
  };
}
