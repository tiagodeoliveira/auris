//! User persistence: find-or-create rows in the `users` table.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Server-internal user row. `id` is the UUID we mint; `auth0_sub`
/// is the stable identity from Auth0 ("auth0|...", "google-oauth2|...",
/// etc.). The schema keeps `email` + `name` as best-effort copies of
/// what Auth0 returned at the most recent login.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: String,
    pub auth0_sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    #[allow(dead_code)]
    pub created_at: DateTime<Utc>,
    #[allow(dead_code)]
    pub last_seen_at: DateTime<Utc>,
}

/// Find or create a `users` row matching `auth0_sub`. Updates `email`,
/// `name`, and `last_seen_at` on every call so the local mirror tracks
/// whatever the most recent JWT claimed (Auth0 is authoritative for
/// these — we just keep a copy for offline reads).
///
/// One-shot UPSERT using Postgres' `ON CONFLICT ... DO UPDATE ... RETURNING`,
/// so the row comes back fresh in a single round trip regardless of
/// whether we inserted or updated.
pub async fn upsert_user_by_auth0_sub(
    pool: &PgPool,
    auth0_sub: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<UserRow> {
    let id = uuid::Uuid::new_v4().to_string();
    let row: UserRow = sqlx::query_as(
        r#"
        INSERT INTO users (id, auth0_sub, email, name)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (auth0_sub) DO UPDATE SET
            email = COALESCE(EXCLUDED.email, users.email),
            name = COALESCE(EXCLUDED.name, users.name),
            last_seen_at = NOW()
        RETURNING id, auth0_sub, email, name, created_at, last_seen_at
        "#,
    )
    .bind(&id)
    .bind(auth0_sub)
    .bind(email)
    .bind(name)
    .fetch_one(pool)
    .await
    .with_context(|| format!("upsert_user({auth0_sub})"))?;
    Ok(row)
}

/// Read-only lookup of a `users` row by `auth0_sub`. Unlike
/// `upsert_user_by_auth0_sub` this never writes — no row creation,
/// no `last_seen_at` touch. Used by the `recover-meeting` binary's
/// `--dry-run` path to validate the operator's `--user-id` without
/// breaking the "no DB writes" promise.
pub async fn find_user_by_auth0_sub(pool: &PgPool, auth0_sub: &str) -> Result<Option<UserRow>> {
    let row: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, auth0_sub, email, name, created_at, last_seen_at
          FROM users
         WHERE auth0_sub = $1
        "#,
    )
    .bind(auth0_sub)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("find_user_by_auth0_sub({auth0_sub})"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn find_user_by_auth0_sub_returns_existing_row(pool: PgPool) {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        let created = upsert_user_by_auth0_sub(&pool, &sub, Some("t@example.com"), Some("T"))
            .await
            .unwrap();

        let found = find_user_by_auth0_sub(&pool, &sub)
            .await
            .unwrap()
            .expect("row must exist after upsert");
        assert_eq!(found.id, created.id);
        assert_eq!(found.auth0_sub, sub);
        assert_eq!(found.email.as_deref(), Some("t@example.com"));
    }

    #[sqlx::test]
    async fn find_user_by_auth0_sub_returns_none_for_unknown_sub(pool: PgPool) {
        let found = find_user_by_auth0_sub(&pool, "auth0|does-not-exist")
            .await
            .unwrap();
        assert!(found.is_none(), "unknown sub must return None, not error");
    }
}
