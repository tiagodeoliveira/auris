//! Postgres persistence layer.
//!
//! Connection string comes from `DATABASE_URL`; the `docker-compose.yml`
//! at the repo root brings up a local Postgres on `5432` matching the
//! default in `.env.example`.
//!
//! `<DATA_DIR>` (env var `AURIS_DATA_DIR`, default `./data`)
//! is still used for blob storage — transcript JSONL, moment screenshots
//! — but no longer hosts the relational store. The two surfaces are
//! independent so the server can scale horizontally with Postgres in
//! front while blob storage moves to S3 (or stays local during dev).
//!
//! All write paths run inside small, focused transactions on the
//! `PgPool`. The pool itself is held by `ServerHandle`; intent
//! handlers reach for it after `apply_intent` returns, keeping the
//! `SessionRegistry` mutex free of any I/O.

pub mod artifacts;
pub mod chat_attachments;
pub mod image;
pub mod items;
pub mod meetings;
pub mod moments;
pub mod persistence_loop;
pub mod quick_ask;
pub mod users;

// Convenience re-exports for the most-used row types so callers
// can keep short paths (e.g. `crate::storage::UserRow`):
pub use artifacts::ArtifactRow;
pub use chat_attachments::ChatAttachmentRow;
pub use meetings::AttachedMeetingMeta;
pub use moments::MomentRow;
pub use quick_ask::QuickAskRow;
pub use users::UserRow;

use std::path::PathBuf;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::info;

/// Absolute path to the data directory (`<DATA_DIR>` in the docs).
/// Resolves the env var, expands `~`, and creates the directory if
/// it doesn't exist yet. Hosts `blobs/` for transcript JSONL and
/// moment screenshots; the relational store lives in Postgres.
pub fn data_dir() -> Result<PathBuf> {
    let raw = crate::config::var_or("AURIS_DATA_DIR", "./data");
    let expanded = if let Some(stripped) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME not set; cannot expand ~")?;
        PathBuf::from(home).join(stripped)
    } else {
        PathBuf::from(raw)
    };
    std::fs::create_dir_all(&expanded)
        .with_context(|| format!("failed to create data dir at {}", expanded.display()))?;
    Ok(expanded)
}

/// Open the Postgres pool against `$DATABASE_URL` and run pending
/// migrations. Idempotent on already-migrated databases.
pub async fn open_pool() -> Result<PgPool> {
    let url = std::env::var("DATABASE_URL").context(
        "DATABASE_URL is required (e.g. postgres://auris:dev@localhost:5432/auris). \
         Run `docker compose up -d postgres` from the repo root for a local instance.",
    )?;
    let pool = open_pool_at(&url).await?;
    info!("postgres ready");
    Ok(pool)
}

/// Test/integration entrypoint: open against an arbitrary URL.
pub async fn open_pool_at(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(url)
        .await
        .with_context(|| format!("failed to open postgres at {}", redact_url(url)))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("sqlx migrations failed")?;

    Ok(pool)
}

/// Render a connection URL with the password masked. Used in error
/// messages so a misconfigured `DATABASE_URL` doesn't leak the
/// credential into logs.
///
/// Format expected: `scheme://[user[:password]@]host[:port]/path`.
/// We mask whatever sits between the first `:` after `//` and the
/// first `@`. Falls through unchanged if no `@` is present.
fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = scheme_end + 3;
    let Some(at_offset) = url[after_scheme..].find('@') else {
        return url.to_string();
    };
    let at_idx = after_scheme + at_offset;
    let Some(colon_offset) = url[after_scheme..at_idx].find(':') else {
        // user only, no password.
        return url.to_string();
    };
    let colon_idx = after_scheme + colon_offset;
    let mut out = String::with_capacity(url.len());
    out.push_str(&url[..colon_idx + 1]);
    out.push_str("***");
    out.push_str(&url[at_idx..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_masks_password() {
        assert_eq!(
            redact_url("postgres://user:secret@host:5432/db"),
            "postgres://user:***@host:5432/db"
        );
        // No password — pass through untouched.
        assert_eq!(
            redact_url("postgres://user@host/db"),
            "postgres://user@host/db"
        );
    }
}
