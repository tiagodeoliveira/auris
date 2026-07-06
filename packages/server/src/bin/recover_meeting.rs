//! Recover lost meetings by replaying JSONL transcripts into Postgres
//! and Mnemo. The live server writes `transcription.jsonl` per meeting
//! during recording; if the `meetings` table gets wiped while those
//! JSONL files survive, this binary backfills them so prior meetings
//! become available for recall in future meetings.
//!
//! What it does, per meeting found:
//!   1. Parse `<root>/<meeting_id>/transcription.jsonl` into items.
//!   2. Resolve --user-id (Auth0 sub) to the internal users.id UUID
//!      (upserting the users row — recreating it after full DB loss),
//!      then INSERT INTO meetings (id, user_id, started_at, ended_at,
//!      description, metadata) — idempotent via ON CONFLICT DO NOTHING.
//!   3. For each transcript item, push a `user`-role sentence event to
//!      Mnemo with `attributes.meeting_id = <id>` — same payload shape
//!      the live `mnemo/pusher.rs` emits during a real meeting, so
//!      recall in subsequent meetings can find these items by id.
//!   4. Write a `_recovered.json` marker so the meeting is skipped on
//!      re-runs (unless `--force`).
//!
//! What it does NOT do:
//!   - Generate highlights / actions / open-questions / summary.
//!   - Run vision analysis on moment screenshots.
//!   - Persist anything to `items` table for non-transcript modes.
//!
//! Required env:
//!   MNEMO_USER_TOKEN     — fresh Mnemo bearer JWT for the user
//!   DATABASE_URL         — Postgres connection (sqlx convention)
//!   AURIS_MNEMO_URL      — Mnemo base URL
//!
//! Optional:
//!   AURIS_MNEMO_WORKSTATION   — defaults to host's name
//!
//! Per-meeting optional sidecar `<root>/<meeting_id>/metadata.json`:
//!   {
//!     "description": "…",
//!     "metadata": {"project": "x", "title": "Y"},
//!     "started_at": "2026-04-01T10:00:00Z"   // overrides file mtime
//!   }
//!
//! Usage:
//!   MNEMO_USER_TOKEN=... DATABASE_URL=... AURIS_MNEMO_URL=... \
//!     cargo run -p auris-server --bin recover-meeting -- \
//!     --user-id 'google-oauth2|123…' --dry-run
//!
//! Without `--dry-run`, the tool runs end-to-end against the live DB
//! and Mnemo. Use `--dry-run` first to confirm the parsed item counts
//! and preview the timestamps before committing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use auris_server::mnemo;
use auris_server::protocol::Item;
use chrono::{DateTime, Utc};
use clap::Parser;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    name = "recover-meeting",
    about = "Replay JSONL transcripts into Postgres + Mnemo to recover lost meetings."
)]
struct Cli {
    /// Auth0 sub of the meeting owner (e.g., google-oauth2|123…).
    /// Resolved to — or, on a wiped DB, recreated as — the internal
    /// `users.id` UUID before any meeting insert: `meetings.user_id`
    /// is a FK onto users(id), never the raw sub.
    #[arg(long, env = "RECOVERY_USER_ID")]
    user_id: String,

    /// Directory containing one subfolder per meeting id, each with a
    /// `transcription.jsonl`. Tilde expansion is supported.
    #[arg(long, default_value = "~/Downloads/meetings")]
    root: PathBuf,

    /// Process only this single meeting id (subfolder name). Without
    /// this flag, every meeting under `--root` is processed.
    #[arg(long)]
    meeting_id: Option<String>,

    /// Print what would happen and exit. No meeting/user writes, no
    /// Mnemo pushes, no `_recovered.json` markers. When DATABASE_URL
    /// is set, the pool is still opened (read-only in spirit; sqlx
    /// migrations run, idempotent on a live DB) to validate that
    /// --user-id resolves to an existing users row.
    #[arg(long)]
    dry_run: bool,

    /// Re-process meetings even if they already carry a
    /// `_recovered.json` marker. Useful when an earlier run errored
    /// partway through.
    #[arg(long)]
    force: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let root = expand_tilde(&cli.root)?;

    let mnemo_token = std::env::var("MNEMO_USER_TOKEN").ok();
    if !cli.dry_run && mnemo_token.is_none() {
        anyhow::bail!("MNEMO_USER_TOKEN env var is required for non-dry-run mode");
    }

    // DB handle: pool + the meeting owner's internal users.id UUID.
    // `meetings.user_id` is a FK onto the server-minted users(id), so
    // the Auth0 sub from --user-id must be resolved before any insert
    // — binding the raw sub FK-fails on every meeting. Non-dry-run
    // upserts (recreating the users row in a full-DB-loss scenario);
    // dry-run only does a read-only lookup so the operator learns up
    // front whether a real run would reuse or mint the owner row.
    let db: Option<(sqlx::PgPool, String)> = if cli.dry_run {
        if std::env::var("DATABASE_URL").is_ok() {
            let pool = auris_server::storage::open_pool()
                .await
                .context("open Postgres pool (dry-run --user-id validation)")?;
            match auris_server::storage::users::find_user_by_auth0_sub(&pool, &cli.user_id)
                .await
                .context("look up --user-id (Auth0 sub) in users table")?
            {
                Some(row) => {
                    info!(auth0_sub = %cli.user_id, internal_id = %row.id, "resolved meeting owner");
                }
                None => {
                    warn!(
                        auth0_sub = %cli.user_id,
                        "WARNING: no users row for this sub — a real run will create one"
                    );
                }
            }
            None // dry-run never writes; the pool was only for validation.
        } else {
            info!("DATABASE_URL not set — skipping --user-id validation in dry-run");
            None
        }
    } else {
        let pool = auris_server::storage::open_pool()
            .await
            .context("open Postgres pool")?;
        let row =
            auris_server::storage::users::upsert_user_by_auth0_sub(&pool, &cli.user_id, None, None)
                .await
                .context("resolve --user-id (Auth0 sub) to internal user id")?;
        info!(auth0_sub = %cli.user_id, internal_id = %row.id, "resolved meeting owner");
        Some((pool, row.id))
    };

    // Mnemo client: token store gets the user's JWT injected so push_event
    // can authenticate without going through the live WS refresh loop.
    let mnemo_tokens = Arc::new(mnemo::token_store::MnemoTokenStore::new());
    if let Some(token) = mnemo_token.as_ref() {
        mnemo_tokens.store(&cli.user_id, token.clone());
    }
    let mnemo_client = mnemo::client::MnemoClient::from_env(mnemo_tokens, None, None);
    if !cli.dry_run && !mnemo_client.is_enabled() {
        anyhow::bail!("Mnemo client is disabled — set AURIS_MNEMO_URL");
    }

    let meetings = collect_meetings(&root, cli.meeting_id.as_deref())?;
    if meetings.is_empty() {
        anyhow::bail!(
            "No meetings with a transcription.jsonl found under {}",
            root.display()
        );
    }
    info!(
        count = meetings.len(),
        root = %root.display(),
        dry_run = cli.dry_run,
        "recovery plan",
    );

    let mut successes = 0usize;
    let mut skipped = 0usize;
    let mut failures = 0usize;
    for meeting in &meetings {
        let marker = meeting.dir.join("_recovered.json");
        if marker.exists() && !cli.force {
            info!(meeting = %meeting.id, "skipping — _recovered.json present (use --force to re-run)");
            skipped += 1;
            continue;
        }
        match recover_one(
            meeting,
            &cli.user_id,
            db.as_ref().map(|(pool, uid)| (pool, uid.as_str())),
            &mnemo_client,
            cli.dry_run,
        )
        .await
        {
            Ok(pushed) => {
                info!(meeting = %meeting.id, items = pushed, "recovery complete");
                if !cli.dry_run {
                    if let Err(e) = write_marker(&marker, pushed) {
                        warn!(meeting = %meeting.id, error = %e, "could not write _recovered.json");
                    }
                }
                successes += 1;
            }
            Err(e) => {
                warn!(meeting = %meeting.id, error = %e, "recovery failed");
                failures += 1;
            }
        }
    }
    info!(successes, skipped, failures, dry_run = cli.dry_run, "done",);
    Ok(())
}

#[derive(Debug)]
struct MeetingPlan {
    id: String,
    dir: PathBuf,
    transcript_path: PathBuf,
}

/// Discover every `<root>/<meeting_id>/transcription.jsonl` candidate.
/// If `only` is set, restrict the result to that single id. The
/// folder name IS the meeting id (matches the live server's
/// `<DATA_DIR>/blobs/meetings/<id>/` layout).
fn collect_meetings(root: &Path, only: Option<&str>) -> Result<Vec<MeetingPlan>> {
    let mut out = Vec::new();
    let entries =
        std::fs::read_dir(root).with_context(|| format!("read_dir {}", root.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip hidden / dotfiles + our own marker dirs in case someone
        // points the tool at a place containing mixed content.
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        if let Some(filter) = only {
            if name != filter {
                continue;
            }
        }
        let transcript = path.join("transcription.jsonl");
        if !transcript.exists() {
            continue;
        }
        out.push(MeetingPlan {
            id: name.to_string(),
            dir: path,
            transcript_path: transcript,
        });
    }
    // Stable order (by id) so dry-run + commit show the same sequence.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Optional per-meeting overrides. Drop a `metadata.json` next to
/// `transcription.jsonl` to set description / metadata / started_at.
/// All fields optional; absent ones fall back to derived defaults.
#[derive(serde::Deserialize, Default, Debug)]
struct MetadataSidecar {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
    /// Override for the meetings row's `started_at`. Without it, we
    /// fall back to the transcript file's mtime (which is the END of
    /// the meeting — best-effort but visibly imprecise).
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
}

/// `mnemo_user_key` is the raw --user-id string, used ONLY as the
/// MnemoTokenStore key (main() stored MNEMO_USER_TOKEN under it; the
/// identity Mnemo sees is the bearer JWT itself). `db` carries the
/// pool plus the RESOLVED internal users.id UUID for the meetings FK.
async fn recover_one(
    meeting: &MeetingPlan,
    mnemo_user_key: &str,
    db: Option<(&sqlx::PgPool, &str)>,
    mnemo_client: &mnemo::client::MnemoClient,
    dry_run: bool,
) -> Result<usize> {
    // Parse the JSONL into Items. Skip blank lines + lines that fail
    // to deserialize so a single bad line doesn't doom the whole
    // meeting — matches `persistence::read_transcription`'s leniency.
    let content = std::fs::read_to_string(&meeting.transcript_path)
        .with_context(|| format!("read {}", meeting.transcript_path.display()))?;
    let items: Vec<Item> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Item>(l).ok())
        .collect();
    if items.is_empty() {
        anyhow::bail!("transcript file has no parseable items");
    }

    // Load optional sidecar for description / metadata / started_at.
    let sidecar_path = meeting.dir.join("metadata.json");
    let sidecar: MetadataSidecar = if sidecar_path.exists() {
        let raw = std::fs::read_to_string(&sidecar_path)
            .with_context(|| format!("read {}", sidecar_path.display()))?;
        serde_json::from_str(&raw).context("parse metadata.json sidecar")?
    } else {
        MetadataSidecar::default()
    };

    // Timestamps. Without explicit sidecar.started_at we fall back to
    // the transcript file's mtime — which is the END of the meeting.
    // Setting both started_at and ended_at to mtime gives a 0-duration
    // row; user can refine via the sidecar if they care.
    let mtime: DateTime<Utc> = {
        let meta = std::fs::metadata(&meeting.transcript_path)?;
        DateTime::<Utc>::from(meta.modified()?)
    };
    let started_at = sidecar.started_at.unwrap_or(mtime);
    let ended_at = mtime;

    if dry_run {
        println!(
            "[DRY-RUN] meeting={} items={} started_at={} ended_at={} metadata_keys={}",
            meeting.id,
            items.len(),
            started_at,
            ended_at,
            sidecar.metadata.len(),
        );
        for (i, item) in items.iter().take(3).enumerate() {
            let preview: String = item.text.chars().take(80).collect();
            println!("  item[{i}]: {preview}");
        }
        if items.len() > 3 {
            println!("  ... + {} more items", items.len() - 3);
        }
        return Ok(items.len());
    }

    // DB insert — idempotent (ON CONFLICT (id) DO NOTHING inside the
    // helper, so re-runs land cleanly). `db_user_id` is the internal
    // users.id UUID resolved in main(), NOT the Auth0 sub — the
    // meetings.user_id FK rejects raw subs.
    if let Some((pool, db_user_id)) = db {
        let metadata_json = serde_json::to_string(&sidecar.metadata)
            .context("serialize sidecar metadata to JSON")?;
        auris_server::storage::meetings::insert_recovered_meeting(
            pool,
            &meeting.id,
            db_user_id,
            started_at,
            ended_at,
            sidecar.description.as_deref(),
            &metadata_json,
        )
        .await
        .context("insert meetings row")?;
    }

    // Mnemo push: one fresh session uuid per meeting; one sentence
    // event per transcript item. Mirrors `mnemo/pusher.rs` so recall
    // in future meetings finds these as if they'd been live-streamed.
    let session_id = Uuid::new_v4().to_string();
    let workstation = mnemo_client.workstation().to_string();
    for item in &items {
        let content = format_transcript_content(item);
        let event = mnemo::payload::build_sentence_event(
            &session_id,
            &workstation,
            &sidecar.metadata,
            Some(meeting.id.as_str()),
            &content,
        );
        mnemo_client
            .push_event(mnemo_user_key, &event)
            .await
            .context("mnemo push_event failed")?;
    }

    Ok(items.len())
}

/// Mirror `mnemo::pusher::format_transcript_content` so the recovery
/// items shape identically to live-pushed items.
fn format_transcript_content(item: &Item) -> String {
    let speaker = item
        .meta
        .as_ref()
        .and_then(|m| m.get("speaker"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    match speaker {
        Some(s) => format!("[Speaker {s}] {}", item.text),
        None => item.text.clone(),
    }
}

fn write_marker(path: &Path, items_pushed: usize) -> Result<()> {
    let marker = serde_json::json!({
        "recovered_at": Utc::now().to_rfc3339(),
        "items_pushed": items_pushed,
    });
    std::fs::write(path, serde_json::to_string_pretty(&marker)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Expand a leading `~/` against the `HOME` env var. clap doesn't
/// do this for us, and `PathBuf` parses `~/Downloads/...` literally
/// (treats `~` as a directory name).
fn expand_tilde(p: &Path) -> Result<PathBuf> {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME env var not set")?;
        Ok(PathBuf::from(home).join(rest))
    } else if s == "~" {
        let home = std::env::var("HOME").context("HOME env var not set")?;
        Ok(PathBuf::from(home))
    } else {
        Ok(p.to_path_buf())
    }
}
