-- Multi-user pivot. Existing meetings/moments are dropped — dev data,
-- the user explicitly opted to wipe rather than backfill. Phase B of
-- the OAuth migration: every meeting now belongs to a user, and the
-- `users` table mirrors Auth0 identity (`auth0_sub` is the foreign
-- key into Auth0).
--
-- Note on SQLite ALTER limitations: SQLite can ADD a column, but
-- adding a NOT NULL FK to an existing table with rows requires
-- either a DEFAULT or a full table rebuild. We're wiping anyway, so
-- DROP + CREATE is simpler and clearer than juggling temporaries.

PRAGMA foreign_keys = ON;

DROP TABLE IF EXISTS moments;
DROP TABLE IF EXISTS meetings;

CREATE TABLE users (
    -- Server-internal id (UUID v4). The `auth0_sub` is the stable
    -- identity from Auth0; we mint our own `id` so the rest of the
    -- schema doesn't have to know about Auth0's id format.
    id           TEXT PRIMARY KEY,
    auth0_sub    TEXT NOT NULL UNIQUE,
    -- Best-effort identity fields, copied from the JWT claims on each
    -- login. Both nullable — Auth0 doesn't always return them
    -- (e.g., scope=openid alone returns just `sub`).
    email        TEXT,
    name         TEXT,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_users_auth0_sub ON users(auth0_sub);

-- `user_id` is nullable in this migration so the existing intent
-- handlers (which don't yet pass a user_id) keep compiling and
-- running. Stage 3 of the OAuth rollout adds a follow-up migration
-- that tightens it to NOT NULL once every writer is user-scoped.
-- Treat NULL as "legacy / pre-OAuth" and never insert NULL rows
-- once stage 3 lands.
CREATE TABLE meetings (
    id          TEXT PRIMARY KEY,
    user_id     TEXT REFERENCES users(id) ON DELETE CASCADE,
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    description TEXT,
    metadata    TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Listing the user's meetings is the dominant read pattern; a
-- composite index makes "newest first per user" cheap.
CREATE INDEX idx_meetings_user_started ON meetings(user_id, started_at DESC);

CREATE TABLE moments (
    id             TEXT PRIMARY KEY,
    meeting_id     TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL DEFAULT 'manual',
    t              INTEGER NOT NULL,
    note           TEXT,
    asset_path     TEXT,
    summary        TEXT,
    summary_status TEXT NOT NULL DEFAULT 'pending',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_moments_meeting_id ON moments(meeting_id, t);
