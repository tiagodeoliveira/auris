//! Application composition root, extracted from `ws/mod.rs` so the
//! transport module stops doubling as the boot sequence.
//!
//! Boot ORDER here is load-bearing:
//!   1. Postgres pool
//!   2. boot recovery + orphaned-wrap-up sweep (query meetings BEFORE
//!      any subscriber spins up)
//!   3. mnemo client + the single durable-writer task + the two-lane
//!      `EventBus` + the mnemo recaller/drain
//!   4. `ServerHandle` construction
//!   5. background workers (moment / artifact / wrap-up retry + boot
//!      re-kick / orphan-blob sweep) + liveness reaper
//!   6. recovered-pipeline restarts
//!   7. heartbeat task
//!   8. `axum::serve` on the router built by `crate::ws::make_app_router`
//!   9. shutdown: cancel, 2 s drain, then bounded `TaskTracker` wait
//!
//! Change the order only with the shutdown / heartbeat / live-pipeline
//! integration tests green.

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::auth::AuthMode;
use crate::context::{EventBus, ServerHandle};
use crate::llm::LlmClient;
use crate::ws::control;

pub async fn run_server(
    addr: SocketAddr,
    auth: AuthMode,
    chat_llm: Arc<LlmClient>,
    background_llm: Arc<LlmClient>,
    breaker_metrics: Arc<crate::observability::BreakerMetrics>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = ?listener.local_addr()?, "listening");
    run_server_with_listener(
        listener,
        auth,
        chat_llm,
        background_llm,
        breaker_metrics,
        shutdown_rx,
        None,
    )
    .await
}

/// `handle_tx`: when `Some`, receives a clone of the constructed
/// `ServerHandle` once boot wiring is complete — integration tests use
/// this to reach internals like the finalize `TaskTracker`. Production
/// (`run_server`) passes `None`.
pub async fn run_server_with_listener(
    listener: TcpListener,
    auth: AuthMode,
    chat_llm: Arc<LlmClient>,
    background_llm: Arc<LlmClient>,
    breaker_metrics: Arc<crate::observability::BreakerMetrics>,
    mut shutdown_rx: oneshot::Receiver<()>,
    handle_tx: Option<oneshot::Sender<ServerHandle>>,
) -> Result<()> {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex;

    let (fanout_tx, _) =
        broadcast::channel::<crate::protocol::UserEvent>(crate::context::bus::FANOUT_CAPACITY);
    let shutdown = CancellationToken::new();
    let state = Arc::new(Mutex::new(crate::session::SessionRegistry::new()));

    // DB first — boot recovery wants to query meetings before any
    // subscribers spin up.
    let db = crate::storage::open_pool().await?;

    // Boot recovery. If the previous run died with a meeting still
    // active (ended_at IS NULL in the meetings table), pick it up,
    // replay the persisted transcript items, and resume as if the
    // meeting had never been interrupted.
    let recovered = control::recover_active_meetings(&db, &state).await;

    // Boot sweep for the opposite shape: meetings that ENDED but were
    // restart-orphaned mid-finalize still read wrap_up_status='running'
    // forever (detached finalize/retry tasks died with the old process;
    // the retry endpoint rejects 'running'). Mark them 'failed' so the
    // clients' failed-banner + retry paths apply. Runs strictly before
    // the listener serves traffic, so it cannot race a fresh finalize.
    control::sweep_orphaned_wrap_ups(&db).await;

    // Memory layer: spin up the ingestion pusher and the start-of-meeting
    // recaller. Each gets its own broadcast subscription. No-op if mnemo
    // env vars are not set.
    //
    // Construct once + clone for each consumer: pusher, recaller, and
    // the ServerHandle (so the agent's per-meeting fetch tools can
    // recall against attached meetings). MnemoClient is `Clone` and
    // cheap — reqwest::Client clones share the underlying pool.
    // Token store is the bridge between the WS intent handler (where
    // the UI deposits per-user mnemo JWTs) and the mnemo HTTP client
    // (where those JWTs become Bearer headers). Shared via Arc.
    let mnemo_tokens = Arc::new(crate::mnemo::MnemoTokenStore::new());
    breaker_metrics.register("mnemo.push");
    breaker_metrics.register("mnemo.recall");
    let push_breaker = Arc::new(crate::util::circuit_breaker::CircuitBreaker::new(
        "mnemo.push",
        5,
        Duration::from_secs(30),
        Some(Box::new(crate::observability::MetricsObs(
            breaker_metrics.clone(),
        ))),
    ));
    let recall_breaker = Arc::new(crate::util::circuit_breaker::CircuitBreaker::new(
        "mnemo.recall",
        5,
        Duration::from_secs(30),
        Some(Box::new(crate::observability::MetricsObs(
            breaker_metrics.clone(),
        ))),
    ));
    let mnemo_client =
        crate::mnemo::MnemoClient::from_env(mnemo_tokens, Some(push_breaker), Some(recall_breaker));
    // Durable pipeline: ONE writer task owns every durable side-effect
    // (transcript JSONL, items table, mnemo push), fed by the bounded
    // mpsc inside `EventBus::emit` — backpressure, never loss. The
    // mnemo recaller stays on the lossy fan-out lane: a missed recall
    // degrades one prompt, it is not data loss.
    let durable_tx = crate::storage::persistence_loop::spawn_durable_writer(
        state.clone(),
        db.clone(),
        mnemo_client.clone(),
    );
    let bus = EventBus::new(fanout_tx, durable_tx);
    crate::mnemo::spawn_recaller_and_drain(
        mnemo_client.clone(),
        state.clone(),
        &bus.fanout,
        shutdown.clone(),
    );
    let (moment_created_tx, _) = broadcast::channel::<crate::api::MomentCreated>(64);
    let (artifact_created_tx, _) = broadcast::channel::<crate::api::ArtifactCreated>(64);
    let (wrap_up_retry_tx, _) = broadcast::channel::<crate::api::WrapUpRetry>(32);
    let (agent_kick_tx, _) = broadcast::channel::<crate::agent::AgentKick>(32);
    let handle = ServerHandle {
        sessions: state,
        bus: bus.clone(),
        direct_tx: Arc::new(StdMutex::new(HashMap::new())),
        auth: Arc::new(auth),
        shutdown: shutdown.clone(),
        chat_llm,
        background_llm,
        db,
        moment_created_tx,
        artifact_created_tx,
        wrap_up_retry_tx,
        agent_kick_tx,
        mnemo: mnemo_client,
        tasks: tokio_util::task::TaskTracker::new(),
    };

    // Hand a handle clone to the observer, if any (integration tests).
    if let Some(tx) = handle_tx {
        let _ = tx.send(handle.clone());
    }

    // Async LLM worker that fills in moment summaries. Subscribes
    // to `moment_created_tx`; reads transcript JSONL ±N seconds
    // around each moment, prompts the LLM, writes summary back to
    // SQLite. See `summarizer/moment.rs`.
    crate::workers::moment::spawn_worker(handle.clone());

    // Sibling worker for artifact uploads. Subscribes to
    // `artifact_created_tx`; reads bytes off disk, asks the LLM for
    // short + long summaries in one call, writes them back into
    // `artifacts.short_summary` / `long_summary` and flips
    // `summary_status` to `done` (or `failed` on terminal error).
    // See `summarizer/artifact.rs`.
    crate::workers::artifact::spawn_worker(handle.clone());

    // Wrap-up retry worker. Subscribes to `wrap_up_retry_tx`; for
    // each retry signal it reads the meeting's persisted transcript
    // blob and re-runs the actions/open_questions extractor that the
    // live finalize path runs. Spawned alongside the artifact worker
    // — both are long-lived, single-subscriber background tasks.
    crate::workers::wrap_up::spawn_retry_worker(handle.clone());

    // Boot re-kick for wrap-ups a previous process killed mid-finalize
    // (`ended_at` set, `wrap_up_status` stuck at 'running' — the HTTP
    // retry endpoint rejects that state, so this scan is the only way
    // those meetings ever recover their summary). Must run AFTER
    // `spawn_retry_worker`: the worker subscribed synchronously above,
    // so these sends are guaranteed a receiver. Same test escape hatch
    // as `recover_active_meetings` — integration tests share a DB and
    // would re-kick each other's leftovers.
    if !crate::config::flag("AURIS_SKIP_BOOT_RECOVERY") {
        let kicked =
            crate::workers::wrap_up::rekick_interrupted(&handle.db, &handle.wrap_up_retry_tx).await;
        if kicked > 0 {
            info!(kicked, "re-kicked interrupted wrap-ups at boot");
        }
    }

    // Orphan-blob reconciliation sweep. Daily pass that reaps blobs
    // whose DB row is gone (and reports rows whose blob is gone) —
    // the cleanup task the artifact/meeting/moment delete paths
    // promise in their doc comments. First run is delayed ~10 min
    // after boot so it never competes with meeting recovery; knobs
    // via AURIS_SWEEP_* env vars. See workers/sweep.rs.
    crate::workers::sweep::spawn_worker(handle.clone());

    // Liveness reaper: ends meetings whose audio source has been gone
    // past the grace window (default 15 min) — the safety net for a
    // client that dies mid-meeting and never returns, so a meeting
    // never sits Active forever. A returning client reconnects /audio,
    // which clears the timer before this fires.
    control::spawn_liveness_reaper(handle.clone());

    // For each recovered user-meeting pair: emit a synthetic
    // `MeetingStateChanged Active` (tagged to that user so only their
    // connections receive it) and spin up that user's live pipeline.
    for r in recovered {
        handle
            .bus
            .emit(
                r.user_id.clone(),
                crate::protocol::Event::MeetingStateChanged {
                    meeting_state: crate::protocol::MeetingState::Active,
                    meeting_id: Some(r.meeting_id.clone()),
                },
            )
            .await;
        // The recovered meeting's runtime was created by
        // `rehydrate_from_recovered_meeting`, which calls
        // `MeetingRuntime::new`. The cancel token is already on the
        // runtime — fetch a clone; lock released before the await.
        let token = {
            let sessions = handle.sessions.lock().await;
            sessions
                .meeting_cancel_token(&r.user_id)
                .expect("rehydrate installs MeetingRuntime with cancel token")
        };
        control::spawn_live_pipeline(handle.clone(), r.user_id.clone(), token.child_token()).await;
        // Seed the liveness timer: a recovered meeting has no /audio
        // client this process. If one reconnects it clears the timer
        // (mark_audio_connected); if none does within the grace window,
        // the reaper ends this zombie rather than leaving it Active
        // forever after a server restart.
        handle.sessions.lock().await.seed_audio_loss(&r.user_id);
        info!(
            user_id = %r.user_id,
            meeting_id = %r.meeting_id,
            "live pipeline restarted for recovered meeting"
        );
    }

    // Periodic Status broadcast — one event per connected user so
    // each only sees their own listening state. Users without an
    // active meeting still receive an idle Status so the wsStatus
    // dot updates promptly.
    let hb_handle = handle.clone();
    let hb_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hb_shutdown_clone = hb_shutdown.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(control::heartbeat_interval());
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            if hb_shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let snapshots = {
                let s = hb_handle.sessions.lock().await;
                s.user_ids()
                    .into_iter()
                    .map(|uid| {
                        let listening = s
                            .user(&uid)
                            .map(|u| {
                                matches!(u.meeting_state, crate::protocol::MeetingState::Active)
                            })
                            .unwrap_or(false);
                        (uid, listening)
                    })
                    .collect::<Vec<_>>()
            };
            for (uid, listening) in snapshots {
                hb_handle.bus.emit_fanout_only(
                    uid,
                    crate::protocol::Event::Status {
                        status: crate::protocol::Status {
                            listening,
                            error: None,
                        },
                    },
                );
            }
        }
    });

    // Single axum app: WebSocket (/, /audio) + REST (/meetings…)
    // share one listener. Connection-info (peer address) is wired
    // via `into_make_service_with_connect_info`.
    let app = crate::ws::make_app_router(handle.clone());
    let serve_shutdown = shutdown.clone();
    let serve_handle = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { serve_shutdown.cancelled().await })
        .await;
        if let Err(e) = result {
            tracing::warn!(error = ?e, "axum::serve stopped with error");
        }
    });

    // Park until the shutdown signal arrives, then unwind.
    let _ = (&mut shutdown_rx).await;
    info!("shutdown received");
    shutdown.cancel(); // signal all per-connection tasks to close
    hb_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(2)).await; // 2 s drain
    serve_handle.abort();
    // Bounded wait for detached-but-finite background work (post-stop
    // finalize, wrap-up-retry fan-outs). Without this, a redeploy
    // seconds after StopMeeting killed the summary mid-flight — the
    // >=6 s STT drain plus three LLM calls far exceed the 2 s drain
    // above — and left wrap_up_status stuck at 'running', which the
    // retry endpoint rejects. Empty tracker → wait() returns
    // immediately, so deploys with nothing in flight stay at ~2 s.
    // Tasks spawned after close() are still tracked and awaited, so a
    // stop racing the shutdown signal is covered rather than leaked.
    handle.tasks.close();
    let grace = Duration::from_millis(crate::config::var_u64_or("AURIS_SHUTDOWN_GRACE_MS", 20_000));
    if tokio::time::timeout(grace, handle.tasks.wait())
        .await
        .is_err()
    {
        tracing::warn!(
            grace_ms = grace.as_millis() as u64,
            "finalize task(s) still running at shutdown deadline; boot re-kick will recover"
        );
    }
    Ok(())
}
