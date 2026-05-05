//! Soniox streaming STT client.
//! Wire-format reference: `packages/pwa/src/stt/soniox.ts` (TypeScript impl,
//! confirmed working with the user's Soniox account).

use crate::stt::{SttInitError, SttProvider, TranscriptChunk};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const SONIOX_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
const MODEL_DEFAULT: &str = "stt-rt-preview";
const SAMPLE_RATE: u32 = 16000;
const RECONNECT_BASE: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

pub struct SonioxStt {
    api_key: String,
    url: String,
    model: String,
}

impl SonioxStt {
    pub fn from_env() -> Result<Self, SttInitError> {
        let api_key = std::env::var("SONIOX_API_KEY").map_err(|_| {
            SttInitError::MissingCredentials(
                "SONIOX_API_KEY is required when MEETING_COMPANION_STT_PROVIDER=soniox".to_string(),
            )
        })?;
        if api_key.is_empty() {
            return Err(SttInitError::MissingCredentials(
                "SONIOX_API_KEY is empty".to_string(),
            ));
        }
        let model = std::env::var("MEETING_COMPANION_SONIOX_MODEL")
            .unwrap_or_else(|_| MODEL_DEFAULT.to_string());
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
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    tokens: Vec<Token>,
    #[serde(default)]
    error_code: Option<i32>,
    #[serde(default)]
    error: Option<ErrorBlock>,
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
        events_tx: broadcast::Sender<crate::contract::Event>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(run_soniox(
            *self,
            audio_rx,
            transcript_tx,
            events_tx,
            cancel,
        ))
    }
}

/// Main run loop. Reconnects with exponential backoff if the WS drops.
/// Stops cleanly on cancel.
async fn run_soniox(
    cfg: SonioxStt,
    mut audio_rx: Option<mpsc::Receiver<Vec<u8>>>,
    transcript_tx: broadcast::Sender<TranscriptChunk>,
    events_tx: broadcast::Sender<crate::contract::Event>,
    cancel: CancellationToken,
) {
    if audio_rx.is_none() {
        warn!("Soniox provider has no audio source; transcription will produce nothing");
    }

    let mut consecutive_failures: u32 = 0;
    let mut backoff = RECONNECT_BASE;
    let session_started = std::time::Instant::now();

    loop {
        if cancel.is_cancelled() {
            return;
        }

        match try_one_session(
            &cfg,
            audio_rx.as_mut(),
            &transcript_tx,
            &events_tx,
            &cancel,
            session_started,
        )
        .await
        {
            Ok(()) => {
                // Clean shutdown — don't reconnect
                info!("Soniox session ended cleanly");
                emit_status_clear(&events_tx);
                return;
            }
            Err(e) => {
                consecutive_failures += 1;
                warn!(error = %e, backoff_ms = backoff.as_millis() as u64, consecutive_failures, "Soniox session failed; reconnecting");
                let code = if consecutive_failures >= 5 {
                    "stt_unavailable"
                } else {
                    "stt_reconnecting"
                };
                emit_status_error(&events_tx, code);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        }
    }
}

/// Emit a `Status` event carrying an error code (e.g. "stt_reconnecting").
fn emit_status_error(events_tx: &broadcast::Sender<crate::contract::Event>, code: &str) {
    let _ = events_tx.send(crate::contract::Event::Status {
        status: crate::contract::Status {
            listening: true,
            paused: false,
            error: Some(code.to_string()),
        },
    });
}

/// Emit a `TranscriptInterim` event carrying the live "in-flight" preview
/// text — the accumulated finalized buffer awaiting flush, plus the latest
/// per-response interim. PWA renders this as a dim italic row at the bottom
/// of transcript mode. Pass empty buffer + empty interim to clear the row
/// (e.g., after flush, on cancel, on session close).
fn emit_live(events_tx: &broadcast::Sender<crate::contract::Event>, buffer: &str, interim: &str) {
    let mut text = String::with_capacity(buffer.len() + interim.len());
    text.push_str(buffer);
    text.push_str(interim);
    let _ = events_tx.send(crate::contract::Event::TranscriptInterim { text });
}

/// Emit a `Status` event clearing any prior error (successful session open/close).
fn emit_status_clear(events_tx: &broadcast::Sender<crate::contract::Event>) {
    let _ = events_tx.send(crate::contract::Event::Status {
        status: crate::contract::Status {
            listening: true,
            paused: false,
            error: None,
        },
    });
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

/// Flush the accumulated finalized-token buffer as a single TranscriptChunk
/// and clear it. No-op if the buffer is empty after trimming.
///
/// `buffer_first_start_ms` / `buffer_last_end_ms` hold the first and last
/// token timestamps across the buffer's lifetime. When present (non-zero
/// from Soniox), they become the chunk's `t_start_ms` / `t_end_ms`.
/// Falls back to session-elapsed wall-clock if tokens have zero timestamps.
fn flush_buffer(
    buffer: &mut String,
    buffer_first_start_ms: &mut Option<u64>,
    buffer_last_end_ms: &mut Option<u64>,
    transcript_tx: &broadcast::Sender<TranscriptChunk>,
    events_tx: &broadcast::Sender<crate::contract::Event>,
    session_started: std::time::Instant,
) {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        buffer.clear();
        *buffer_first_start_ms = None;
        *buffer_last_end_ms = None;
        return;
    }
    let elapsed_ms = session_started.elapsed().as_millis() as u64;
    let t_start = buffer_first_start_ms.unwrap_or_else(|| elapsed_ms.saturating_sub(2000));
    let t_end = buffer_last_end_ms.unwrap_or(elapsed_ms);
    // One info log per finalized utterance — operator-meaningful signal that
    // the live pipeline is producing transcripts. Replaces the per-frame
    // PCM/response counters that were too noisy to be useful.
    info!(
        ms = t_end.saturating_sub(t_start),
        text = %trimmed,
        "transcript"
    );
    let committed_text = trimmed.to_string();
    let chunk = TranscriptChunk {
        id: uuid::Uuid::new_v4().to_string(),
        text: committed_text.clone(),
        t_start_ms: t_start,
        t_end_ms: t_end,
        speaker: None,
    };
    let _ = transcript_tx.send(chunk);
    // Surface the committed segment to WS clients so they can
    // append it to a rolling transcript view (the Mac overlay,
    // future PWA reuse). `TranscriptInterim` only carries the
    // mutable preview, so without this clients can't reliably
    // tell when a segment has been finalized.
    let _ = events_tx.send(crate::contract::Event::TranscriptCommitted {
        text: committed_text,
    });
    buffer.clear();
    *buffer_first_start_ms = None;
    *buffer_last_end_ms = None;
}

/// One WS session attempt. Returns Ok on cancellation, Err on protocol failure
/// or WS disconnect. The caller decides whether to retry.
async fn try_one_session(
    cfg: &SonioxStt,
    mut audio_rx: Option<&mut mpsc::Receiver<Vec<u8>>>,
    transcript_tx: &broadcast::Sender<TranscriptChunk>,
    events_tx: &broadcast::Sender<crate::contract::Event>,
    cancel: &CancellationToken,
    session_started: std::time::Instant,
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
    };
    let config_json = serde_json::to_string(&config).map_err(|e| format!("encode config: {e}"))?;
    writer
        .send(Message::Text(config_json))
        .await
        .map_err(|e| format!("send config: {e}"))?;

    info!(model = %cfg.model, "Soniox session opened");
    // Clear any prior reconnecting/unavailable status now that we're connected.
    emit_status_clear(events_tx);

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
    let mut buffer = String::new();
    let mut buffer_first_start_ms: Option<u64> = None;
    let mut buffer_last_end_ms: Option<u64> = None;
    let mut last_token_at: Option<std::time::Instant> = None;
    const MAX_BUFFER_LEN: usize = 240;
    // 3s of no new tokens = a real conversational pause. Sub-3s gaps are
    // mid-sentence (breath, thinking). Combined with the soft-boundary
    // gate below, this prevents the "Hi, h" / "ello, how are you?"
    // fragmentation that 1s + no-gating produced.
    const IDLE_FLUSH_MS: u64 = 3000;
    let mut idle_ticker = tokio::time::interval(Duration::from_millis(500));
    idle_ticker.tick().await; // discard immediate tick

    // 2. Pump loop: forward audio, parse transcripts
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                flush_buffer(&mut buffer, &mut buffer_first_start_ms, &mut buffer_last_end_ms, transcript_tx, events_tx, session_started);
                emit_live(events_tx, "", "");
                let _ = writer.close().await;
                return Ok(());
            }
            // Forward PCM bytes from audio task to Soniox
            pcm = async {
                match audio_rx.as_deref_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending::<Option<Vec<u8>>>().await,
                }
            } => {
                match pcm {
                    Some(bytes) => {
                        if let Err(e) = writer.send(Message::Binary(bytes)).await {
                            return Err(format!("send pcm: {e}"));
                        }
                    }
                    None => {
                        // Audio source ended — close the session
                        flush_buffer(&mut buffer, &mut buffer_first_start_ms, &mut buffer_last_end_ms, transcript_tx, events_tx, session_started);
                        emit_live(events_tx, "", "");
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
                if !buffer.is_empty() {
                    if let Some(t) = last_token_at {
                        let stale = t.elapsed().as_millis() as u64 >= IDLE_FLUSH_MS;
                        if stale && ends_at_soft_boundary(&buffer) {
                            flush_buffer(&mut buffer, &mut buffer_first_start_ms, &mut buffer_last_end_ms, transcript_tx, events_tx, session_started);
                            last_token_at = None;
                            emit_live(events_tx, "", "");
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
                                        if buffer_first_start_ms.is_none() && tok.start_ms > 0 {
                                            buffer_first_start_ms = Some(tok.start_ms);
                                        }
                                        if tok.end_ms > 0 {
                                            buffer_last_end_ms = Some(tok.end_ms);
                                        }
                                        buffer.push_str(&tok.text);
                                        got_final = true;
                                    } else {
                                        interim_text.push_str(&tok.text);
                                    }
                                }
                                if got_final {
                                    last_token_at = Some(std::time::Instant::now());
                                }
                                // Flush on punctuation or length cap. After flush the
                                // buffer is empty and the live row should clear.
                                if ends_with_terminator(&buffer)
                                    || buffer.len() >= MAX_BUFFER_LEN
                                {
                                    flush_buffer(&mut buffer, &mut buffer_first_start_ms, &mut buffer_last_end_ms, transcript_tx, events_tx, session_started);
                                    last_token_at = None;
                                }
                                // Emit a live preview AFTER any flush so the buffer
                                // value reflects post-flush state. The live text is
                                // what the user sees as the "in flight" row in the
                                // PWA's transcript pane.
                                emit_live(events_tx, &buffer, &interim_text);
                            }
                            Err(e) => {
                                warn!(error = %e, raw = %t, "Soniox response parse error");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        flush_buffer(&mut buffer, &mut buffer_first_start_ms, &mut buffer_last_end_ms, transcript_tx, events_tx, session_started);
                        emit_live(events_tx, "", "");
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
        let (events_tx, _events_rx) = broadcast::channel::<crate::contract::Event>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            provider
                .run(None, transcript_tx, events_tx, task_cancel)
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
        let (events_tx, _events_rx) = broadcast::channel::<crate::contract::Event>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            provider
                .run(None, transcript_tx, events_tx, task_cancel)
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
        let (events_tx, _events_rx) = broadcast::channel::<crate::contract::Event>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            provider
                .run(None, transcript_tx, events_tx, task_cancel)
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
        let (events_tx, mut events_rx) = broadcast::channel::<crate::contract::Event>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            provider
                .run(None, transcript_tx, events_tx, task_cancel)
                .await;
        });

        // Drain events; expect at least one TranscriptInterim with our interim text
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut saw_interim = false;
        while tokio::time::Instant::now() < deadline && !saw_interim {
            if let Ok(Ok(evt)) =
                tokio::time::timeout(std::time::Duration::from_millis(200), events_rx.recv()).await
            {
                if let crate::contract::Event::TranscriptInterim { text } = evt {
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
        let (events_tx, mut events_rx) = broadcast::channel::<crate::contract::Event>(16);
        let cancel = CancellationToken::new();
        let provider = Box::new(SonioxStt::new_with_url("test_key".into(), url));
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            provider
                .run(None, transcript_tx, events_tx, task_cancel)
                .await;
        });

        // Wait for a Status event with non-None error
        let mut saw_error = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline && !saw_error {
            if let Ok(Ok(evt)) =
                tokio::time::timeout(std::time::Duration::from_millis(500), events_rx.recv()).await
            {
                if let crate::contract::Event::Status { status } = evt {
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

    #[test]
    fn from_env_reads_model_override() {
        // Save and restore env state
        let prev_model = std::env::var("MEETING_COMPANION_SONIOX_MODEL").ok();
        let prev_key = std::env::var("SONIOX_API_KEY").ok();

        std::env::set_var("SONIOX_API_KEY", "test_key");
        std::env::set_var("MEETING_COMPANION_SONIOX_MODEL", "custom-model");
        let s = SonioxStt::from_env().unwrap();
        assert_eq!(s.model, "custom-model");

        std::env::remove_var("MEETING_COMPANION_SONIOX_MODEL");
        let s = SonioxStt::from_env().unwrap();
        assert_eq!(s.model, MODEL_DEFAULT);

        // Restore
        match prev_model {
            Some(v) => std::env::set_var("MEETING_COMPANION_SONIOX_MODEL", v),
            None => std::env::remove_var("MEETING_COMPANION_SONIOX_MODEL"),
        }
        match prev_key {
            Some(v) => std::env::set_var("SONIOX_API_KEY", v),
            None => std::env::remove_var("SONIOX_API_KEY"),
        }
    }
}
