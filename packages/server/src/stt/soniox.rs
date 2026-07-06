//! Soniox streaming STT client.
//! Wire-format reference: `packages/pwa/src/stt/soniox.ts` (TypeScript impl,
//! confirmed working with the user's Soniox account).

use crate::observability::SttMetrics;
use crate::stt::{SttInitError, SttProvider, TranscriptChunk};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const SONIOX_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
// `stt-rt-v4` is Soniox's current real-time model. We previously used
// `stt-rt-preview` which appears to be deprecated — sessions died after
// 10-20 s and transcripts came back as Finnish/Spanish gibberish once
// `enable_speaker_diarization` was set. Override via
// `AURIS_SONIOX_MODEL` if a newer model ships.
const MODEL_DEFAULT: &str = "stt-rt-v4";
const SAMPLE_RATE: u32 = 16000;

/// Initial backoff between reconnect attempts. Doubles on each
/// consecutive failure up to `reconnect_max()`. Override via
/// `AURIS_SONIOX_RECONNECT_BASE_MS` (default 500).
fn reconnect_base() -> Duration {
    Duration::from_millis(crate::config::var_u64_or(
        "AURIS_SONIOX_RECONNECT_BASE_MS",
        500,
    ))
}

/// Ceiling on the exponential reconnect backoff. Override via
/// `AURIS_SONIOX_RECONNECT_MAX_S` (default 30).
fn reconnect_max() -> Duration {
    Duration::from_secs(crate::config::var_u64_or(
        "AURIS_SONIOX_RECONNECT_MAX_S",
        30,
    ))
}

/// Idle threshold: when no new tokens have arrived for this long
/// during an active session, flush the buffer as an end-of-utterance.
/// Sub-3s gaps are mid-sentence; longer pauses are real conversational
/// boundaries. Override via `AURIS_SONIOX_IDLE_FLUSH_MS` (default 3000).
fn idle_flush_ms() -> u64 {
    crate::config::var_u64_or("AURIS_SONIOX_IDLE_FLUSH_MS", 3000)
}

/// Max time to keep reading the Soniox socket after we send the empty
/// end-of-audio frame, before force-closing on whatever we have. The
/// common case finishes in well under a second (Soniox sends
/// `finished:true` then closes); this is the safety bound for a stalled
/// provider. Override via `AURIS_SONIOX_DRAIN_MS` (default 5000).
fn drain_grace_ms() -> u64 {
    crate::config::var_u64_or("AURIS_SONIOX_DRAIN_MS", 5000)
}

/// A *failed* session still counts as "healthy" — resetting the
/// reconnect escalation — if it stayed up at least this long before
/// dying, even when it never produced a final token (muted mic /
/// silent room holding a connection open). Token receipt is the
/// primary health signal; this is the duration fallback. Override via
/// `AURIS_SONIOX_HEALTHY_SESSION_S` (default 60).
fn healthy_session_threshold() -> Duration {
    Duration::from_secs(crate::config::var_u64_or(
        "AURIS_SONIOX_HEALTHY_SESSION_S",
        60,
    ))
}

/// Pure reconnect escalation state for the Soniox session loop —
/// extracted from `run_soniox` so the escalate/backoff/reset arithmetic
/// is unit-testable without sockets or timing.
///
/// `consecutive_failures` means what it says: failures since the last
/// *healthy* session (one that produced a final token or stayed up past
/// `healthy_session_threshold()`). Before this extraction the counter
/// and backoff were monotone per meeting — a blip two hours in waited
/// up to `reconnect_max()` (30 s of lost transcript) and false-alarmed
/// `stt_unavailable` after 5 *lifetime* blips.
struct ReconnectState {
    consecutive_failures: u32,
    backoff: Duration,
    base: Duration,
    max: Duration,
}

impl ReconnectState {
    fn new(base: Duration, max: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            backoff: base,
            base,
            max,
        }
    }

    /// Record one failed session attempt. Returns
    /// `(sleep_before_retry, status_code)`.
    ///
    /// `healthy` = the session that just failed had *proven itself*
    /// (received at least one final token, or stayed up >= the healthy
    /// threshold) — in that case escalation restarts from scratch before
    /// counting this failure. Connect/auth/config failures pass `false`
    /// so a revoked API key still escalates to `stt_unavailable`.
    fn on_failure(&mut self, healthy: bool) -> (Duration, &'static str) {
        if healthy {
            self.consecutive_failures = 0;
            self.backoff = self.base;
        }
        self.consecutive_failures += 1;
        let code = if self.consecutive_failures >= 5 {
            "stt_unavailable"
        } else {
            "stt_reconnecting"
        };
        let sleep = self.backoff;
        self.backoff = (self.backoff * 2).min(self.max);
        (sleep, code)
    }
}

pub struct SonioxStt {
    api_key: String,
    url: String,
    model: String,
}

impl SonioxStt {
    pub fn from_env() -> Result<Self, SttInitError> {
        let api_key = std::env::var("SONIOX_API_KEY").map_err(|_| {
            SttInitError::MissingCredentials(
                "SONIOX_API_KEY is required when AURIS_STT_PROVIDER=soniox".to_string(),
            )
        })?;
        if api_key.is_empty() {
            return Err(SttInitError::MissingCredentials(
                "SONIOX_API_KEY is empty".to_string(),
            ));
        }
        let model = crate::config::var_or("AURIS_SONIOX_MODEL", MODEL_DEFAULT);
        Ok(Self {
            api_key,
            url: SONIOX_URL.to_string(),
            model,
        })
    }

    /// Test-only constructor that lets tests override the URL.
    #[cfg(test)]
    fn new_with_url(api_key: String, url: String) -> Self {
        Self {
            api_key,
            url,
            model: MODEL_DEFAULT.to_string(),
        }
    }
}

#[derive(Serialize)]
struct ConfigFrame<'a> {
    api_key: &'a str,
    audio_format: &'a str,
    sample_rate: u32,
    num_channels: u32,
    model: &'a str,
    enable_speaker_diarization: bool,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    tokens: Vec<Token>,
    #[serde(default)]
    error_code: Option<i32>,
    #[serde(default)]
    error: Option<ErrorBlock>,
    /// Soniox sets this `true` in its terminal message (empty `tokens`)
    /// after we send the empty end-of-audio frame. Signals the stream
    /// is fully drained and the WS is about to close.
    #[serde(default)]
    finished: bool,
}

#[derive(Deserialize)]
struct Token {
    #[serde(default)]
    text: String,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    start_ms: u64,
    #[serde(default)]
    end_ms: u64,
    #[serde(default)]
    speaker: Option<String>,
}

#[derive(Deserialize)]
struct ErrorBlock {
    #[serde(default)]
    code: Option<i32>,
}

impl SttProvider for SonioxStt {
    fn name(&self) -> &'static str {
        "soniox"
    }

    fn run(
        self: Box<Self>,
        audio_rx: Option<mpsc::Receiver<Vec<u8>>>,
        transcript_tx: broadcast::Sender<TranscriptChunk>,
        events_tx: broadcast::Sender<crate::protocol::UserEvent>,
        user_id: String,
        cancel: CancellationToken,
        drain: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(run_soniox(
            *self,
            audio_rx,
            transcript_tx,
            events_tx,
            user_id,
            cancel,
            drain,
        ))
    }
}

/// Main run loop. Reconnects with exponential backoff if the WS drops.
/// Stops cleanly on cancel.
async fn run_soniox(
    cfg: SonioxStt,
    mut audio_rx: Option<mpsc::Receiver<Vec<u8>>>,
    transcript_tx: broadcast::Sender<TranscriptChunk>,
    events_tx: broadcast::Sender<crate::protocol::UserEvent>,
    user_id: String,
    cancel: CancellationToken,
    drain: CancellationToken,
) {
    if audio_rx.is_none() {
        warn!("Soniox provider has no audio source; transcription will produce nothing");
    }

    let stt_metrics = SttMetrics::new();
    let mut reconnect = ReconnectState::new(reconnect_base(), reconnect_max());
    let healthy_threshold = healthy_session_threshold();
    // Meeting-relative epoch for ALL sessions (created once, before the
    // reconnect loop). Soniox token timestamps are relative to the audio
    // stream of the *current* WS session, so a reconnected session's
    // clock restarts at 0 mid-meeting; `session_offset_ms` below maps
    // those back onto this epoch. Caveat (pre-existing, unrelated):
    // Soniox's clock counts *received audio*, so if the audio client —
    // not Soniox — disconnects mid-session, token times pause while wall
    // time advances; this offset does not (and cannot) correct that.
    let session_started = std::time::Instant::now();
    let mut first_session = true;
    // Owned here, NOT in try_one_session, so Soniox-confirmed text that
    // hasn't reached a flush boundary survives a session error: the Err
    // arm below flushes it before reconnecting or exiting. Clean exits
    // flush inside try_one_session, leaving this empty (and flush on an
    // empty buffer is a no-op), so double-flushing is harmless.
    let mut buf = UtteranceBuffer::default();

    loop {
        if cancel.is_cancelled() {
            return;
        }
        // If a drain was signalled, don't open or reconnect a session.
        // A session error mid-drain (e.g. the end-of-audio write failing)
        // would otherwise reconnect and send end-of-audio to a fresh,
        // empty Soniox session — wasteful and noisy. The Err arm below
        // flushes any confirmed-but-unflushed text before we can get
        // here, so the finalize task's backstop really does collect
        // everything we produced.
        if drain.is_cancelled() {
            return;
        }

        // Epoch offset added to this session's token timestamps. The
        // first session's audio stream starts at meeting start, so its
        // token times are already meeting-relative — offset 0 keeps them
        // bit-for-bit unchanged (audio-aligned precision). Reconnected
        // sessions get wall-elapsed-at-open, accurate to within the
        // ~1.6s audio channel buffer plus connect latency. `elapsed()`
        // is strictly increasing, so timestamps stay monotonic across N
        // reconnects. (Chunks persisted before this fix shipped carry
        // unrecoverable session-relative times; only new meetings are
        // corrected.)
        let session_offset_ms = if first_session {
            0
        } else {
            session_started.elapsed().as_millis() as u64
        };
        first_session = false;

        let mut session_healthy = false;
        let attempt_started = std::time::Instant::now();
        match try_one_session(
            &cfg,
            audio_rx.as_mut(),
            &mut buf,
            &transcript_tx,
            &events_tx,
            &user_id,
            &cancel,
            &drain,
            session_started,
            session_offset_ms,
            &mut session_healthy,
        )
        .await
        {
            Ok(()) => {
                // Clean shutdown — don't reconnect. Return the gauge to 0
                // so "consecutive failures since last success" reads true
                // after the meeting ends (it previously held the last
                // failure count forever).
                info!("Soniox session ended cleanly");
                stt_metrics.set_reconnect_failures(0);
                emit_status_clear(&events_tx, &user_id);
                return;
            }
            Err(e) => {
                // Soniox already confirmed whatever sits in `buf`; flush it
                // before retry/exit so a session error never silently
                // truncates the transcript. The reconnected session gets a
                // fresh empty buffer — Soniox timestamps are per-session,
                // so carrying text across sessions would corrupt chunk
                // timebases, and the reconnect gap is a real utterance
                // boundary anyway.
                buf.flush(&transcript_tx, &user_id, session_started);
                emit_live(&events_tx, &user_id, "", "");
                if drain.is_cancelled() {
                    // Mid-drain failure: never reconnect, and skip the
                    // backoff sleep — the finalize task is waiting on us
                    // and the flush above already saved the tail.
                    warn!(error = %e, "Soniox session failed during drain; exiting after flush");
                    return;
                }
                // A session that produced a final token (connect + auth +
                // model + audio path all proven) or simply stayed up past
                // the threshold resets escalation: its failure is a fresh
                // blip, not failure N of a streak. Connect/config/auth
                // failures stay unhealthy so a revoked API key still
                // escalates to stt_unavailable.
                let was_healthy = session_healthy || attempt_started.elapsed() >= healthy_threshold;
                let (sleep, code) = reconnect.on_failure(was_healthy);
                stt_metrics.set_reconnect_failures(reconnect.consecutive_failures as u64);
                warn!(
                    error = %e,
                    backoff_ms = sleep.as_millis() as u64,
                    consecutive_failures = reconnect.consecutive_failures,
                    was_healthy,
                    "Soniox session failed; reconnecting"
                );
                emit_status_error(&events_tx, &user_id, code);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(sleep) => {}
                }
            }
        }
    }
}

/// Emit a `Status` event carrying an error code (e.g. "stt_reconnecting").
fn emit_status_error(
    events_tx: &broadcast::Sender<crate::protocol::UserEvent>,
    user_id: &str,
    code: &str,
) {
    crate::context::broadcast_user_event(
        events_tx,
        user_id,
        crate::protocol::Event::Status {
            status: crate::protocol::Status {
                listening: true,
                error: Some(code.to_string()),
            },
        },
    );
}

/// Emit a `TranscriptInterim` event carrying the live "in-flight" preview
/// text — the accumulated finalized buffer awaiting flush, plus the latest
/// per-response interim. PWA renders this as a dim italic row at the bottom
/// of transcript mode. Pass empty buffer + empty interim to clear the row
/// (e.g., after flush, on cancel, on session close).
fn emit_live(
    events_tx: &broadcast::Sender<crate::protocol::UserEvent>,
    user_id: &str,
    buffer: &str,
    interim: &str,
) {
    let mut text = String::with_capacity(buffer.len() + interim.len());
    text.push_str(buffer);
    text.push_str(interim);
    crate::context::broadcast_user_event(
        events_tx,
        user_id,
        crate::protocol::Event::TranscriptInterim { text },
    );
}

/// Emit a `Status` event clearing any prior error (successful session open/close).
fn emit_status_clear(events_tx: &broadcast::Sender<crate::protocol::UserEvent>, user_id: &str) {
    crate::context::broadcast_user_event(
        events_tx,
        user_id,
        crate::protocol::Event::Status {
            status: crate::protocol::Status {
                listening: true,
                error: None,
            },
        },
    );
}

/// True if `s` (after trimming trailing whitespace) ends with a sentence
/// terminator. Covers ASCII (.?!) and CJK fullwidth (。？！).
fn ends_with_terminator(s: &str) -> bool {
    let trimmed = s.trim_end();
    match trimmed.chars().next_back() {
        Some(c) => matches!(c, '.' | '?' | '!' | '。' | '？' | '！'),
        None => false,
    }
}

/// True if `s` ends at a "soft" boundary — whitespace, comma, semicolon,
/// or another non-alphanumeric character. Used to gate the idle-timeout
/// flush so we never split mid-word ("Hi, h" + "ello" instead of "Hi, hello").
/// An empty string ends at no boundary.
fn ends_at_soft_boundary(s: &str) -> bool {
    match s.chars().next_back() {
        None => false,
        Some(c) => !c.is_alphanumeric(),
    }
}

/// Soft cap: prefer to flush around here, but only if the buffer
/// ends at a word boundary. Holds back mid-word flushes that
/// produce splits like "Admiral Grace Hopp" / "er. Grace…".
const MAX_BUFFER_LEN_SOFT: usize = 240;

/// Hard cap: force-flush regardless of position. Safety net for
/// pathological cases (a speaker emitting one continuous mono-word
/// or non-Latin script that never hits a soft boundary). Twice the
/// soft cap is comfortable headroom in normal English.
const MAX_BUFFER_LEN_HARD: usize = 480;

/// Combined flush decision used by the streaming pump loop.
/// Pulled out for unit testing — easier than driving the full
/// mock-WS path to assert flush boundaries.
fn should_flush(buffer: &str) -> bool {
    if ends_with_terminator(buffer) {
        return true;
    }
    if buffer.len() >= MAX_BUFFER_LEN_HARD {
        return true;
    }
    buffer.len() >= MAX_BUFFER_LEN_SOFT && ends_at_soft_boundary(buffer)
}

/// Pick a single speaker label for an utterance from the per-token
/// `speaker` values that Soniox returns. Most-common wins; ties break
/// by first-seen order. Returns `None` when no token carried a speaker
/// (diarization disabled or unavailable for the chunk).
fn aggregate_speaker(speakers: &[Option<String>]) -> Option<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for s in speakers.iter().flatten() {
        let key = s.as_str();
        if !counts.contains_key(key) {
            order.push(key);
        }
        *counts.entry(key).or_insert(0) += 1;
    }
    if counts.is_empty() {
        return None;
    }
    let max_count = *counts.values().max().unwrap();
    order
        .into_iter()
        .find(|s| counts[s] == max_count)
        .map(|s| s.to_string())
}

/// Accumulates Soniox-confirmed finalized tokens into utterance-shaped
/// chunks, plus the token timestamps and per-token speaker labels needed
/// at flush time. Owned by `run_soniox` (not `try_one_session`) so
/// confirmed-but-unflushed text survives a session error: the reconnect
/// loop's `Err` arm flushes it before retrying or exiting, instead of the
/// text dying with `try_one_session`'s stack frame.
#[derive(Default)]
struct UtteranceBuffer {
    /// Concatenated finalized-token text awaiting flush.
    text: String,
    /// `start_ms` of the first buffered token with a non-zero timestamp,
    /// already shifted onto the meeting epoch (`session_offset_ms` added
    /// at capture time, so it stays meeting-relative after a reconnect).
    first_start_ms: Option<u64>,
    /// `end_ms` of the most recent buffered token with a non-zero
    /// timestamp, likewise meeting-relative.
    last_end_ms: Option<u64>,
    /// Per-finalized-token speaker labels, parallel to `text`. Aggregated
    /// at flush time via `aggregate_speaker` (most-common wins). Empty when
    /// diarization isn't enabled or tokens carry no speaker.
    speakers: Vec<Option<String>>,
}

impl UtteranceBuffer {
    /// Flush the accumulated finalized-token buffer as a single
    /// TranscriptChunk and clear it. No chunk is emitted if the buffer is
    /// empty after trimming (state is still cleared), so redundant flushes
    /// are harmless no-ops.
    ///
    /// `first_start_ms` / `last_end_ms` hold the first and last token
    /// timestamps across the buffer's lifetime (meeting-relative). When
    /// present (non-zero from Soniox), they become the chunk's
    /// `t_start_ms` / `t_end_ms`. Falls back to meeting-elapsed wall-clock
    /// (`session_started.elapsed()`) if tokens have zero timestamps — also
    /// meeting-relative, so both paths share one timeline.
    fn flush(
        &mut self,
        transcript_tx: &broadcast::Sender<TranscriptChunk>,
        user_id: &str,
        session_started: std::time::Instant,
    ) {
        let trimmed = self.text.trim();
        if trimmed.is_empty() {
            self.clear();
            return;
        }
        let elapsed_ms = session_started.elapsed().as_millis() as u64;
        let t_start = self
            .first_start_ms
            .unwrap_or_else(|| elapsed_ms.saturating_sub(2000));
        let t_end = self.last_end_ms.unwrap_or(elapsed_ms);
        let speaker = aggregate_speaker(&self.speakers);
        // One info log per finalized utterance — operator-meaningful signal
        // that the live pipeline is producing transcripts. Replaces the
        // per-frame PCM/response counters that were too noisy to be useful.
        info!(
            ms = t_end.saturating_sub(t_start),
            user_id = %user_id,
            speaker = ?speaker,
            text = %trimmed,
            "transcript"
        );
        let chunk = TranscriptChunk {
            id: uuid::Uuid::new_v4().to_string(),
            text: trimmed.to_string(),
            t_start_ms: t_start,
            t_end_ms: t_end,
            speaker,
            user_id: user_id.to_string(),
        };
        let _ = transcript_tx.send(chunk);
        self.clear();
    }

    /// Reset all buffer state without emitting.
    fn clear(&mut self) {
        self.text.clear();
        self.first_start_ms = None;
        self.last_end_ms = None;
        self.speakers.clear();
    }
}

/// One WS session attempt. Returns Ok on cancellation, Err on protocol failure
/// or WS disconnect. The caller decides whether to retry.
///
/// `session_healthy` is flipped to `true` the first time a FINAL token
/// arrives — receipt proves connect, auth, model, and the audio path all
/// worked, so the caller resets reconnect escalation even though this
/// attempt ultimately returned Err.
#[allow(clippy::too_many_arguments)] // session plumbing; buffer is caller-owned so error paths can flush it
async fn try_one_session(
    cfg: &SonioxStt,
    mut audio_rx: Option<&mut mpsc::Receiver<Vec<u8>>>,
    buf: &mut UtteranceBuffer,
    transcript_tx: &broadcast::Sender<TranscriptChunk>,
    events_tx: &broadcast::Sender<crate::protocol::UserEvent>,
    user_id: &str,
    cancel: &CancellationToken,
    drain: &CancellationToken,
    session_started: std::time::Instant,
    session_offset_ms: u64,
    session_healthy: &mut bool,
) -> Result<(), String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (ws, _) = tokio_tungstenite::connect_async(cfg.url.as_str())
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut writer, mut reader) = ws.split();

    // 1. Send config frame
    let config = ConfigFrame {
        api_key: &cfg.api_key,
        audio_format: "pcm_s16le",
        sample_rate: SAMPLE_RATE,
        num_channels: 1,
        model: &cfg.model,
        enable_speaker_diarization: true,
    };
    let config_json = serde_json::to_string(&config).map_err(|e| format!("encode config: {e}"))?;
    writer
        .send(Message::Text(config_json))
        .await
        .map_err(|e| format!("send config: {e}"))?;

    info!(model = %cfg.model, "Soniox session opened");
    // Clear any prior reconnecting/unavailable status now that we're connected.
    emit_status_clear(events_tx, user_id);

    // Buffer finalized tokens into utterance-shaped chunks. Soniox's
    // stt-rt-preview emits sub-word tokens (e.g. "Hell", "o,", "how",
    // "are", "y", "ou"); rendering each as its own Item produces a
    // letter-stack column in the PWA. Instead we accumulate finalized
    // tokens and flush a single TranscriptChunk on:
    //   1. Sentence-terminator punctuation (`.`, `?`, `!`, CJK equivalents)
    //   2. Buffer length >= MAX_BUFFER_LEN (avoid unbounded growth on
    //      monologues without punctuation)
    //   3. Idle timeout >= IDLE_FLUSH_MS (user paused; emit what we have)
    //   4. Session end (cancel or WS close): flush whatever's in flight
    //   5. Session ERROR: the buffer is owned by run_soniox, whose Err
    //      arm flushes it — none of the `return Err` paths below lose
    //      confirmed text.
    let mut last_token_at: Option<std::time::Instant> = None;
    // Drain state: set when the finalize task fires the drain signal.
    // While draining we stop forwarding new PCM and keep reading the
    // socket until Soniox sends `finished` / closes, or the grace
    // deadline elapses.
    let mut draining = false;
    let drain_grace = Duration::from_millis(drain_grace_ms());
    let mut drain_deadline: Option<std::time::Instant> = None;
    // Default 3s of no new tokens = a real conversational pause.
    // Sub-3s gaps are mid-sentence (breath, thinking). Combined with
    // the soft-boundary gate below, this prevents the "Hi, h" /
    // "ello, how are you?" fragmentation that 1s + no-gating produced.
    // Override via `AURIS_SONIOX_IDLE_FLUSH_MS`.
    let idle_flush_ms = idle_flush_ms();
    let mut idle_ticker = tokio::time::interval(Duration::from_millis(500));
    idle_ticker.tick().await; // discard immediate tick

    // 2. Pump loop: forward audio, parse transcripts
    loop {
        tokio::select! {
            // Graceful drain: flush any PCM still buffered locally to
            // Soniox, then send the empty frame that signals end-of-audio.
            // Keep the loop running to read trailing finals + `finished`.
            _ = drain.cancelled(), if !draining => {
                if let Some(rx) = audio_rx.as_deref_mut() {
                    while let Ok(bytes) = rx.try_recv() {
                        if let Err(e) = writer.send(Message::Binary(bytes)).await {
                            return Err(format!("send pcm (drain): {e}"));
                        }
                    }
                }
                if let Err(e) = writer.send(Message::Text(String::new())).await {
                    return Err(format!("send end-of-audio: {e}"));
                }
                draining = true;
                drain_deadline = Some(std::time::Instant::now() + drain_grace);
                info!(user_id = %user_id, "Soniox draining: end-of-audio sent, awaiting finals");
            }
            _ = cancel.cancelled() => {
                buf.flush(transcript_tx, user_id, session_started);
                emit_live(events_tx, user_id, "", "");
                let _ = writer.close().await;
                return Ok(());
            }
            // Forward PCM bytes from audio task to Soniox
            pcm = async {
                match audio_rx.as_deref_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending::<Option<Vec<u8>>>().await,
                }
            }, if !draining => {
                match pcm {
                    Some(bytes) => {
                        if let Err(e) = writer.send(Message::Binary(bytes)).await {
                            return Err(format!("send pcm: {e}"));
                        }
                    }
                    None => {
                        // Audio source ended — close the session
                        buf.flush(transcript_tx, user_id, session_started);
                        emit_live(events_tx, user_id, "", "");
                        let _ = writer.close().await;
                        return Ok(());
                    }
                }
            }
            // Idle flush: every 500ms, check if the buffer is stale AND
            // ends at a soft boundary (whitespace/punctuation). The
            // soft-boundary gate prevents splitting mid-word when Soniox
            // sub-word tokens straddle a long pause.
            _ = idle_ticker.tick() => {
                if let Some(deadline) = drain_deadline {
                    if std::time::Instant::now() >= deadline {
                        buf.flush(transcript_tx, user_id, session_started);
                        emit_live(events_tx, user_id, "", "");
                        let _ = writer.close().await;
                        info!(user_id = %user_id, "Soniox drain grace elapsed; closing");
                        return Ok(());
                    }
                }
                if !buf.text.is_empty() {
                    if let Some(t) = last_token_at {
                        let stale = t.elapsed().as_millis() as u64 >= idle_flush_ms;
                        if stale && ends_at_soft_boundary(&buf.text) {
                            buf.flush(transcript_tx, user_id, session_started);
                            last_token_at = None;
                            emit_live(events_tx, user_id, "", "");
                        }
                    }
                }
            }
            // Read transcript responses
            msg = reader.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<TokenResponse>(&t) {
                            Ok(resp) => {
                                // Auth error?
                                if resp.error_code == Some(401)
                                    || resp.error.as_ref().and_then(|e| e.code) == Some(401)
                                {
                                    return Err("auth: invalid API key".into());
                                }
                                if resp.finished {
                                    buf.flush(transcript_tx, user_id, session_started);
                                    emit_live(events_tx, user_id, "", "");
                                    let _ = writer.close().await;
                                    if draining {
                                        info!(user_id = %user_id, "Soniox finished; session drained cleanly");
                                        return Ok(());
                                    }
                                    // `finished` outside a drain is anomalous (e.g. a
                                    // provider-side session limit). Treat it like a
                                    // server close: return Err so run_soniox reconnects
                                    // rather than silently ending a live meeting.
                                    return Err("Soniox sent finished outside drain".into());
                                }
                                // Separate final and interim tokens; accumulate finals,
                                // emit a live preview combining the accumulated buffer
                                // and the per-response interim text. The PWA renders this
                                // as a dim "live" row at the bottom of the transcript pane.
                                let mut got_final = false;
                                let mut interim_text = String::new();
                                for tok in resp.tokens {
                                    if tok.is_final {
                                        if tok.text.is_empty() {
                                            continue;
                                        }
                                        // Token times are session-relative; shift onto the
                                        // meeting epoch so reconnected sessions don't reset
                                        // the timeline to 0 (offset is 0 for session 1).
                                        if buf.first_start_ms.is_none() && tok.start_ms > 0 {
                                            buf.first_start_ms =
                                                Some(session_offset_ms + tok.start_ms);
                                        }
                                        if tok.end_ms > 0 {
                                            buf.last_end_ms =
                                                Some(session_offset_ms + tok.end_ms);
                                        }
                                        buf.text.push_str(&tok.text);
                                        buf.speakers.push(tok.speaker.clone());
                                        got_final = true;
                                    } else {
                                        interim_text.push_str(&tok.text);
                                    }
                                }
                                if got_final {
                                    last_token_at = Some(std::time::Instant::now());
                                    *session_healthy = true;
                                }
                                // Flush on punctuation or length cap (soft cap
                                // requires word boundary; hard cap force-flushes).
                                // After flush the buffer is empty and the live
                                // row should clear.
                                if should_flush(&buf.text) {
                                    buf.flush(transcript_tx, user_id, session_started);
                                    last_token_at = None;
                                }
                                // Emit a live preview AFTER any flush so the buffer
                                // value reflects post-flush state. The live text is
                                // what the user sees as the "in flight" row in the
                                // PWA's transcript pane.
                                emit_live(events_tx, user_id, &buf.text, &interim_text);
                            }
                            Err(e) => {
                                warn!(error = %e, raw = %t, "Soniox response parse error");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        buf.flush(transcript_tx, user_id, session_started);
                        emit_live(events_tx, user_id, "", "");
                        if draining {
                            return Ok(());
                        }
                        return Err("Soniox closed connection".into());
                    }
                    Some(Ok(_)) => {} // ignore Pong/Ping/Binary
                    Some(Err(e)) => {
                        return Err(format!("ws read: {e}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    /// Spin up a local mock WS server on an OS-assigned port; return its URL
    /// and a oneshot to wait for the server task's exit.
    async fn spawn_mock_server<F, Fut>(handler: F) -> (String, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{}/", addr);
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = accept_async(stream).await.unwrap();
            handler(ws).await;
        });
        (url, handle)
    }

    #[tokio::test]
    async fn soniox_parses_final_tokens() {
        let (url, server) = spawn_mock_server(|mut ws| async move {
            // Read config frame
            let _config = ws.next().await.unwrap().unwrap();
            // Send a canned response with a final token
            let resp = serde_json::json!({
                "tokens": [
                    { "text": "hello world.", "is_final": true, "start_ms": 100, "end_ms": 800 }
                ]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            // Hold the connection a moment so the client can read it
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Wait for the chunk
        let chunk =
            tokio::time::timeout(std::time::Duration::from_millis(500), transcript_rx.recv())
                .await
                .expect("timeout waiting for transcript")
                .expect("recv");

        // Buffered finalization preserves the trailing punctuation and emits
        // the trimmed accumulated text. Timestamps now reflect the token's
        // actual start_ms / end_ms from the Soniox response.
        assert_eq!(chunk.text, "hello world.");
        assert_eq!(chunk.t_start_ms, 100);
        assert_eq!(chunk.t_end_ms, 800);

        // Cancel the client; let it shut down cleanly.
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_skips_interim_tokens() {
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            // Interim token only — should NOT produce a chunk
            let resp = serde_json::json!({
                "tokens": [{ "text": "partial", "is_final": false }]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Should NOT receive a chunk within 300ms
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(300), transcript_rx.recv()).await;
        assert!(
            result.is_err(),
            "interim tokens must not produce TranscriptChunks"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_buffers_subword_tokens_until_punctuation() {
        // Simulates Soniox stt-rt-preview's subword token stream:
        // sends "Hell", "o,", " how", " are", " you", "?" — each as a
        // separate finalized token. Expects ONE TranscriptChunk with the
        // concatenated text, flushed by the trailing "?".
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [
                    { "text": "Hell",  "is_final": true },
                    { "text": "o,",    "is_final": true },
                    { "text": " how",  "is_final": true },
                    { "text": " are",  "is_final": true },
                    { "text": " you",  "is_final": true },
                    { "text": "?",     "is_final": true },
                ]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        let chunk =
            tokio::time::timeout(std::time::Duration::from_millis(500), transcript_rx.recv())
                .await
                .expect("timeout waiting for buffered chunk")
                .expect("recv");
        assert_eq!(chunk.text, "Hello, how are you?");

        // No further chunk should arrive (single response → single buffered emit).
        let second =
            tokio::time::timeout(std::time::Duration::from_millis(200), transcript_rx.recv()).await;
        assert!(
            second.is_err(),
            "expected exactly one buffered chunk, got more"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[test]
    fn ends_with_terminator_recognizes_ascii_and_cjk() {
        assert!(ends_with_terminator("Hello."));
        assert!(ends_with_terminator("How are you?"));
        assert!(ends_with_terminator("Wow!"));
        assert!(ends_with_terminator("Trailing space ends. "));
        assert!(ends_with_terminator("Japanese 。"));
        assert!(!ends_with_terminator("no punctuation here"));
        assert!(!ends_with_terminator(""));
    }

    #[test]
    fn ends_at_soft_boundary_gates_mid_word_idle_flush() {
        // Soft boundaries — safe to idle-flush
        assert!(ends_at_soft_boundary("Hi, "));
        assert!(ends_at_soft_boundary("Hello,"));
        assert!(ends_at_soft_boundary("Done."));
        assert!(ends_at_soft_boundary("Wait;"));
        // Mid-word — must NOT idle-flush, would split "Hi, h" / "ello..."
        assert!(!ends_at_soft_boundary("Hi, h"));
        assert!(!ends_at_soft_boundary("Hello"));
        assert!(!ends_at_soft_boundary("123"));
        // Empty
        assert!(!ends_at_soft_boundary(""));
    }

    #[tokio::test]
    async fn soniox_emits_interim_event_for_nonfinal_tokens() {
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [
                    { "text": "Hello,", "is_final": true },
                    { "text": " how are you", "is_final": false },
                ]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;

        let (transcript_tx, _) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, mut events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Drain events; expect at least one TranscriptInterim with our interim text
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut saw_interim = false;
        while tokio::time::Instant::now() < deadline && !saw_interim {
            if let Ok(Ok(evt)) =
                tokio::time::timeout(std::time::Duration::from_millis(200), events_rx.recv()).await
            {
                if let crate::protocol::Event::TranscriptInterim { text } = evt.event {
                    assert!(
                        text.contains("how are you"),
                        "unexpected interim text: {text}"
                    );
                    saw_interim = true;
                }
            }
        }
        assert!(saw_interim, "expected TranscriptInterim event");

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_emits_status_error_on_reconnect() {
        // Mock server that immediately closes — forces a reconnect
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{}/", addr);
        let server = tokio::spawn(async move {
            // Accept and immediately drop without doing handshake
            for _ in 0..2 {
                let _ = listener.accept().await;
            }
        });

        let (transcript_tx, _) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, mut events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Wait for a Status event with non-None error
        let mut saw_error = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline && !saw_error {
            if let Ok(Ok(evt)) =
                tokio::time::timeout(std::time::Duration::from_millis(500), events_rx.recv()).await
            {
                if let crate::protocol::Event::Status { status } = evt.event {
                    if status.error.is_some() {
                        saw_error = true;
                    }
                }
            }
        }
        assert!(
            saw_error,
            "expected Status event with error during reconnect"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_recovers_to_reconnecting_after_healthy_session() {
        use tokio_tungstenite::tungstenite::Message;

        // Fast reconnects so 7 attempts complete in milliseconds. Same
        // save/restore env pattern as soniox_drain_force_closes_on_grace_deadline.
        let prev_base = std::env::var("AURIS_SONIOX_RECONNECT_BASE_MS").ok();
        std::env::set_var("AURIS_SONIOX_RECONNECT_BASE_MS", "1");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{}/", addr);
        let server = tokio::spawn(async move {
            // Connections 1-5: accept and drop without a WS handshake →
            // five unhealthy failures; the 5th escalates to stt_unavailable.
            for _ in 0..5 {
                let _ = listener.accept().await;
            }
            // Connection 6: a real, healthy session — handshake, read the
            // config frame, send one FINAL token ("hello." flushes
            // immediately and proves the session healthy), then close.
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = accept_async(stream).await {
                    let _config = ws.next().await;
                    let resp = serde_json::json!({
                        "tokens": [
                            { "text": "hello.", "is_final": true, "start_ms": 10, "end_ms": 50 }
                        ]
                    });
                    let _ = ws.send(Message::Text(resp.to_string())).await;
                    // Let the client read the token before we drop the WS.
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    let _ = ws.close(None).await;
                }
            }
            // Connections 7+: accept-and-drop again so each retry fails fast.
            loop {
                if listener.accept().await.is_err() {
                    return;
                }
            }
        });

        let (transcript_tx, _transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, mut events_rx) = broadcast::channel::<crate::protocol::UserEvent>(64);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // State machine over Status events:
        //   1. see error == "stt_unavailable"        (failure 5)
        //   2. see error == None                     (clear on healthy connect)
        //   3. capture the FIRST error code after that.
        let mut saw_unavailable = false;
        let mut saw_clear_after_unavailable = false;
        let mut first_code_after_recovery: Option<String> = None;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && first_code_after_recovery.is_none() {
            match tokio::time::timeout(std::time::Duration::from_millis(500), events_rx.recv())
                .await
            {
                Ok(Ok(evt)) => {
                    if let crate::protocol::Event::Status { status } = evt.event {
                        match status.error {
                            Some(code) => {
                                if saw_clear_after_unavailable {
                                    first_code_after_recovery = Some(code);
                                } else if code == "stt_unavailable" {
                                    saw_unavailable = true;
                                }
                            }
                            None => {
                                if saw_unavailable {
                                    saw_clear_after_unavailable = true;
                                }
                            }
                        }
                    }
                }
                _ => break, // recv error or quiet 500ms — fall through to asserts
            }
        }

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        server.abort();
        match prev_base {
            Some(v) => std::env::set_var("AURIS_SONIOX_RECONNECT_BASE_MS", v),
            None => std::env::remove_var("AURIS_SONIOX_RECONNECT_BASE_MS"),
        }

        assert!(
            saw_unavailable,
            "expected escalation to stt_unavailable after 5 unhealthy failures"
        );
        assert!(
            saw_clear_after_unavailable,
            "expected a Status clear when the healthy session connected"
        );
        assert_eq!(
            first_code_after_recovery.as_deref(),
            Some("stt_reconnecting"),
            "a failure AFTER a healthy session must de-escalate to stt_reconnecting, \
             not keep reporting stt_unavailable from the lifetime counter"
        );
    }

    #[tokio::test]
    async fn soniox_reconnect_offsets_token_timestamps_to_meeting_time() {
        use tokio_tungstenite::tungstenite::Message;

        // Two sequential WS sessions on ONE listener, both with real
        // handshakes. Session 1 emits a final token then closes (forcing
        // run_soniox's reconnect path); session 2 emits a final token
        // whose Soniox clock has restarted near 0. Timestamps in the
        // emitted chunks must stay meeting-relative and monotonic.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("ws://{}/", addr);
        let server = tokio::spawn(async move {
            // --- Session 1 ---
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [
                    { "text": "first.", "is_final": true, "start_ms": 100, "end_ms": 800 }
                ]
            });
            ws.send(Message::Text(resp.to_string())).await.unwrap();
            // Hold the session open so session-1 wall time is a
            // guaranteed >= 500ms component of the reconnect offset,
            // then close to force `Err("Soniox closed connection")`.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = ws.close(None).await;
            // --- Session 2 (after the client's ~500ms default backoff) ---
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [
                    { "text": "second.", "is_final": true, "start_ms": 120, "end_ms": 900 }
                ]
            });
            ws.send(Message::Text(resp.to_string())).await.unwrap();
            // Hold the connection a moment so the client can read it.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        });

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Chunk 1: first session — timestamps must be the verbatim token
        // times (offset 0). Guards that the fix doesn't shift session 1.
        let chunk1 = tokio::time::timeout(std::time::Duration::from_secs(2), transcript_rx.recv())
            .await
            .expect("timeout waiting for session-1 chunk")
            .expect("recv");
        assert_eq!(chunk1.text, "first.");
        assert_eq!(chunk1.t_start_ms, 100);
        assert_eq!(chunk1.t_end_ms, 800);

        // Chunk 2: reconnected session — Soniox's token clock restarted
        // near 0 (start_ms=120), but the emitted chunk must be offset to
        // meeting time. Session-1 lifetime (>=500ms sleep) + default
        // reconnect backoff (500ms) guarantee an offset >= ~1000ms, so:
        //   - ordering: chunk2 starts after chunk1 ends (no time travel)
        //   - loose lower bound: well past the raw 120ms session time
        let chunk2 = tokio::time::timeout(std::time::Duration::from_secs(5), transcript_rx.recv())
            .await
            .expect("timeout waiting for session-2 chunk")
            .expect("recv");
        assert_eq!(chunk2.text, "second.");
        assert!(
            chunk2.t_start_ms > chunk1.t_end_ms,
            "post-reconnect chunk regressed to session-relative time: \
             t_start_ms={} (must be > {})",
            chunk2.t_start_ms,
            chunk1.t_end_ms
        );
        assert!(
            chunk2.t_start_ms >= 900,
            "post-reconnect t_start_ms={} not offset to meeting time",
            chunk2.t_start_ms
        );
        assert!(
            chunk2.t_end_ms > chunk2.t_start_ms,
            "chunk2 end {} must follow start {}",
            chunk2.t_end_ms,
            chunk2.t_start_ms
        );

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
    }

    #[test]
    fn from_env_reads_model_override() {
        // Save and restore env state
        let prev_model = std::env::var("AURIS_SONIOX_MODEL").ok();
        let prev_key = std::env::var("SONIOX_API_KEY").ok();

        std::env::set_var("SONIOX_API_KEY", "test_key");
        std::env::set_var("AURIS_SONIOX_MODEL", "custom-model");
        let s = SonioxStt::from_env().unwrap();
        assert_eq!(s.model, "custom-model");

        std::env::remove_var("AURIS_SONIOX_MODEL");
        let s = SonioxStt::from_env().unwrap();
        assert_eq!(s.model, MODEL_DEFAULT);

        // Restore
        match prev_model {
            Some(v) => std::env::set_var("AURIS_SONIOX_MODEL", v),
            None => std::env::remove_var("AURIS_SONIOX_MODEL"),
        }
        match prev_key {
            Some(v) => std::env::set_var("SONIOX_API_KEY", v),
            None => std::env::remove_var("SONIOX_API_KEY"),
        }
    }

    #[test]
    fn should_flush_true_on_terminator_under_soft_cap() {
        assert!(should_flush("hello world."));
        assert!(should_flush("Wait?"));
    }

    #[test]
    fn should_flush_false_when_under_soft_cap_no_terminator() {
        assert!(!should_flush("partial sentence with"));
    }

    #[test]
    fn should_flush_false_at_soft_cap_when_mid_word() {
        // 250 chars, no whitespace at end → don't split mid-word.
        let s = "a".repeat(MAX_BUFFER_LEN_SOFT + 10);
        assert!(!should_flush(&s));
    }

    #[test]
    fn should_flush_true_at_soft_cap_when_at_soft_boundary() {
        // 250 chars ending in whitespace → safe to flush.
        let mut s = "a".repeat(MAX_BUFFER_LEN_SOFT + 9);
        s.push(' ');
        assert!(should_flush(&s));
    }

    #[test]
    fn should_flush_true_at_hard_cap_regardless() {
        // Hard cap force-flushes even mid-word — safety net for
        // mono-word streams that never hit a soft boundary.
        let s = "a".repeat(MAX_BUFFER_LEN_HARD + 1);
        assert!(should_flush(&s));
    }

    #[tokio::test]
    async fn soniox_drains_on_signal_and_finishes_cleanly() {
        // Server: send one buffered-but-unflushed final (no terminator),
        // then wait for the client's empty end-of-audio frame, then
        // reply with {"finished":true} and close. The client must flush
        // the buffered token as a chunk and return cleanly (Ok, no
        // reconnect) — proving in-flight text isn't lost on stop.
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [{ "text": "trailing words", "is_final": true, "start_ms": 10, "end_ms": 50 }]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(resp.to_string()))
                .await
                .unwrap();
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) if t.is_empty() => {
                        break
                    }
                    Some(Ok(_)) => continue,
                    _ => return,
                }
            }
            let fin = serde_json::json!({ "tokens": [], "finished": true });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(fin.to_string()))
                .await
                .unwrap();
            let _ = ws.close(None).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let drain = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = drain.clone();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drain.cancel();

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), transcript_rx.recv())
            .await
            .expect("timeout waiting for drained chunk")
            .expect("recv");
        assert_eq!(chunk.text, "trailing words");

        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("STT task did not exit after drain")
            .expect("join");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_drain_force_closes_on_grace_deadline() {
        // Server accepts config, sends one buffered final, then goes
        // silent forever (never sends `finished`, never closes). With a
        // short drain grace the client must still flush + return Ok.
        std::env::set_var("AURIS_SONIOX_DRAIN_MS", "300");
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [{ "text": "stuck words", "is_final": true }]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let drain = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = drain.clone();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drain.cancel();

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), transcript_rx.recv())
            .await
            .expect("timeout waiting for force-flushed chunk")
            .expect("recv");
        assert_eq!(chunk.text, "stuck words");

        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("STT task did not force-close on grace deadline")
            .expect("join");
        std::env::remove_var("AURIS_SONIOX_DRAIN_MS");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_flushes_buffer_when_ws_dies_mid_utterance() {
        // Regression for improvement #13, case 1 (mid-meeting transient
        // error): the server sends one finalized token WITHOUT a sentence
        // terminator (so it stays buffered, unflushed), then drops the TCP
        // stream without a WS Close handshake. The client hits the
        // `reader.next() => Some(Err(..))` path in try_one_session. The
        // Soniox-confirmed text must still be emitted as a TranscriptChunk
        // (flushed by run_soniox's Err-arm epilogue) instead of dying with
        // try_one_session's stack frame.
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [{ "text": "trailing words", "is_final": true, "start_ms": 10, "end_ms": 50 }]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            // Give the client time to read + buffer the token, then return,
            // dropping `ws` abruptly — TCP FIN, no WS Close frame.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = CancellationToken::new();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), transcript_rx.recv())
            .await
            .expect("timeout: buffered text was dropped on ws error instead of flushed")
            .expect("recv");
        assert_eq!(chunk.text, "trailing words");

        // Stop the reconnect loop (the mock listener is gone, so the client
        // is cycling connect-failure backoffs by now).
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[tokio::test]
    async fn soniox_flushes_buffer_when_session_dies_during_drain() {
        // Regression for improvement #13, case 2 (the permanent-loss case):
        // the server sends an unterminated finalized token; the test fires
        // `drain.cancel()`; the client's drain arm sends the empty
        // end-of-audio frame; the server then drops the socket abruptly —
        // no `finished`, no Close. try_one_session returns Err while drain
        // is cancelled, so run_soniox must NOT reconnect. Before the fix the
        // buffered text was never sent on transcript_tx, so the finalize
        // tail collection never saw it: the meeting's last sentence was
        // permanently lost. Assertions are race-immune by design: whichever
        // select arm observes the death first (write failure or ws-read
        // error), we assert the OUTCOME — the chunk arrives AND the
        // provider task exits without reconnecting.
        let (url, server) = spawn_mock_server(|mut ws| async move {
            let _config = ws.next().await.unwrap().unwrap();
            let resp = serde_json::json!({
                "tokens": [{ "text": "so the action item is", "is_final": true, "start_ms": 10, "end_ms": 50 }]
            });
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                resp.to_string(),
            ))
            .await
            .unwrap();
            // Wait for the client's empty end-of-audio text frame (sent by
            // the drain arm), then return — dropping the socket mid-drain
            // with no `finished` response and no Close handshake.
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) if t.is_empty() => {
                        break
                    }
                    Some(Ok(_)) => continue,
                    _ => return,
                }
            }
        })
        .await;

        let (transcript_tx, mut transcript_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (events_tx, _events_rx) = broadcast::channel::<crate::protocol::UserEvent>(16);
        let cancel = CancellationToken::new();
        let drain = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task_drain = drain.clone();
        let task = tokio::spawn(async move {
            provider
                .run(
                    None,
                    transcript_tx,
                    events_tx,
                    "test-user".into(),
                    task_cancel,
                    task_drain,
                )
                .await;
        });

        // Let the client read + buffer the token, then signal drain.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drain.cancel();

        // 1) The confirmed text must be flushed despite the session error.
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), transcript_rx.recv())
            .await
            .expect("timeout: drain-path session error permanently dropped buffered text")
            .expect("recv");
        assert_eq!(chunk.text, "so the action item is");

        // 2) The provider must exit promptly without reconnecting
        //    (drain is cancelled — finalize is waiting on this task).
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("STT task did not exit after drain-path session error")
            .expect("join");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    #[test]
    fn aggregate_speaker_returns_none_for_empty_input() {
        let speakers: Vec<Option<String>> = vec![];
        assert_eq!(aggregate_speaker(&speakers), None);
    }

    #[test]
    fn aggregate_speaker_returns_none_when_all_none() {
        let speakers: Vec<Option<String>> = vec![None, None, None];
        assert_eq!(aggregate_speaker(&speakers), None);
    }

    #[test]
    fn aggregate_speaker_returns_single_speaker_when_uniform() {
        let speakers = vec![
            Some("1".to_string()),
            Some("1".to_string()),
            Some("1".to_string()),
        ];
        assert_eq!(aggregate_speaker(&speakers), Some("1".to_string()));
    }

    #[test]
    fn aggregate_speaker_picks_most_common_when_mixed() {
        let speakers = vec![
            Some("1".to_string()),
            Some("2".to_string()),
            Some("1".to_string()),
            Some("1".to_string()),
        ];
        assert_eq!(aggregate_speaker(&speakers), Some("1".to_string()));
    }

    #[test]
    fn aggregate_speaker_breaks_ties_by_first_seen() {
        let speakers = vec![
            Some("2".to_string()),
            Some("1".to_string()),
            Some("2".to_string()),
            Some("1".to_string()),
        ];
        // 2 and 1 each appear twice; 2 was seen first → 2 wins.
        assert_eq!(aggregate_speaker(&speakers), Some("2".to_string()));
    }

    #[test]
    fn aggregate_speaker_ignores_none_entries_in_count() {
        let speakers = vec![
            None,
            Some("1".to_string()),
            None,
            Some("2".to_string()),
            Some("1".to_string()),
        ];
        // 1 appears twice, 2 once → 1 wins; None entries ignored.
        assert_eq!(aggregate_speaker(&speakers), Some("1".to_string()));
    }

    #[test]
    fn utterance_buffer_flush_is_noop_when_empty_and_clears_state() {
        let (tx, mut rx) = broadcast::channel::<TranscriptChunk>(4);
        let started = std::time::Instant::now();

        // Whitespace-only buffer: flush must NOT emit a chunk, but must
        // still clear all four pieces of state (ports the guard from the
        // old free fn `flush_buffer`).
        let mut buf = UtteranceBuffer {
            text: "   ".to_string(),
            first_start_ms: Some(10),
            last_end_ms: Some(20),
            speakers: vec![Some("1".to_string())],
        };
        buf.flush(&tx, "test-user", started);
        assert!(
            rx.try_recv().is_err(),
            "whitespace-only flush must not emit a chunk"
        );
        assert!(buf.text.is_empty());
        assert_eq!(buf.first_start_ms, None);
        assert_eq!(buf.last_end_ms, None);
        assert!(buf.speakers.is_empty());

        // Non-empty buffer: flush emits exactly one trimmed chunk carrying
        // the buffered token timestamps and aggregated speaker, then clears.
        buf.text.push_str(" hello there. ");
        buf.first_start_ms = Some(100);
        buf.last_end_ms = Some(900);
        buf.speakers.push(Some("2".to_string()));
        buf.flush(&tx, "test-user", started);
        let chunk = rx.try_recv().expect("non-empty flush must emit a chunk");
        assert_eq!(chunk.text, "hello there.");
        assert_eq!(chunk.t_start_ms, 100);
        assert_eq!(chunk.t_end_ms, 900);
        assert_eq!(chunk.speaker, Some("2".to_string()));
        assert_eq!(chunk.user_id, "test-user");
        assert!(buf.text.is_empty());
        assert_eq!(buf.first_start_ms, None);
        assert_eq!(buf.last_end_ms, None);
        assert!(buf.speakers.is_empty());

        // Default construction starts empty.
        let buf2 = UtteranceBuffer::default();
        assert!(buf2.text.is_empty() && buf2.speakers.is_empty());
    }

    #[test]
    fn reconnect_state_escalates_to_unavailable_after_5_failures() {
        let mut st = ReconnectState::new(Duration::from_millis(500), Duration::from_secs(30));
        // Failures 1-4: still "reconnecting".
        let mut last_code = "";
        for _ in 0..4 {
            let (_, code) = st.on_failure(false);
            last_code = code;
        }
        assert_eq!(last_code, "stt_reconnecting");
        // Failure 5 and beyond: escalated.
        let (_, code) = st.on_failure(false);
        assert_eq!(code, "stt_unavailable");
        let (_, code) = st.on_failure(false);
        assert_eq!(code, "stt_unavailable");
    }

    #[test]
    fn reconnect_state_backoff_doubles_and_caps() {
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(4);
        let mut st = ReconnectState::new(base, max);
        // Sleep-before-retry sequence: base, 2x, 4x, then pinned at max.
        let expected_ms = [500u64, 1000, 2000, 4000, 4000, 4000];
        for (i, exp) in expected_ms.iter().enumerate() {
            let (sleep, _) = st.on_failure(false);
            assert_eq!(sleep, Duration::from_millis(*exp), "attempt {i}");
        }
    }

    #[test]
    fn reconnect_state_resets_after_healthy_session() {
        let base = Duration::from_millis(500);
        let mut st = ReconnectState::new(base, Duration::from_secs(30));
        // Six unhealthy failures: escalated, backoff well above base.
        for _ in 0..6 {
            st.on_failure(false);
        }
        assert_eq!(st.consecutive_failures, 6);
        // A failure AFTER a session that proved healthy restarts the
        // escalation: counter becomes 1 (this failure), code de-escalates
        // to "stt_reconnecting", and the retry sleep is back at base.
        let (sleep, code) = st.on_failure(true);
        assert_eq!(code, "stt_reconnecting");
        assert_eq!(sleep, base);
        assert_eq!(st.consecutive_failures, 1);
    }
}
