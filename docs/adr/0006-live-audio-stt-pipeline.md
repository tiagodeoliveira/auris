# ADR-0006: Live audio + STT pipeline

**Status:** Accepted
**Date:** 2026-05-03

## Context

The server needs to capture meeting audio and turn it into a streaming
transcript with sentence-level boundaries. The output drives both the
real-time transcript mode and the rolling input that the summarizers
consume.

Specific constraints on the audio source:

- **The user's voice and the other party's voice both matter.** Many
  meetings happen via Zoom / Slack / Meet — the local laptop is the
  capture point. We need _system audio_ (the remote speaker's stream) and
  _microphone_ (the local speaker) to be combined into one transcript;
  capturing only one half loses the conversation.
- **macOS-first.** The author's laptop is a Mac. ScreenCaptureKit is the
  only documented Apple API that captures system audio without a virtual
  audio device. It also supports microphone capture via the same
  `SCStream`, but only on macOS 15+ with the right cargo feature.
- **STT must be streaming with finalized + interim tokens.** Sentence
  flushing is downstream of "finalized tokens since the last sentence
  boundary"; a non-streaming STT (post-processing the full audio at
  meeting end) would break the live-summarization story.
- **STT provider must be swappable.** Soniox is the production choice,
  but a mock backend is essential for offline development and CI.

## Decision

- **Audio capture: ScreenCaptureKit via the [`screencapturekit`
  crate](https://crates.io/crates/screencapturekit), feature
  `macos_15_0` enabled.** Single `SCStream` configured with both
  `system_audio: true` and `captures_microphone: true`. Output type:
  `SampleBuffer` for both audio types.
- **In-process audio mixer.** System audio and microphone arrive on
  separate frame callbacks at independent rates. A small ring buffer per
  source feeds a 50 fps mixer that aligns frames by timestamp and
  produces one combined PCM stream. Mixed output is what the STT
  consumes.
- **STT adapter trait (`SttAdapter`).** A small async trait with two
  methods: `start(audio_rx) -> events_rx` and `cancel()`. Two
  implementations:
  - `SonioxAdapter` — production. WebSocket streaming, sends `audio_pcm`
    frames, parses finalized + interim tokens.
  - `MockAdapter` — emits canned `TranscriptChunk`s on a configurable
    cadence; activated by `MEETING_COMPANION_STT_PROVIDER=mock`. No API
    key required.
- **Sentence flushing on the Soniox side.** A 3-second idle threshold
  combined with a "soft boundary" gate (the last finalized token must end
  in non-alphanumeric punctuation) decides when to promote a buffered
  span to an `Item`. This avoids splitting a sentence mid-word when the
  speaker pauses.
- **Per-response interim emission.** Soniox sends interim updates as the
  user speaks; each response with a non-empty interim block triggers a
  `TranscriptInterim` event that the PWA renders as the dim "live row"
  below the consolidated transcript items.
- **Cancellation via `CancellationToken`.** Audio task, mixer task, and
  STT adapter all run as siblings of the meeting lifetime; stop_meeting
  cancels the parent token, taking all three down cleanly.

## Consequences

**Positive:**

- One stream produces a transcript that contains both voices — the
  meeting is genuinely captured without external bridging.
- The mock adapter makes it possible to run end-to-end smoke tests in
  CI with no API keys, no audio hardware, and no macOS dependency.
- The STT adapter trait makes adding a Whisper or Deepgram backend a
  one-file change with no consumer-side awareness.
- Sentence flushing produces semantically meaningful transcript items —
  the same units the summarizers operate on and the mnemo pusher streams
  to memory.

**Negative:**

- macOS-only. Linux and Windows are not supported and require a
  different audio capture path (PulseAudio loopback, WASAPI loopback).
  Documented; not on the roadmap.
- ScreenCaptureKit requires user grant for screen recording permission,
  even though we only consume the audio sub-stream. The first run prompts
  for permission with a slightly misleading dialog.
- The `macos_15_0` feature gate must be enabled in `Cargo.toml`;
  forgetting it makes `with_captures_microphone(true)` a no-op at
  runtime.
- The 3-second idle threshold is a single global value. A user who
  speaks slowly may see longer-than-ideal sentence boundaries; one who
  speaks quickly may rarely hit the idle path.

**Accepted risks:**

- ScreenCaptureKit's mic-capture support is relatively new on macOS;
  Apple may shift behavior in a future OS version.
- Soniox occasional `Lagged(N)` warnings under sustained load; we log
  but don't degrade. Mitigated by a 64-frame broadcast channel.

## Alternatives considered

### (a, chosen) ScreenCaptureKit + in-process mixer + adapter trait

See above.

### (b) cpal microphone-only + hint to the user to "use a meeting headset"

Simpler, cross-platform. Rejected: defeats the meeting use case where
the _other_ speaker is the one we need to transcribe. A headset captures
neither the remote stream cleanly nor the room's other voices.

### (c) Virtual audio device (BlackHole)

User installs a kernel extension that aggregates system audio + mic into
a virtual input device; we read it via cpal. Rejected: requires the user
to install signed-but-non-Apple kext, configure routing in Audio MIDI
Setup, and re-route their audio at meeting time. Not a path the project
wants on its onboarding.

### (d) Cloud STT with file uploads at meeting end

Record locally, upload at stop. Rejected: kills the live transcript and
the rolling-summary flow; the project is built around real-time signal.

### (e) STT inside the PWA via Web Audio + Soniox JS SDK

Rejected for the meeting flow: only the laptop has access to system
audio; the phone PWA can't capture the remote speaker. (We do still
use Soniox in the PWA for the _meeting description_ dictation flow,
which is mic-only and runs in the user's browser.)
