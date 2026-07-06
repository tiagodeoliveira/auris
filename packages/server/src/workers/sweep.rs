//! Orphan-blob reconciliation sweep.
//!
//! Every fs+DB write path in the API is deliberately non-atomic
//! (blob written before the row insert; row deleted before the
//! best-effort blob removal) and resolves failures to "leave the
//! orphan; a future cleanup task will reap it" — see the doc
//! comments on `api::artifacts::upload_artifact`,
//! `api::meetings::delete_meeting`, and `api::moments::delete_moment`.
//! This module IS that cleanup task.
//!
//! Because every blob path embeds its row's primary key
//! (`blobs/artifacts/<uid>/<aid>`, `blobs/meetings/<mid>/
//! {screenshots,chat}/<id>.png`), reconciliation needs no manifest:
//! a blob is live iff a row with that ID exists.
//!
//! Safety properties (all tested below):
//! - Filesystem is listed FIRST, DB IDs are snapshotted SECOND — a
//!   row inserted mid-scan is always in the snapshot for any file
//!   already listed, so a freshly-inserted row can never look
//!   orphaned.
//! - A candidate is only reaped when its mtime is older than the
//!   grace window (default 1 h) — the upload write→insert gap is
//!   milliseconds, so in-flight uploads always survive.
//! - Abort guard: if the would-reap set exceeds both
//!   `ABORT_MIN_FILES` files and 50% of everything scanned, the run
//!   deletes nothing — this catches an `AURIS_DATA_DIR` mismatch or
//!   a DB-connection-to-wrong-instance scenario where *everything*
//!   looks orphaned.
//! - A screenshot whose moment row exists with a NULL `asset_path`
//!   (the upload's UPDATE failed after the PNG landed) is HEALED,
//!   never reaped.
//! - The inverse direction (row exists, blob gone) is report-only:
//!   the DB row is the declared source of truth.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use crate::context::ServerHandle;

/// Abort guard: never reap when the would-reap set is larger than
/// this many files AND more than half of everything scanned.
const ABORT_MIN_FILES: usize = 10;

/// Per-run outcome, logged in full by the worker loop — this is the
/// observability surface for fs/DB divergence.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub orphan_artifact_blobs: usize,
    pub orphan_meeting_dirs: usize,
    pub orphan_screenshots: usize,
    pub orphan_chat_files: usize,
    /// Screenshots whose moment row had a NULL `asset_path` that the
    /// sweep back-filled (orphan path "upload wrote PNG, UPDATE failed").
    pub healed_asset_paths: usize,
    /// Inverse check: rows whose referenced blob is absent. Report-only.
    pub rows_missing_blob: usize,
    /// Safety guard tripped — nothing was deleted this run.
    pub aborted: bool,
}

/// Worker knobs, resolved from env once at spawn. Pulled into a
/// struct so the parsing is unit-testable without spawning anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepConfig {
    /// Delay before the first run (default 10 min) — keeps the sweep
    /// from competing with boot-time meeting recovery for the pool.
    pub initial_delay: Duration,
    /// Time between runs (default 24 h).
    pub interval: Duration,
    /// Minimum age before an unmatched blob is reapable (default 1 h).
    pub grace: Duration,
    /// Report-only mode: classify and log, delete nothing.
    pub dry_run: bool,
}

impl SweepConfig {
    pub fn from_env() -> Self {
        Self {
            initial_delay: Duration::from_secs(crate::config::var_u64_or(
                "AURIS_SWEEP_INITIAL_DELAY_SECS",
                600,
            )),
            interval: Duration::from_secs(crate::config::var_u64_or(
                "AURIS_SWEEP_INTERVAL_SECS",
                86_400,
            )),
            grace: Duration::from_secs(crate::config::var_u64_or("AURIS_SWEEP_GRACE_SECS", 3_600)),
            dry_run: crate::config::flag("AURIS_SWEEP_DRY_RUN"),
        }
    }
}

/// Spawn the sweep worker. Mirrors `workers::wrap_up::spawn_retry_worker`:
/// one long-lived task for the server lifetime, exits on the
/// server-wide shutdown token. Returns immediately.
pub fn spawn_worker(handle: ServerHandle) {
    let db = handle.db.clone();
    let shutdown = handle.shutdown.clone();
    tokio::spawn(async move {
        let cfg = SweepConfig::from_env();
        info!(?cfg, "orphan-blob sweep worker started");
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(cfg.initial_delay) => {}
        }
        let mut interval = tokio::time::interval(cfg.interval);
        // First tick fires immediately (i.e. right after initial_delay);
        // subsequent ticks every `interval`. Delay, don't burst, after
        // a suspended laptop catches up.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {
                    // Resolve the data dir per run through the same
                    // `storage::data_dir()` every writer uses, so the
                    // sweep can never diverge from the write paths.
                    let data_dir = match crate::storage::data_dir() {
                        Ok(d) => d,
                        Err(e) => {
                            warn!(error = ?e, "sweep: data_dir unresolved; skipping run");
                            continue;
                        }
                    };
                    match run_sweep(&db, &data_dir, cfg.grace, cfg.dry_run).await {
                        Ok(report) => {
                            // The full report at info level IS the
                            // observability metric for this feature.
                            info!(?report, dry_run = cfg.dry_run, "orphan-blob sweep complete");
                        }
                        Err(e) => warn!(error = ?e, "orphan-blob sweep failed"),
                    }
                }
            }
        }
    });
}

/// Reconcile `<data_dir>/blobs/**` against Postgres. Injectable for
/// tests: `data_dir` is a parameter (do NOT read `AURIS_DATA_DIR`
/// here — the env var is process-global and would race concurrent
/// tests). `dry_run` reports candidates without mutating anything.
pub async fn run_sweep(
    db: &sqlx::PgPool,
    data_dir: &Path,
    grace: Duration,
    dry_run: bool,
) -> Result<SweepReport> {
    let now = SystemTime::now();

    // ---- Phase 1: list the filesystem FIRST. Any file captured here
    // was written before the DB snapshot below, so its row (if the
    // insert succeeded) is guaranteed to be in the snapshot.
    let artifact_files = list_artifact_blobs(data_dir).await?;
    let meeting_dirs = list_meeting_dirs(data_dir).await?;

    // ---- Phase 2: snapshot DB ids AFTER the fs listing.
    let artifact_rows: Vec<(String, String, String)> =
        sqlx::query_as(r#"SELECT user_id, id, asset_path FROM artifacts"#)
            .fetch_all(db)
            .await
            .context("sweep: snapshot artifacts")?;
    let artifact_keys: HashSet<(String, String)> = artifact_rows
        .iter()
        .map(|(uid, id, _)| (uid.clone(), id.clone()))
        .collect();
    let meeting_ids: HashSet<String> = sqlx::query_scalar(r#"SELECT id FROM meetings"#)
        .fetch_all(db)
        .await
        .context("sweep: snapshot meetings")?
        .into_iter()
        .collect::<HashSet<String>>();
    let moment_rows: Vec<(String, Option<String>)> =
        sqlx::query_as(r#"SELECT id, asset_path FROM moments"#)
            .fetch_all(db)
            .await
            .context("sweep: snapshot moments")?;
    let moments: HashMap<String, Option<String>> = moment_rows.into_iter().collect();
    let chat_rows: Vec<(String, String)> =
        sqlx::query_as(r#"SELECT id, bytes_path FROM chat_attachments"#)
            .fetch_all(db)
            .await
            .context("sweep: snapshot chat_attachments")?;
    let chat_ids: HashSet<String> = chat_rows.iter().map(|(id, _)| id.clone()).collect();

    // ---- Phase 3: classify. Matching is strictly by primary key
    // embedded in the path — no heuristics.
    let mut scanned = 0usize;
    let mut orphan_artifacts: Vec<PathBuf> = Vec::new();
    let mut orphan_dirs: Vec<PathBuf> = Vec::new();
    let mut orphan_screens: Vec<PathBuf> = Vec::new();
    let mut orphan_chat: Vec<PathBuf> = Vec::new();
    let mut heals: Vec<(String, String)> = Vec::new(); // (moment_id, rel_path)

    for blob in &artifact_files {
        scanned += 1;
        let key = (blob.user_id.clone(), blob.artifact_id.clone());
        if !artifact_keys.contains(&key) && file_past_grace(&blob.path, now, grace).await {
            orphan_artifacts.push(blob.path.clone());
        }
    }

    for md in &meeting_dirs {
        if meeting_ids.contains(&md.meeting_id) {
            // Live meeting: reconcile only the two ID-keyed subtrees;
            // everything else in the dir (transcription.jsonl) belongs
            // to the live meeting and is kept.
            for f in list_files(&md.path.join("screenshots")).await? {
                scanned += 1;
                let stem = file_stem(&f);
                match moments.get(&stem) {
                    // Row exists and recorded a path — live.
                    Some(Some(_)) => {}
                    // Row exists but `asset_path` never landed (the
                    // upload's UPDATE failed): heal, never reap.
                    Some(None) => {
                        if let Some(rel) = rel_path(data_dir, &f) {
                            heals.push((stem, rel));
                        }
                    }
                    None => {
                        if file_past_grace(&f, now, grace).await {
                            orphan_screens.push(f);
                        }
                    }
                }
            }
            for f in list_files(&md.path.join("chat")).await? {
                scanned += 1;
                let stem = file_stem(&f);
                if !chat_ids.contains(&stem) && file_past_grace(&f, now, grace).await {
                    orphan_chat.push(f);
                }
            }
        } else {
            scanned += 1;
            if dir_past_grace(&md.path, now, grace).await {
                orphan_dirs.push(md.path.clone());
            }
        }
    }

    let mut report = SweepReport {
        orphan_artifact_blobs: orphan_artifacts.len(),
        orphan_meeting_dirs: orphan_dirs.len(),
        orphan_screenshots: orphan_screens.len(),
        orphan_chat_files: orphan_chat.len(),
        healed_asset_paths: heals.len(),
        rows_missing_blob: 0,
        aborted: false,
    };

    // ---- Safety guard: a mostly-orphaned tree means the sweep is
    // probably looking at the wrong data dir or the wrong database.
    let would_reap =
        orphan_artifacts.len() + orphan_dirs.len() + orphan_screens.len() + orphan_chat.len();
    if would_reap > ABORT_MIN_FILES && would_reap * 2 > scanned {
        report.aborted = true;
        error!(
            would_reap,
            scanned,
            "sweep: would reap >50% of scanned files — aborting without deleting \
             (check AURIS_DATA_DIR / DATABASE_URL pairing)"
        );
        return Ok(report);
    }

    // ---- Phase 4: reap + heal (entirely skipped in dry-run).
    if !dry_run {
        for p in &orphan_artifacts {
            reap_file(p).await;
        }
        for p in &orphan_dirs {
            match tokio::fs::remove_dir_all(p).await {
                Ok(()) => info!(path = %p.display(), "sweep: reaped orphan meeting dir"),
                Err(e) => warn!(error = ?e, path = %p.display(), "sweep: remove_dir_all failed"),
            }
        }
        for p in orphan_screens.iter().chain(orphan_chat.iter()) {
            reap_file(p).await;
        }
        for (moment_id, rel) in &heals {
            // Warn-level on purpose: a healed NULL asset_path means
            // the original upload's UPDATE failed — keep it visible.
            warn!(
                moment_id = %moment_id,
                rel = %rel,
                "sweep: healing moment asset_path that never landed"
            );
            if let Err(e) =
                crate::storage::moments::update_moment_asset_path(db, moment_id, rel).await
            {
                warn!(error = ?e, moment_id = %moment_id, "sweep: heal failed");
            }
        }
    }

    // ---- Phase 5: inverse check — rows whose blob vanished.
    // Report-only: the DB row is the declared source of truth.
    let mut missing_artifacts = 0usize;
    for (_uid, id, asset_path) in &artifact_rows {
        if !blob_exists(data_dir, asset_path).await {
            warn!(artifact_id = %id, asset_path = %asset_path, "sweep: artifact row has no blob on disk");
            missing_artifacts += 1;
        }
    }
    let mut missing_screens = 0usize;
    for (id, asset_path) in &moments {
        if let Some(rel) = asset_path {
            if !blob_exists(data_dir, rel).await {
                warn!(moment_id = %id, asset_path = %rel, "sweep: moment row has no screenshot on disk");
                missing_screens += 1;
            }
        }
    }
    let mut missing_chat = 0usize;
    for (id, bytes_path) in &chat_rows {
        if !blob_exists(data_dir, bytes_path).await {
            warn!(attachment_id = %id, bytes_path = %bytes_path, "sweep: chat_attachment row has no blob on disk");
            missing_chat += 1;
        }
    }
    report.rows_missing_blob = missing_artifacts + missing_screens + missing_chat;

    Ok(report)
}

/// One artifact blob on disk: `blobs/artifacts/<user_id>/<artifact_id>`.
struct ArtifactBlob {
    user_id: String,
    artifact_id: String,
    path: PathBuf,
}

struct MeetingDir {
    meeting_id: String,
    path: PathBuf,
}

/// Enumerate `blobs/artifacts/<uid>/<aid>` files. A missing tree
/// (fresh install, empty data dir) is an empty list, not an error.
async fn list_artifact_blobs(data_dir: &Path) -> Result<Vec<ArtifactBlob>> {
    let root = data_dir.join("blobs").join("artifacts");
    let mut out = Vec::new();
    let Some(mut users) = read_dir_opt(&root).await? else {
        return Ok(out);
    };
    while let Some(user_entry) = users
        .next_entry()
        .await
        .context("sweep: read artifacts dir")?
    {
        if !user_entry
            .file_type()
            .await
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let user_id = user_entry.file_name().to_string_lossy().into_owned();
        let Some(mut files) = read_dir_opt(&user_entry.path()).await? else {
            continue;
        };
        while let Some(f) = files
            .next_entry()
            .await
            .context("sweep: read user artifacts")?
        {
            if f.file_type().await.map(|t| t.is_dir()).unwrap_or(true) {
                continue;
            }
            out.push(ArtifactBlob {
                user_id: user_id.clone(),
                artifact_id: f.file_name().to_string_lossy().into_owned(),
                path: f.path(),
            });
        }
    }
    Ok(out)
}

/// Enumerate `blobs/meetings/<meeting_id>/` directories.
async fn list_meeting_dirs(data_dir: &Path) -> Result<Vec<MeetingDir>> {
    let root = data_dir.join("blobs").join("meetings");
    let mut out = Vec::new();
    let Some(mut entries) = read_dir_opt(&root).await? else {
        return Ok(out);
    };
    while let Some(e) = entries
        .next_entry()
        .await
        .context("sweep: read meetings dir")?
    {
        if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        out.push(MeetingDir {
            meeting_id: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
        });
    }
    Ok(out)
}

/// Regular files directly inside `dir`; empty when the dir is absent.
async fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let Some(mut entries) = read_dir_opt(dir).await? else {
        return Ok(out);
    };
    while let Some(e) = entries.next_entry().await.context("sweep: read subdir")? {
        if e.file_type().await.map(|t| t.is_dir()).unwrap_or(true) {
            continue;
        }
        out.push(e.path());
    }
    Ok(out)
}

/// `read_dir` that maps NotFound to `None` instead of erroring.
async fn read_dir_opt(dir: &Path) -> Result<Option<tokio::fs::ReadDir>> {
    match tokio::fs::read_dir(dir).await {
        Ok(rd) => Ok(Some(rd)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("sweep: read_dir {}", dir.display())),
    }
}

/// `<id>.png` → `<id>`; extensionless artifact blobs → the full name.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Path relative to `data_dir`, as stored in the DB path columns.
fn rel_path(data_dir: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(data_dir)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// True when the file's mtime is older than `grace`. Unreadable
/// metadata fails safe (keep the file).
async fn file_past_grace(path: &Path, now: SystemTime, grace: Duration) -> bool {
    match tokio::fs::metadata(path).await.and_then(|m| m.modified()) {
        Ok(mtime) => now
            .duration_since(mtime)
            .map(|age| age > grace)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Grace check for a whole meeting dir: the newest *file* mtime
/// anywhere under it must be past grace. Directory mtimes are
/// excluded (`create_dir_all` would otherwise reset the clock); an
/// empty dir falls back to the dir's own mtime. Unreadable trees
/// fail safe (keep).
async fn dir_past_grace(dir: &Path, now: SystemTime, grace: Duration) -> bool {
    match newest_file_mtime(dir).await {
        Ok(mtime) => now
            .duration_since(mtime)
            .map(|age| age > grace)
            .unwrap_or(false),
        Err(_) => false,
    }
}

async fn newest_file_mtime(dir: &Path) -> std::io::Result<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut rd = tokio::fs::read_dir(&d).await?;
        while let Some(entry) = rd.next_entry().await? {
            let meta = entry.metadata().await?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else if let Ok(m) = meta.modified() {
                newest = Some(newest.map_or(m, |n| n.max(m)));
            }
        }
    }
    match newest {
        Some(m) => Ok(m),
        None => tokio::fs::metadata(dir).await?.modified(),
    }
}

/// Best-effort single-file reap. Already-gone is silent (someone
/// else cleaned up); other errors are logged and skipped — the next
/// daily run retries for free.
async fn reap_file(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => info!(path = %path.display(), "sweep: reaped orphan blob"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(error = ?e, path = %path.display(), "sweep: remove_file failed"),
    }
}

async fn blob_exists(data_dir: &Path, rel: &str) -> bool {
    tokio::fs::try_exists(data_dir.join(rel))
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    /// 1 h grace used across the suite — matches the production default.
    const GRACE: Duration = Duration::from_secs(3_600);

    /// Fresh per-test data dir under /tmp, passed directly into
    /// `run_sweep`. Never touches the process-global `AURIS_DATA_DIR`
    /// env var (deliberately avoids the `scoped_data_dir` set_var hack
    /// in api/artifacts.rs tests). Not cleaned up — the OS recycles
    /// /tmp and the volume is a handful of bytes per test.
    fn temp_data_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("auris-sweep-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_blob(data_dir: &Path, rel: &str) -> PathBuf {
        let abs = data_dir.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, b"test-bytes").unwrap();
        abs
    }

    /// Push a file's mtime 2 h into the past — past the 1 h grace.
    /// `File::set_modified` is stable std (1.75+); no filetime dep.
    fn backdate(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(2 * 3_600);
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(old).unwrap();
    }

    async fn test_user(pool: &PgPool) -> String {
        crate::storage::users::upsert_user_by_auth0_sub(
            pool,
            &format!("test|{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap()
        .id
    }

    async fn test_meeting(pool: &PgPool, user_id: &str) -> String {
        let mid = uuid::Uuid::new_v4().to_string();
        crate::storage::meetings::insert_meeting(
            pool,
            &mid,
            user_id,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        mid
    }

    #[sqlx::test]
    async fn sweep_reaps_orphan_artifact_blob_past_grace(pool: PgPool) {
        let data_dir = temp_data_dir();
        let abs = write_blob(&data_dir, "blobs/artifacts/ghost-user/ghost-artifact");
        backdate(&abs);

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert!(!report.aborted);
        assert_eq!(report.orphan_artifact_blobs, 1);
        assert!(!abs.exists(), "orphan artifact blob should be reaped");
    }

    #[sqlx::test]
    async fn sweep_keeps_artifact_blob_with_matching_row(pool: PgPool) {
        let data_dir = temp_data_dir();
        let uid = test_user(&pool).await;
        let rel = format!("blobs/artifacts/{uid}/art-1");
        crate::storage::artifacts::insert_artifact(
            &pool,
            "art-1",
            &uid,
            "x.md",
            "text/markdown",
            &rel,
            10,
        )
        .await
        .unwrap();
        let abs = write_blob(&data_dir, &rel);
        backdate(&abs); // age alone must never make a live blob reapable

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.orphan_artifact_blobs, 0);
        assert!(abs.exists(), "live artifact blob must survive");
    }

    #[sqlx::test]
    async fn sweep_keeps_orphan_blob_within_grace(pool: PgPool) {
        let data_dir = temp_data_dir();
        // Freshly written (mtime = now) — simulates an upload that has
        // written its blob but not yet inserted its row.
        let abs = write_blob(&data_dir, "blobs/artifacts/ghost-user/in-flight");

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.orphan_artifact_blobs, 0);
        assert!(abs.exists(), "fresh orphan must survive the grace window");
    }

    #[sqlx::test]
    async fn sweep_reaps_meeting_dir_without_meeting_row(pool: PgPool) {
        let data_dir = temp_data_dir();
        let jsonl = write_blob(
            &data_dir,
            "blobs/meetings/ghost-meeting/transcription.jsonl",
        );
        let shot = write_blob(&data_dir, "blobs/meetings/ghost-meeting/screenshots/m1.png");
        backdate(&jsonl);
        backdate(&shot);

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.orphan_meeting_dirs, 1);
        assert!(
            !data_dir.join("blobs/meetings/ghost-meeting").exists(),
            "orphan meeting dir should be removed recursively"
        );
    }

    #[sqlx::test]
    async fn sweep_keeps_live_meeting_dir_and_reaps_only_unmatched_screenshot_and_chat_files(
        pool: PgPool,
    ) {
        let data_dir = temp_data_dir();
        let uid = test_user(&pool).await;
        let mid = test_meeting(&pool, &uid).await;

        let shot_rel = format!("blobs/meetings/{mid}/screenshots/mom-1.png");
        crate::storage::moments::insert_moment(
            &pool,
            "mom-1",
            &mid,
            "manual",
            0,
            None,
            Some(&shot_rel),
        )
        .await
        .unwrap();
        let chat_rel = format!("blobs/meetings/{mid}/chat/att-1.png");
        crate::storage::chat_attachments::insert_chat_attachment(
            &pool,
            "att-1",
            &mid,
            &uid,
            "image/png",
            &chat_rel,
            1,
        )
        .await
        .unwrap();

        let transcript = write_blob(
            &data_dir,
            &format!("blobs/meetings/{mid}/transcription.jsonl"),
        );
        let live_shot = write_blob(&data_dir, &shot_rel);
        let ghost_shot = write_blob(
            &data_dir,
            &format!("blobs/meetings/{mid}/screenshots/ghost.png"),
        );
        let live_chat = write_blob(&data_dir, &chat_rel);
        let ghost_chat = write_blob(&data_dir, &format!("blobs/meetings/{mid}/chat/ghost.png"));
        for f in [
            &transcript,
            &live_shot,
            &ghost_shot,
            &live_chat,
            &ghost_chat,
        ] {
            backdate(f);
        }

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.orphan_meeting_dirs, 0);
        assert_eq!(report.orphan_screenshots, 1);
        assert_eq!(report.orphan_chat_files, 1);
        assert!(transcript.exists(), "live dir's transcript must survive");
        assert!(live_shot.exists());
        assert!(live_chat.exists());
        assert!(
            !ghost_shot.exists(),
            "unmatched screenshot should be reaped"
        );
        assert!(!ghost_chat.exists(), "unmatched chat file should be reaped");
    }

    #[sqlx::test]
    async fn sweep_keeps_screenshot_whose_moment_row_has_null_asset_path(pool: PgPool) {
        let data_dir = temp_data_dir();
        let uid = test_user(&pool).await;
        let mid = test_meeting(&pool, &uid).await;
        // Orphan path 5: the PNG landed on disk but the follow-up
        // `update_moment_asset_path` never ran — row exists, NULL path.
        crate::storage::moments::insert_moment(&pool, "mom-null", &mid, "manual", 0, None, None)
            .await
            .unwrap();
        let rel = format!("blobs/meetings/{mid}/screenshots/mom-null.png");
        let abs = write_blob(&data_dir, &rel);
        backdate(&abs);

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.orphan_screenshots, 0, "heal-or-keep, never reap");
        assert_eq!(report.healed_asset_paths, 1);
        assert!(abs.exists());
        let healed: (Option<String>,) =
            sqlx::query_as(r#"SELECT asset_path FROM moments WHERE id = 'mom-null'"#)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            healed.0.as_deref(),
            Some(rel.as_str()),
            "asset_path back-filled"
        );
    }

    #[sqlx::test]
    async fn sweep_reports_rows_with_missing_blobs_without_touching_rows(pool: PgPool) {
        let data_dir = temp_data_dir();
        let uid = test_user(&pool).await;
        let mid = test_meeting(&pool, &uid).await;
        crate::storage::artifacts::insert_artifact(
            &pool,
            "art-gone",
            &uid,
            "x.md",
            "text/markdown",
            &format!("blobs/artifacts/{uid}/art-gone"),
            10,
        )
        .await
        .unwrap();
        crate::storage::moments::insert_moment(
            &pool,
            "mom-gone",
            &mid,
            "manual",
            0,
            None,
            Some(&format!("blobs/meetings/{mid}/screenshots/mom-gone.png")),
        )
        .await
        .unwrap();
        crate::storage::chat_attachments::insert_chat_attachment(
            &pool,
            "att-gone",
            &mid,
            &uid,
            "image/png",
            &format!("blobs/meetings/{mid}/chat/att-gone.png"),
            1,
        )
        .await
        .unwrap();
        // No files on disk at all — all three rows reference missing blobs.

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert_eq!(report.rows_missing_blob, 3);
        let counts: (i64, i64, i64) = sqlx::query_as(
            r#"SELECT (SELECT COUNT(*) FROM artifacts),
                      (SELECT COUNT(*) FROM moments),
                      (SELECT COUNT(*) FROM chat_attachments)"#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(counts, (1, 1, 1), "inverse pass must never mutate rows");
    }

    #[sqlx::test]
    async fn sweep_aborts_when_reap_ratio_exceeds_threshold(pool: PgPool) {
        let data_dir = temp_data_dir();
        let mut files = Vec::new();
        // 12 orphans / 12 scanned: > ABORT_MIN_FILES and > 50%.
        for i in 0..12 {
            let abs = write_blob(&data_dir, &format!("blobs/artifacts/ghost-user/orphan-{i}"));
            backdate(&abs);
            files.push(abs);
        }

        let report = run_sweep(&pool, &data_dir, GRACE, false).await.unwrap();

        assert!(report.aborted, "12/12 orphans must trip the >50% guard");
        for f in &files {
            assert!(f.exists(), "abort must delete nothing");
        }
    }

    #[sqlx::test]
    async fn sweep_dry_run_reports_but_deletes_nothing(pool: PgPool) {
        let data_dir = temp_data_dir();
        let abs = write_blob(&data_dir, "blobs/artifacts/ghost-user/ghost-artifact");
        backdate(&abs);

        let report = run_sweep(&pool, &data_dir, GRACE, true).await.unwrap();

        assert_eq!(report.orphan_artifact_blobs, 1, "dry run still reports");
        assert!(abs.exists(), "dry run must not delete");
    }

    const SWEEP_ENV_KEYS: [&str; 4] = [
        "AURIS_SWEEP_INITIAL_DELAY_SECS",
        "AURIS_SWEEP_INTERVAL_SECS",
        "AURIS_SWEEP_GRACE_SECS",
        "AURIS_SWEEP_DRY_RUN",
    ];

    /// Save/clear the sweep env vars, run `f`, restore. Safe because
    /// the suite runs `--test-threads=1` (same pre-existing constraint
    /// the config.rs env tests rely on).
    fn with_clean_sweep_env(f: impl FnOnce()) {
        let saved: Vec<(&str, Option<String>)> = SWEEP_ENV_KEYS
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        for k in SWEEP_ENV_KEYS {
            std::env::remove_var(k);
        }
        f();
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn sweep_config_defaults() {
        with_clean_sweep_env(|| {
            let cfg = SweepConfig::from_env();
            assert_eq!(cfg.initial_delay, Duration::from_secs(600));
            assert_eq!(cfg.interval, Duration::from_secs(86_400));
            assert_eq!(cfg.grace, Duration::from_secs(3_600));
            assert!(!cfg.dry_run);
        });
    }

    #[test]
    fn sweep_config_reads_env_overrides() {
        with_clean_sweep_env(|| {
            std::env::set_var("AURIS_SWEEP_INITIAL_DELAY_SECS", "1");
            std::env::set_var("AURIS_SWEEP_INTERVAL_SECS", "60");
            std::env::set_var("AURIS_SWEEP_GRACE_SECS", "120");
            std::env::set_var("AURIS_SWEEP_DRY_RUN", "1");
            let cfg = SweepConfig::from_env();
            assert_eq!(cfg.initial_delay, Duration::from_secs(1));
            assert_eq!(cfg.interval, Duration::from_secs(60));
            assert_eq!(cfg.grace, Duration::from_secs(120));
            assert!(cfg.dry_run);
        });
    }
}
