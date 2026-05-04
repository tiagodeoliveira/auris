# Phase 2 Step 15 — Live Audio + STT + Parallel Mode Summarizers

> Status: spec, drafted 2026-05-03 from in-conversation brainstorm. Pending plan + implementation. Targets `main` after step 16 v3 (multi-provider LLM) is shipped.

## 0. Status & context

Step 16 (LLM metadata extraction) is **shipped** as of `7d83ac2` / `7bda2fb`. The server now extracts `{title, project, ...}` from the meeting description on `start_meeting` via rig + OpenAI/Anthropic/Bedrock. That covers the *header* of an active meeting.

Step 15 covers the *body* — what shows up in `Highlights`, `Transcript`, and `Actions` while the meeting is in progress. Phase 0's `mock.rs` (cycling templated strings every 3 s) was removed in `7a673ef` once it had clearly served its purpose. This spec replaces that hole with real, live, server-side production.

### 0.1 Architectural canon (locked)

These were settled during brainstorming and are not re-litigated by this spec:

- **macOS-only** via ScreenCaptureKit. The server captures *system audio + mic in a single PCM stream*. BlackHole / Loopback explicitly rejected — ScreenCaptureKit handles both sources natively without per-machine virtual-audio setup.
- **Soniox for STT, server-side, custom Rust client.** No official Rust SDK exists; we build a thin streaming-WS client (~200 LOC) on top of `tokio-tungstenite`.
- **All modes run in parallel.** During an active meeting, `transcript`, `highlights`, and `actions` summarizers all consume the same transcript stream concurrently. The PWA's `set_mode` is now a *display filter*, not a producer switch.
- **PWA = pure control plane + display.** No mic, no STT, no summarization. The exception is the existing PWA-side Soniox in `listening.ts` for the brief "Describe meeting" voice flow — that stays as-is.
- **Meetings are ephemeral.** No persistence. State lives in `ServerState` (in-memory). Stop or server-restart wipes it.
- **mnemo deferred** to step 18.

### 0.2 Step 15 is what makes Phase 2 feel real

Until this lands, the meeting body is empty. After this lands, you can run a Zoom call, click "Start meeting" in the PWA, and watch transcript / highlights / action items populate live. That is the visible product.

---

## 1. Purpose & scope

### 1.1 What this delivers

For the duration of an active meeting (`MeetingState::Active` only — paused suspends, stop tears down):

1. The server captures a continuous mixed audio stream (system + mic) from the laptop.
2. The server streams that audio to Soniox and receives finalized + interim transcript tokens.
3. The server runs three summarizer tasks **in parallel**, each consuming the rolling transcript:
   - `transcript`: passes finalized chunks through as `Item`s, no LLM.
   - `highlights`: every ~20 s, runs a rig `Extractor` to produce 3-5 key points; replaces the mode's list.
   - `actions`: every ~15 s, runs a rig `Extractor` to detect action items; appends new ones.
4. Every `Item` produced is broadcast to all WS clients via `Event::ItemsUpdate { mode, items }` (wire-contract change — see §5.1).
5. The PWA buffers items per mode locally; the active mode's buffer is what renders. `set_mode` switches which buffer is shown but does not throw away other modes' work.

### 1.2 Out of scope (this step)

- Linux / Windows builds. The server compiles on those platforms (CI keeps running) but `MEETING_COMPANION_AUDIO_DISABLED=1` is forced and a hard error is returned if anyone tries to start a meeting without it. ScreenCaptureKit is macOS-only.
- Speaker diarization beyond what Soniox returns natively in its token metadata.
- mnemo / cross-meeting memory (step 18).
- Persistence of any kind. Even crash-recovery is out of scope; meetings just die with the process.
- Bring-your-own-STT (Deepgram, Whisper, AssemblyAI). Soniox-only for now. The `stt` module is shaped so a second provider could be added later, but we don't ship one.
- Voice commands beyond the existing PWA "Describe meeting" flow.

### 1.3 Non-goals worth calling out

- **Not real-time**, in the millisecond sense. Soniox's interim tokens land within ~300 ms of utterance, but highlights/actions summarizers run on multi-second cadences. "Live" here means "you see content accumulating during the meeting," not "instantaneous reaction."
- **Not deduplicated across modes.** The same utterance can appear as a transcript line, a highlight bullet, and an action item simultaneously. That's intentional — different modes are different views of the same content.

---

## 2. Functional behavior

### 2.1 Meeting lifecycle

| Intent | Audio task | STT task | Summarizer tasks (×3) | rolling_transcript |
|---|---|---|---|---|
| `start_meeting` | spawn | spawn | spawn | clear, then accumulate |
| `pause` | suspend (drop frames) | hold connection | skip cycle | continue accumulating? **no** — pause means pause |
| `resume` | resume | reuse | resume | continue accumulating |
| `stop_meeting` | cancel | close WS | cancel | clear |
| WS client disconnect | unaffected | unaffected | unaffected | unaffected |
| `stop_meeting` while extraction in-flight | cancel cleanly | discard buffered audio | skip pending cycle | clear |

**Pause/resume semantics:** during pause, audio is *not captured* and the STT WS is held open but no PCM is written to it. On resume, capture resumes. There's a small gap in the transcript covering the paused interval; the summarizers don't try to interpolate.

### 2.2 Per-mode summarizer behavior

#### 2.2.1 `transcript` mode

- **LLM:** none. Pass-through.
- **Trigger:** on every Soniox token whose `is_final == true` and which forms the end of an utterance boundary (typically marked by `is_final` running together with terminal punctuation or a sufficient pause; we let Soniox's "finals" decide).
- **Output `Item` shape:**
  ```rust
  Item {
      id: Uuid::new_v4().to_string(),
      text: <transcript chunk text>,
      detail: None,
      t: <ms_offset_from_meeting_start>,
      meta: Some(json!({ "speaker": <option<string>> }))
  }
  ```
- **Strategy:** `UpdateStrategy::Append`. No item cap. (A 60-min meeting at conversational speed produces ~5-10k chars of transcript — well within memory budget.)
- **Interim tokens:** broadcast as `Event::TranscriptInterim { text }` for live display, but **not** stored as items. Optional MVP feature; can be cut for v0 if it complicates the wire contract.

#### 2.2.2 `highlights` mode

- **LLM:** yes — rig `Extractor<M, HighlightsExtraction>`.
- **Trigger:** time-based heartbeat. Default `MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS=20000`. Skip a cycle if `rolling_transcript` is empty or has not changed since the last cycle.
- **Schema:**
  ```rust
  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
  struct HighlightsExtraction {
      /// 3-5 key points from the meeting transcript so far. Most important first.
      items: Vec<Highlight>,
  }

  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
  struct Highlight {
      /// Concise statement of the key point. ≤ 120 chars.
      text: String,
      /// 1 = nice-to-know, 2 = important, 3 = decisive.
      importance: u8,
  }
  ```
- **Prompt input:** the full `rolling_transcript`, joined with `\n` and prefixed with the meeting `description` + extracted metadata for context.
- **Strategy:** `UpdateStrategy::Replace` — the entire highlights list is rewritten each cycle. Existing 10-item cap on `Replace` mode preserved.
- **Failure mode:** if extraction errors (timeout, rate limit, schema violation), log `warn`, skip the cycle, retry on next heartbeat. Do not crash the summarizer loop.

#### 2.2.3 `actions` mode

- **LLM:** yes — rig `Extractor<M, ActionsExtraction>`.
- **Trigger:** time-based heartbeat. Default `MEETING_COMPANION_ACTIONS_INTERVAL_MS=15000`.
- **Schema:**
  ```rust
  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
  struct ActionsExtraction {
      /// Action items detected since the start of the meeting. Empty if none.
      actions: Vec<ActionItem>,
  }

  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
  struct ActionItem {
      /// Imperative-mood action statement. ≤ 120 chars.
      action: String,
      /// Best guess at the owner if the transcript names one. Empty string if unclear.
      owner: String,
      /// Best guess at a due date if mentioned (e.g. "Friday", "next sprint"). Empty if unclear.
      due: String,
  }
  ```
- **Prompt input:** `rolling_transcript` + the *list of actions already extracted in this meeting*, so the LLM is told "don't repeat these."
- **Strategy:** `UpdateStrategy::Append`. Server dedupes against existing items by exact `action` string equality before pushing (prompt-level dedupe is unreliable; server-side belt-and-suspenders).
- **Failure mode:** same as highlights.

### 2.3 What happens when the LLM-disabled escape hatch is set

When `MEETING_COMPANION_LLM_DISABLED=1`:

- `transcript` mode: still runs (no LLM dependency). You get live transcription, no summarization.
- `highlights` mode: summarizer task spawns but every cycle no-ops with a `debug!` log.
- `actions` mode: same.

This is the offline-dev / test path.

---

## 3. Architecture

### 3.1 Server module layout

```
packages/server/src/
├── audio/
│   ├── mod.rs           NEW — public surface: spawn_audio_task, AudioFrame
│   ├── capture.rs       NEW — macOS ScreenCaptureKit impl (cfg-gated)
│   └── format.rs        NEW — PCM resample to 16 kHz mono S16LE
├── stt/
│   ├── mod.rs           NEW — public surface: spawn_stt_task, TranscriptChunk
│   ├── soniox.rs        NEW — Soniox WS client
│   └── mock.rs          NEW — MEETING_COMPANION_STT_MOCK=1 backend
├── summarizer/
│   ├── mod.rs           NEW — public surface: spawn_summarizer_tasks
│   ├── transcript.rs    NEW — pass-through, no LLM
│   ├── highlights.rs    NEW — rig Extractor, 20 s cadence
│   └── actions.rs       NEW — rig Extractor, 15 s cadence
├── state.rs             MODIFIED — +rolling_transcript +per-mode push/replace methods
├── ws.rs                MODIFIED — spawn audio + stt + summarizer trio on start_meeting
├── contract.rs          MODIFIED — Event::ItemsUpdate gains `mode` field; new TranscriptInterim event
├── llm.rs               UNCHANGED — already provides LlmClient with provider() accessor
├── extraction.rs        UNCHANGED — metadata-extraction wrapper
├── lib.rs               MODIFIED — register new modules
└── main.rs              UNCHANGED
```

### 3.2 Task topology when meeting is active

```
                   ┌──────────────────────────────────────┐
                   │  ScreenCaptureKit (macOS, native)    │
                   │  emits CMSampleBuffer (Float32 48k)  │
                   └────────────────┬─────────────────────┘
                                    │
                    ┌───────────────▼────────────────┐
                    │  audio task                    │
                    │  resample → 16k mono S16LE PCM │
                    │  mpsc bounded(100) ──▶         │
                    └───────────────┬────────────────┘
                                    │
                    ┌───────────────▼─────────────────┐
                    │  stt task (Soniox WS client)    │
                    │  send: PCM binary frames        │
                    │  recv: token JSON               │
                    │  emit: broadcast<TranscriptChunk>│
                    └───────────────┬─────────────────┘
                                    │  (broadcast, capacity 64)
                ┌───────────────────┼───────────────────┐
                │                   │                   │
       ┌────────▼─────────┐ ┌───────▼────────┐ ┌────────▼──────────┐
       │ transcript       │ │ highlights     │ │ actions           │
       │ summarizer       │ │ summarizer     │ │ summarizer        │
       │ on each chunk    │ │ 20 s heartbeat │ │ 15 s heartbeat    │
       │ no LLM           │ │ rig Extractor  │ │ rig Extractor     │
       └────────┬─────────┘ └───────┬────────┘ └────────┬──────────┘
                │                   │                   │
                └───────────────────┼───────────────────┘
                                    ▼
                        state.push_item_for_mode(mode, item)
                                    │
                                    ▼
                        events_tx.send(ItemsUpdate{mode, items})
                                    │
                                    ▼
                        broadcast to all WS clients
```

All five tasks share child tokens of `meeting_cancel`. `stop_meeting` cancels the root → cooperative shutdown of every task.

### 3.3 Crate dependencies (proposed additions)

| Crate | Purpose | Notes |
|---|---|---|
| `screencapturekit` | macOS audio capture | Latest 0.x. Cfg-gate behind `target_os = "macos"`. If the public crate is too thin, fall back to `objc2` + manual bindings; spec doesn't bless one over the other. |
| `dasp` (or hand-rolled) | PCM resample 48k → 16k | Optional. We can implement linear resample by hand in ~30 LOC if we don't want the dep. |
| `uuid` | Item IDs | Already in deps. |

**Existing deps used:** `tokio`, `tokio-tungstenite` (for Soniox WS), `tokio-util` (`CancellationToken`), `serde`, `serde_json`, `rig-core`, `schemars`, `tracing`, `thiserror`. No new framework adoption.

---

## 4. State changes

### 4.1 `ServerState` additions

```rust
pub struct ServerState {
    // ... existing fields ...
    pub(crate) rolling_transcript: Vec<TranscriptChunk>,
}
```

`rolling_transcript` is cleared on `start_meeting` and `stop_meeting`. It accumulates **finalized** transcript chunks only — interim chunks are not stored.

### 4.2 New `TranscriptChunk` type

Defined in `stt/mod.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptChunk {
    /// Stable chunk id (uuid v4).
    pub id: String,
    /// Finalized utterance text. Trimmed; non-empty.
    pub text: String,
    /// ms offset from meeting start at the start of this utterance.
    pub t_start_ms: u64,
    /// ms offset from meeting start at the end of this utterance.
    pub t_end_ms: u64,
    /// Optional speaker label from Soniox token metadata (often unavailable).
    pub speaker: Option<String>,
}
```

### 4.3 New methods on `ServerState`

```rust
impl ServerState {
    /// Append a transcript chunk to the rolling buffer. Returns the chunk back
    /// for downstream consumers that want it. No-op if meeting is not Active.
    pub fn append_transcript_chunk(&mut self, chunk: TranscriptChunk) -> Option<&TranscriptChunk>;

    /// Append an item to the named mode's list using its declared UpdateStrategy.
    /// Returns the broadcast payload (replaces full list for Replace, single-item Vec for Append).
    /// No-op (returns empty vec) if mode is not in `available_modes`.
    pub fn push_item_for_mode(&mut self, mode: &str, item: Item) -> Vec<Item>;

    /// Replace the entire item list for the named mode. Used by Replace-strategy
    /// summarizers that re-derive the whole list each cycle (highlights).
    /// Caps at 10 items to match existing Replace semantics.
    pub fn replace_items_for_mode(&mut self, mode: &str, items: Vec<Item>) -> Vec<Item>;

    /// Read-only access to rolling transcript joined as text, for prompt input.
    pub fn rolling_transcript_text(&self) -> String;
}
```

The previous `push_mock_item` (removed in `7a673ef`) is functionally replaced by `push_item_for_mode` parameterized by mode rather than always using `current_mode`. The 10-item Replace cap and Append-no-cap rules from `push_mock_item` carry over.

### 4.4 What `pause` does to state

`pause` flips `meeting_state` to `Paused` but **does not clear `rolling_transcript`** or the per-mode item lists. On resume, summarizers continue from whatever the buffer was. The audio/STT tasks suspend (don't write PCM during pause); summarizers' heartbeats keep ticking but skip cycles where `rolling_transcript` hasn't grown.

---

## 5. Wire contract changes

### 5.1 `Event::ItemsUpdate` gains `mode` field

**Before:**
```json
{ "type": "items_update", "items": [...] }
```

**After:**
```json
{ "type": "items_update", "mode": "highlights", "items": [...] }
```

This is a **breaking** wire-contract change. The PWA must update simultaneously to handle the new field. Justification: with three modes producing items in parallel, the PWA needs to know which mode an `items_update` belongs to so it can buffer all three and render the active one.

PWA-side handling:
- `AppState` gains `itemsByMode: Record<string, Item[]>` (or similar).
- `ws-handlers.ts` `case "items_update"`: write into `itemsByMode[event.mode]`.
- The mode buffer for the *current* mode is what renders. On `set_mode`, the renderer just reads from the new mode's buffer (already populated).
- On `Event::Snapshot` (which carries `items` for the current mode and `mode`): seed `itemsByMode[mode]` from the snapshot. Other modes' buffers stay empty until their first `items_update` lands.
- On `Event::ModeChanged { mode, items }`: also seed `itemsByMode[mode]` from the carried items.

### 5.2 New event: `Event::TranscriptInterim` (optional MVP)

```json
{ "type": "transcript_interim", "text": "the latest in-flight utterance..." }
```

Emitted whenever Soniox sends a non-final token batch. The PWA uses this to render a live "currently speaking…" indicator in transcript mode. Stateless on the server side — the chunk is broadcast and forgotten.

**MVP cut option:** if this complicates the implementation (it shouldn't — it's a one-line broadcast), defer to a follow-on. The architecture doesn't depend on it.

### 5.3 No new intents

The PWA stays a pure control plane. It does not push transcripts, audio, or any new payloads. Existing intents (`start_meeting`, `stop_meeting`, `pause`, `resume`, `set_mode`, `set_metadata`, `mark_moment`, `expand_item`) are unchanged.

### 5.4 `Event::Status` updates

The `error` field on the `status` event is the channel we already use to surface degraded states (used by the metadata-extraction failure path in `ws.rs`). Step 15 reuses it for:

- `"audio_permission_denied"` — TCC blocked audio capture.
- `"audio_init_failed"` — ScreenCaptureKit init failed for any other reason.
- `"stt_unavailable"` — Soniox WS unreachable after backoff.
- `"summarizer_failing"` — repeated LLM failures across N cycles.

Strings, not codes — the PWA renders them in a status banner. Resolution clears the field.

---

## 6. Audio source: ScreenCaptureKit

### 6.1 What's captured

ScreenCaptureKit captures both:
- **System audio** — anything any app on the laptop is playing (Zoom remote audio, Meet, Slack call audio, etc.).
- **Microphone** — the laptop's default input device.

These are mixed into a single PCM stream by the framework. We don't get separate channels for "remote" vs "local" — that's deliberate, since for transcription we want the full conversation as a single timeline.

### 6.2 Crate choice

**Primary:** `screencapturekit` crate (the most widely-used Rust binding). Verify during impl that it exposes `SCStreamConfiguration::captures_audio = true` and audio-only output (no display).

**Fallback:** if the crate is too thin or missing audio support, write manual bindings using `objc2` against `ScreenCaptureKit.framework`. The relevant Apple APIs are:

- `SCContentFilter` — what we capture (we want "all app audio + mic," typically modeled as a desktop-independent audio filter)
- `SCStreamConfiguration` with `captures_audio = YES`, `excludes_current_process_audio = YES` (don't recursively pick up our own logs played as "ding" sounds)
- `SCStream` — the live stream; we register an `SCStreamOutput` delegate that fires on each `CMSampleBuffer`
- Pull `CMSampleBufferGetDataBuffer` + `CMBlockBufferCopyDataBytes` to get raw bytes

**Decision deferred to impl:** spec leaves the crate-vs-manual choice open. Plan task should start with a 30-min spike on the public crate; if it doesn't expose what we need, drop to objc2.

### 6.3 Permission requirement (TCC)

macOS's Transparency, Consent, and Control framework gates Screen Recording permission. Implications:

- First time the server's audio module starts capturing, macOS pops a system dialog asking the user to grant Screen Recording permission to the **parent process**.
- For `cargo run` from a terminal, that parent is your terminal app (Terminal.app, iTerm2, Ghostty, etc.). The user clicks "Allow," then the spec recommends restarting the terminal.
- For a built `.app` bundle, the bundle itself gets the permission. (Out of scope for v0 — we're shipping `cargo run`.)
- If the user denies, audio init fails cleanly with `Event::Error { code: "audio_permission_denied" }` and the meeting continues silent. The user can revisit in System Settings → Privacy & Security → Screen Recording.

The `audio` module's init function returns a typed `Result<AudioCapture, AudioInitError>` so callers can distinguish denial from other failures.

### 6.4 Audio format conversion

| Stage | Format |
|---|---|
| ScreenCaptureKit native output | Float32, 48 kHz, stereo (per Apple's `kCMSampleAttachmentKey_*` defaults) |
| What we send to Soniox | S16LE, 16 kHz, mono |

Conversion happens entirely in `audio/format.rs` before audio frames hit the mpsc channel. Steps:
1. **Channel mix:** average left + right → mono.
2. **Resample:** 48 kHz → 16 kHz (3:1 decimation; can use a simple linear resampler or `dasp::interpolate`).
3. **Format convert:** Float32 → S16LE (multiply by `i16::MAX` and clamp).
4. **Frame:** group samples into ~20 ms frames (320 samples at 16 kHz).

A 20 ms frame is the conventional STT chunk size — small enough for low latency, large enough to amortize WS framing overhead.

### 6.5 No-audio fallback for non-macOS

`audio::capture::spawn_audio_task` is `cfg(target_os = "macos")`-gated. On other platforms, the function is a stub that immediately returns an error. The `ws.rs` start_meeting handler checks for this error at task-spawn time and:
- If `MEETING_COMPANION_AUDIO_DISABLED=1`: silently skips the audio task.
- Otherwise: emits `Event::Error { code: "audio_unsupported_platform" }` and the meeting runs without content. The PWA's metadata extraction still works.

Tests run fine on Linux because they always set `MEETING_COMPANION_AUDIO_DISABLED=1`.

---

## 7. STT: Soniox (server-side)

### 7.1 Protocol summary

- **Endpoint:** `wss://stt-rt.soniox.com/transcribe-websocket`
- **Auth:** API key passed in the *first* WS text message (config frame), not as a header.
- **First frame (text JSON):**
  ```json
  {
    "api_key": "<SONIOX_API_KEY>",
    "audio_format": "pcm_s16le",
    "sample_rate": 16000,
    "num_channels": 1,
    "model": "stt-rt-preview",
    "include_nonfinal": true,
    "enable_endpoint_detection": true
  }
  ```
- **Subsequent frames:** binary, raw S16LE PCM.
- **Server responses (text JSON):**
  ```json
  {
    "tokens": [
      { "text": "Hello", "is_final": true,  "start_ms": 1240, "end_ms": 1480, "speaker": null },
      { "text": " world", "is_final": false, "start_ms": 1480, "end_ms": 1620, "speaker": null }
    ]
  }
  ```
- **Closing the stream:** send a binary frame containing zero-length payload to signal end-of-audio, then close WS.

(Verify exact field shapes against current Soniox docs during impl. The structure above is representative; it may have a `result` envelope or different field names. Spec is conceptual.)

### 7.2 Custom Rust client

`stt/soniox.rs` implements:

```rust
pub struct SonioxClient {
    api_key: String,
    sample_rate: u32,
}

impl SonioxClient {
    pub fn new(api_key: String) -> Self;

    /// Open a streaming session. Returns a paired sink (write PCM here)
    /// and stream (read TranscriptChunks + interim updates from here).
    pub async fn open_session(
        &self,
    ) -> Result<(SonioxSink, SonioxStream), SonioxError>;
}

pub struct SonioxSink {
    /// Write PCM frames to be transcribed. Bounded mpsc internally.
    pub fn write(&self, pcm: Vec<u8>) -> Result<(), SonioxError>;

    /// Signal end of audio. Closes the WS cleanly.
    pub async fn close(self) -> Result<(), SonioxError>;
}

pub enum SonioxStreamEvent {
    Final(TranscriptChunk),
    Interim(String),
    Error(SonioxError),
}

pub type SonioxStream = impl Stream<Item = SonioxStreamEvent>;
```

Implementation budget: ~200 LOC including the connect/handshake/parse paths. Tests use a `tokio-tungstenite` mock server.

### 7.3 Reconnection strategy

Soniox WS can drop for transient reasons (network blips, 1-hour session limit). Strategy:

- Exponential backoff: 500 ms, 1 s, 2 s, 4 s, … capped at 30 s.
- Indefinite retries while the meeting is `Active` or `Paused`. Stop retrying on `stop_meeting`.
- Each reconnect starts a fresh session — Soniox doesn't support session resumption. Transcript gap during reconnect is acceptable; we surface `Event::Error { code: "stt_unavailable" }` once we've been failing for >10 s.

### 7.4 Mock STT for offline dev

`stt/mock.rs` (`MEETING_COMPANION_STT_MOCK=1`):

- Ignores PCM input.
- Emits canned `TranscriptChunk`s every `MEETING_COMPANION_STT_MOCK_INTERVAL_MS` (default 3000) cycling through a fixed list of utterances drawn from a "fake meeting" corpus.
- Does not require Soniox API key.
- Used by integration tests and by the developer when iterating on summarizer prompts without burning Soniox credits.

---

## 8. Per-mode summarizers (detail)

### 8.1 Shared lifecycle

Each summarizer is its own `tokio::spawn`'d task. All three are started together by `summarizer::spawn_summarizer_tasks(handle, transcript_rx, cancel)` on `start_meeting`.

Each task receives:
- `handle: ServerHandle` — for `state.lock()` and `events_tx.send()`.
- `transcript_rx: broadcast::Receiver<TranscriptChunk>` — fresh subscriber per task.
- `cancel: CancellationToken` — child of `meeting_cancel`.

### 8.2 `transcript` summarizer (`summarizer/transcript.rs`)

```rust
async fn run_transcript_summarizer(
    handle: ServerHandle,
    mut rx: broadcast::Receiver<TranscriptChunk>,
    cancel: CancellationToken,
) {
    let started = Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            chunk = rx.recv() => match chunk {
                Ok(c) => {
                    let item = Item {
                        id: c.id.clone(),
                        text: c.text.clone(),
                        detail: None,
                        t: c.t_start_ms,
                        meta: c.speaker.as_ref().map(|s| json!({ "speaker": s })),
                    };
                    let payload = {
                        let mut s = handle.state.lock().await;
                        s.push_item_for_mode("transcript", item)
                    };
                    let _ = handle.events_tx.send(Event::ItemsUpdate {
                        mode: "transcript".into(),
                        items: payload,
                    });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}
```

No LLM. ~30 LOC. The simplest summarizer.

### 8.3 `highlights` summarizer (`summarizer/highlights.rs`)

```rust
const SYSTEM_PROMPT: &str = "You are a meeting highlights extractor. \
Given the rolling transcript of a meeting in progress, return the 3-5 most important \
points so far. Order by importance, most decisive first. Use the speaker's wording where \
possible. Each point ≤ 120 characters.";

async fn run_highlights_summarizer(
    handle: ServerHandle,
    cancel: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() { continue; }
                let transcript = { handle.state.lock().await.rolling_transcript_text() };
                if transcript.is_empty() { continue; }
                match extract_highlights(&handle.llm, &transcript).await {
                    Ok(highlights) => {
                        let items = highlights.into_iter().enumerate().map(|(i, h)| Item {
                            id: format!("h-{}", i),  // stable index id; replace strategy
                            text: h.text,
                            detail: None,
                            t: 0,
                            meta: Some(json!({ "importance": h.importance })),
                        }).collect();
                        let payload = {
                            let mut s = handle.state.lock().await;
                            s.replace_items_for_mode("highlights", items)
                        };
                        let _ = handle.events_tx.send(Event::ItemsUpdate {
                            mode: "highlights".into(),
                            items: payload,
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "highlights extraction failed; skipping cycle"),
                }
            }
        }
    }
}

// Internal: builds the prompt + calls rig
async fn extract_highlights(
    llm: &LlmClient,
    transcript: &str,
) -> Result<Vec<Highlight>, ExtractionError> { /* ... */ }
```

The `LlmClient` API (already shipped in step 16) exposes `extract<T>` for a single typed extraction; we'll either reuse it with a different schema or add a sibling method `extract_with_prompt::<T>(&self, system_prompt: &str, user_prompt: &str)`. The latter is cleaner — leave `extract` for the metadata path and add `extract_with_prompt` for summarizers. Spec leaves the choice to the plan.

### 8.4 `actions` summarizer (`summarizer/actions.rs`)

```rust
const SYSTEM_PROMPT: &str = "You are a meeting action-item detector. \
Given the rolling transcript and the action items already detected, return only NEW \
action items. Each must be an imperative-mood statement. Do not repeat existing items \
even if rephrased. Use empty string for owner/due if not stated explicitly. Each ≤ 120 chars.";

async fn run_actions_summarizer(
    handle: ServerHandle,
    cancel: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() { continue; }
                let (transcript, existing) = {
                    let s = handle.state.lock().await;
                    (s.rolling_transcript_text(), s.items_for_mode("actions").clone())
                };
                if transcript.is_empty() { continue; }
                match extract_actions(&handle.llm, &transcript, &existing).await {
                    Ok(new_actions) => {
                        let mut payload = Vec::new();
                        let mut s = handle.state.lock().await;
                        for action in new_actions {
                            // Server-side dedupe: exact action text match
                            if existing.iter().any(|i| i.text == action.action) { continue; }
                            let item = Item {
                                id: format!("a-{}", uuid::Uuid::new_v4()),
                                text: action.action,
                                detail: None,
                                t: 0,
                                meta: Some(json!({
                                    "owner": action.owner,
                                    "due": action.due,
                                })),
                            };
                            payload.extend(s.push_item_for_mode("actions", item));
                        }
                        drop(s);
                        if !payload.is_empty() {
                            let _ = handle.events_tx.send(Event::ItemsUpdate {
                                mode: "actions".into(),
                                items: payload,
                            });
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "actions extraction failed; skipping cycle"),
                }
            }
        }
    }
}
```

---

## 9. Concurrency & lifecycle

### 9.1 Cancellation tree

```
meeting_cancel (root, owned by handle.meeting_cancel slot)
├── audio task (child token)
├── stt task (child token)
├── transcript summarizer (child token)
├── highlights summarizer (child token)
└── actions summarizer (child token)
```

`stop_meeting` → take + cancel root → all five children observe `cancel.cancelled()` and exit cleanly.

### 9.2 Pause/resume

`pause` sets `meeting_state = Paused`. The audio task observes this via state lock and stops writing PCM (effectively a soft mute). The STT WS connection stays open. Summarizers' heartbeat tickers keep firing but their idempotency check (skip cycle if transcript hasn't grown) means they no-op.

`resume` flips back to `Active`. Audio task resumes capture. Summarizers continue.

The simpler alternative is to fully tear down on pause and respawn on resume. **Spec chooses the lighter-weight approach** (keep tasks alive) for two reasons: (1) Soniox WS reconnect costs latency; (2) it keeps task lifecycle simpler — only `start_meeting` and `stop_meeting` mutate the task graph.

### 9.3 Backpressure

| Channel | Type | Capacity | Slow-consumer policy |
|---|---|---|---|
| audio → stt | `mpsc::channel<Vec<u8>>` | 100 frames (~2 s at 50 fps) | Drop oldest, log warn. (Audio loss < transcript loss.) |
| stt → summarizers | `broadcast::channel<TranscriptChunk>` | 64 chunks (~10 min of speech) | Lagged receiver gets `RecvError::Lagged`; log warn, continue. |
| events_tx (existing) | `broadcast::channel<Event>` | 64 (already configured) | Existing — disconnects the lagging WS client. |

If we're losing audio frames consistently, that's a real problem and surfaces as `Event::Error { code: "audio_backpressure" }`. Not expected under normal load (Soniox WS handles 16 kHz mono PCM trivially).

### 9.4 Server boot sequence

`main.rs` boot order, updated:

1. tracing init, dotenvy
2. clap parse args
3. `MEETING_COMPANION_TOKEN` validation
4. `LlmClient::from_env()` — fails fast if LLM unavailable (existing)
5. **NEW:** Soniox API key check — if `MEETING_COMPANION_STT_PROVIDER` is `soniox` (default) and not mocked, require `SONIOX_API_KEY`. Fail boot with exit code `4` if missing.
6. **NEW:** TCC permission check — non-blocking. Log `info` if Screen Recording permission is granted, `warn` if not (audio will fail later, but server still boots).
7. `run_server` (existing)

### 9.5 The current `meeting_cancel` slot stays

`ws.rs` already has `handle.meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>`. This spec extends what's spawned under it but doesn't change the slot's semantics. Step 16's metadata extraction continues using a child token of this slot, alongside step 15's five new children.

---

## 10. Error handling

### 10.1 Audio errors

| Condition | Server action | User-visible state |
|---|---|---|
| TCC permission denied | `Event::Error { code: "audio_permission_denied" }` | "Audio capture permission required" banner in PWA. Meeting active but silent. |
| ScreenCaptureKit init fails | `Event::Error { code: "audio_init_failed" }` | Generic "Audio init failed" banner. |
| Mic / system audio device unavailable mid-meeting | log warn, retry every 5 s | Banner if down >10 s. |
| Backpressure: `mpsc::send` returns `Full` | drop frame, log warn | None unless sustained. |

### 10.2 STT errors

| Condition | Server action | User-visible state |
|---|---|---|
| Soniox WS connect fails | exponential backoff retry (§7.3) | None unless >10 s; then `stt_unavailable` banner. |
| Soniox returns auth error | log error, do not retry | `stt_auth_failed` banner. Recoverable only by stopping the meeting and fixing `SONIOX_API_KEY`. |
| Soniox WS disconnects mid-stream | reconnect | Banner if reconnect >10 s. |
| Token parse error (unexpected JSON) | log warn, drop chunk, continue | None. |

### 10.3 Summarizer errors

| Condition | Server action | User-visible state |
|---|---|---|
| LLM call fails (timeout, schema, rate limit) | log warn, skip cycle, retry next heartbeat | None unless N consecutive failures (then `summarizer_failing` banner). |
| LLM disabled by env var | continue silently | None. Highlights and actions just stay empty. |
| `MEETING_COMPANION_LLM_DISABLED=1` set after spawn | observed at next cycle | Same as above. |

### 10.4 Server-side error budget

If everything fails simultaneously (no audio + no STT + no LLM), the meeting still has `transcript` mode (empty), the metadata extraction may have run on the description, and the WS connection is alive. The PWA UI doesn't crash; it just shows empty content + an error banner explaining what's degraded.

---

## 11. Configuration / environment variables

### 11.1 New variables

| Env var | Required | Default | Purpose |
|---|---|---|---|
| `SONIOX_API_KEY` | when `STT_PROVIDER=soniox` | — | Soniox auth key (read by `stt::soniox`). |
| `MEETING_COMPANION_STT_PROVIDER` | no | `soniox` | One of `soniox`, `mock`. |
| `MEETING_COMPANION_AUDIO_DISABLED` | no | unset | When set, skips audio capture entirely. |
| `MEETING_COMPANION_STT_MOCK` | no | unset | Convenience: equivalent to `STT_PROVIDER=mock`. |
| `MEETING_COMPANION_STT_MOCK_INTERVAL_MS` | no | `3000` | How often the mock STT emits canned chunks. |
| `MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS` | no | `20000` | Highlights heartbeat. |
| `MEETING_COMPANION_ACTIONS_INTERVAL_MS` | no | `15000` | Actions heartbeat. |

### 11.2 Existing variables (no change)

`MEETING_COMPANION_TOKEN`, `MEETING_COMPANION_HEARTBEAT_MS`, `MEETING_COMPANION_LLM_*`, `RUST_LOG`, `--port`, `--bind` — all unchanged.

`MEETING_COMPANION_LLM_DISABLED=1` now also disables highlights + actions summarizer LLM calls (transcript still runs).

### 11.3 `.env.example` additions

```
# ─── STT (when running with audio capture) ───────────────────────────────────
SONIOX_API_KEY=replace-me

# Override the STT provider. Defaults to `soniox`. Use `mock` for offline dev.
# MEETING_COMPANION_STT_PROVIDER=soniox

# Skip audio capture entirely (development on Linux, or if you want a meeting
# without audio for any reason).
# MEETING_COMPANION_AUDIO_DISABLED=1

# Mock STT cadence (only meaningful with STT_PROVIDER=mock).
# MEETING_COMPANION_STT_MOCK_INTERVAL_MS=3000

# Per-mode summarizer cadences.
# MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS=20000
# MEETING_COMPANION_ACTIONS_INTERVAL_MS=15000
```

---

## 12. Test plan

### 12.1 Unit tests

| Module | Test focus | Approach |
|---|---|---|
| `audio/format.rs` | Float32 → S16LE mono 16k correctness | Drive synthetic sine wave through the resampler; assert frame size + rough RMS preservation. Pure CPU, no SCKit. |
| `stt/soniox.rs` | WS handshake + JSON parse + reconnect | Mock WS server using `tokio-tungstenite::accept_async`. Cover: handshake, valid token JSON, malformed JSON, mid-stream disconnect, auth rejection. |
| `stt/mock.rs` | Cadence + cycling | Drive with a virtual clock (`tokio::time::pause`), assert chunks land at the configured interval. |
| `summarizer/transcript.rs` | Pass-through correctness | Feed canned `TranscriptChunk`s, assert the right `Item`s land in `state.items_per_mode["transcript"]`. No LLM. |
| `summarizer/highlights.rs` | Cadence + LLM call shape + Replace strategy | Mock `LlmClient::extract_with_prompt` to return fixed `HighlightsExtraction`. Drive the heartbeat with virtual clock. Assert items replace cleanly each cycle. |
| `summarizer/actions.rs` | Cadence + dedupe | Same pattern as highlights, with multiple cycles asserting that previously-seen actions don't repeat. |
| `state.rs` | New `push_item_for_mode`, `replace_items_for_mode`, `append_transcript_chunk`, `rolling_transcript_text` | Pure unit tests, no async. Cover the Append-vs-Replace branches and the 10-cap on Replace. |
| `contract.rs` | `Event::ItemsUpdate { mode, items }` round-trip | Existing `assert_round_trip` pattern; ensure the new field serializes/deserializes correctly. |

### 12.2 Integration tests

| Test | Setup | Assertion |
|---|---|---|
| `live_pipeline_smoke` | `MEETING_COMPANION_STT_MOCK=1`, `MEETING_COMPANION_LLM_DISABLED=1`, all-modes-running | Send `start_meeting` → wait 5 s → assert `transcript` mode has ≥ 1 item. |
| `live_pipeline_modes_emit_in_parallel` | Mock STT + mock LLM (returning fixed schemas) | Assert `items_update` events for all three modes arrive within 30 s. |
| `live_pipeline_clean_shutdown` | Same as smoke + send `stop_meeting` | Assert all five tasks exit; no leaked tokio tasks (`tokio::runtime::Handle::current().metrics()` if available). |
| `pause_resume_continuity` | Mock STT | `start` → 2 s → `pause` → wait 2 s → `resume` → 2 s → `stop`. Assert the transcript has chunks from both active windows but none from the paused window. |

Mock LLM = a `LlmClient` constructed with `Provider::Mock` (new variant gated behind `cfg(test)` or `MEETING_COMPANION_LLM_PROVIDER=mock`). Returns canned schemas.

### 12.3 Manual smoke

`Justfile` recipe:

```just
# Live audio + STT smoke. Requires SONIOX_API_KEY + macOS Screen Recording permission.
live-smoke:
    cargo run -p meeting-companion-server -- --port 7331
    # ... in another terminal:
    # just pwa-sim, then click Start meeting, talk for 30 s, observe transcript flow
```

No fully-automated end-to-end test — that requires a real audio source. The smoke test proves the pipeline locally; CI runs only the unit + integration tests.

### 12.4 Test count delta

- Step 16 baseline: 81 tests (after `7a673ef`).
- Step 15 adds: ~25 unit tests + 4 integration tests = ~29 tests.
- Target: ~110 server tests after step 15 lands.

PWA tests: +3 to handle the new `itemsByMode` state and `set_mode` semantics. ~67 PWA tests after.

---

## 13. PWA-side changes

### 13.1 New state shape

```ts
// types.ts
interface AppState {
  // ... existing ...
  itemsByMode: Record<string, Item[]>;   // NEW; replaces single `items`
  // legacy `items` removed; renderers read from itemsByMode[currentMode]
  liveTranscriptInterim: string;          // NEW (optional, for §5.2)
}
```

### 13.2 `ws-handlers.ts` updates

```ts
case "items_update":
  store.update(s => ({
    itemsByMode: { ...s.itemsByMode, [event.mode]: event.items }
  }));
  break;

case "transcript_interim":  // optional
  store.update({ liveTranscriptInterim: event.text });
  break;

case "snapshot":
  // Seed itemsByMode[event.mode] from the snapshot's items
  // ... (existing reconciliation logic, updated)
  break;

case "mode_changed":
  // Update current mode + seed itemsByMode[event.mode] from carried items
  // ... (existing logic, updated)
  break;
```

### 13.3 Renderer changes

The active-list and active-detail glasses layouts (and the phone-side equivalent) read from `s.itemsByMode[s.currentMode]` instead of `s.items`. Pure plumbing; the rendering logic itself doesn't change.

### 13.4 Interim transcript banner (optional MVP)

When `s.currentMode === "transcript"` and `s.liveTranscriptInterim` is non-empty, show the interim text in dim color below the finalized list. Easy add; can be deferred to a follow-up.

---

## 14. Open questions

1. **screencapturekit-rs crate audit.** Does the public crate expose audio-only capture cleanly, or do we drop to objc2? **Resolution path:** 30-min spike at start of plan task 1.

2. **Soniox token model name.** `stt-rt-preview` vs other; spec assumes `stt-rt-preview`. **Resolution path:** verify against Soniox docs at impl time; fall back to whatever model the docs recommend.

3. **rolling_transcript memory cap.** A 4-hour meeting at 150 wpm produces ~36k words, ~250 KB of text. Each summarizer call sends this to the LLM. At ~1 token/byte that's ~250k tokens — over context windows. **Resolution path:** for v0, cap `rolling_transcript_text()` at the *last 30 minutes* (sliding window) when constructing summarizer prompts. The full transcript stays in `rolling_transcript` and feeds the `transcript` mode unchanged.

4. **Should `Item.id` for highlights be stable across cycles?** With Replace strategy, the PWA's renderer can either show smooth diffs (if ids are stable) or full re-renders (if ids regenerate). **Resolution path:** generate ids by `format!("h-{}", i)` where `i` is the rank index. Stable for "still in top 5" highlights; new id when an item exits the list. Simple, deterministic, good enough.

5. **TranscriptInterim cut?** §5.2 marks it MVP-optional. **Resolution path:** include in v0; cost is negligible (one extra event variant).

---

## 15. Self-review

| Section | Covered |
|---|---|
| Purpose & scope | §1 |
| Functional behavior (lifecycle + per-mode) | §2 |
| Architecture (modules, topology, deps) | §3 |
| State changes (rolling_transcript, new methods) | §4 |
| Wire contract changes (Event::ItemsUpdate {mode, items}, optional TranscriptInterim) | §5 |
| Audio source (ScreenCaptureKit, TCC, format) | §6 |
| STT (Soniox protocol, custom client, reconnect, mock) | §7 |
| Per-mode summarizers (transcript, highlights, actions) | §8 |
| Concurrency & lifecycle (cancellation tree, pause, backpressure) | §9 |
| Error handling | §10 |
| Configuration | §11 |
| Test plan | §12 |
| PWA-side changes | §13 |
| Out of scope | §1.2 |
| Open questions | §14 |

**Type consistency:** `TranscriptChunk`, `Item`, `LlmClient`, `Event::ItemsUpdate { mode, items }`, `HighlightsExtraction`, `ActionsExtraction`, `SonioxClient`, `AudioFrame`, `MissedTickBehavior` — each defined exactly once and referenced consistently.

**Placeholder scan:** No `TODO`, `TBD`, `fill in details`. Open questions §14 are explicit known-unknowns with named resolution paths, not placeholders.

**Spec ↔ canon alignment:** Every architectural canon point in §0.1 is reflected in the design (macOS-only → §1.2 + §6.5; Soniox custom client → §7.2; parallel modes → §2.2 + §3.2 + §5.1; ephemeral → §1.1 + §4.1; mnemo deferred → §1.2; PWA control plane → §5.3 + §13).

---

## 16. Implementation notes (post-shipping reconciliation)

The spec was drafted before any code was written; this section records
what actually shipped vs what was originally planned, and the gotchas
discovered during execution.

### 16.1 Deviations from the spec

| Spec section | Spec said | Actually shipped | Why |
|---|---|---|---|
| §6.1 Audio source | "system audio + mic mixed by SCKit" | mic only (system audio captured but discarded) | SCKit delivers Audio + Mic as two separate ~50fps output types. Naive concatenation = 2x-time playback. Proper mixer is a deferred follow-up. |
| §7.1 Soniox protocol | endpoint detection / start_ms+end_ms | per-token sub-word fragments; we buffer to sentence boundaries | `stt-rt-preview` emits sub-word tokens; we buffer finalized tokens until terminator punctuation or 1s idle. Per-token timestamps lost in the process; chunk t_start/t_end come from session-elapsed time. |
| §5.2 TranscriptInterim | optional MVP; emit interim text | wire shape exists, but provider doesn't emit | Plumbing through the SttProvider trait would require a second sender. Deferred. |

### 16.2 macOS-toolchain gotchas (committed fixes)

- **Workspace `.cargo/config.toml`** with rpath at `/usr/lib/swift` —
  required because the screencapturekit crate's Swift bridge links
  against libswift_Concurrency.dylib, expected at @rpath. The
  /usr/lib/swift path resolves through the dyld shared cache.
- **screencapturekit `macos_15_0` Cargo feature** — without it, the
  Swift bridge's `set_captures_microphone` is a compile-time no-op.
- **tokio-tungstenite `native-tls` feature** — required for wss://
  endpoints (Soniox).

### 16.3 TCC permissions (manual user action)

Two macOS TCC permissions must be granted to the parent terminal:
1. Screen Recording (for SCKit init)
2. Microphone (for SCKit mic capture, macOS 15+)

After granting, the user must restart the terminal. macOS doesn't
update permissions for already-running processes.

### 16.4 Test count

Pre-step-15 baseline: 81 server tests. After step 15: **107 server
tests** (+26). Breakdown:
- Task 1 (wire bump): +1 (TranscriptInterim round-trip)
- Task 2 (state extensions): +8
- Task 3 (mock STT): +1
- Task 4 (transcript summarizer): +1
- Task 5 (highlights summarizer): +3
- Task 6 (actions summarizer): +3
- Task 7 (lifecycle smoke): +1
- Task 9 (audio format conversion): +4
- Task 10 (Soniox client): +2
- Token buffering follow-up: +2

PWA tests: 64 → 64 (preserved; wire-shape changes were absorbed by
fixture updates without adding tests).

### 16.5 Follow-up enhancements (out of v0 scope)

1. **Audio mixer** — sample-buffer summer that combines system audio +
   mic at fixed 50fps. Lifts the headphone-user limitation. ~80 LOC.
2. **Live interim transcripts** — plumb `Event::TranscriptInterim`
   through the SttProvider trait; emit per Soniox response.
3. **Per-token timestamps** — track buffer's first-token start_ms +
   last-token end_ms instead of session-elapsed wall-clock.
4. **Soniox model selection** — allow `MEETING_COMPANION_SONIOX_MODEL`
   override; some models tokenize at word/sentence granularity natively.
5. **Reconnect telemetry** — `Event::Status` with current Soniox
   reconnect attempts, surfaced as a banner in the PWA.
