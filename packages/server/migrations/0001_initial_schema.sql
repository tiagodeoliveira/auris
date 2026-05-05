-- Initial schema. Tracks two entities for now; items + transcripts
-- come in a follow-up migration once their persistence shape settles
-- (likely a mix of SQLite + blob storage under <DATA_DIR>/blobs/).
--
-- Conventions:
--  * IDs are TEXT (UUID v4) to stay aligned with the wire contract
--    (Item.id, Device.id, etc. are all strings server-side).
--  * Timestamps are stored as TEXT in RFC 3339 / ISO 8601. Strings are
--    lexicographically sortable for the same offset, so range queries
--    work without conversion. The `chrono` feature on `sqlx` decodes
--    these to `DateTime<Utc>` automatically.
--  * `metadata` is a JSON-encoded string, not a separate side table.
--    SQLite has json1 functions if we ever need to query inside it,
--    but the access pattern today is "hand it back to the client as a
--    map" — denormalised storage matches that exactly.

PRAGMA foreign_keys = ON;

CREATE TABLE meetings (
    id          TEXT PRIMARY KEY,
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    description TEXT,
    metadata    TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_meetings_started_at ON meetings(started_at DESC);

-- moments: user-marked points within a meeting. `t` is the offset in
-- milliseconds from the meeting's start, matching the wire contract's
-- `MarkMoment { t, note }`. `asset_path` is reserved for Phase 5
-- screenshot capture (will hold a relative path under
-- <DATA_DIR>/blobs/...) so we don't need a schema migration to add it.
CREATE TABLE moments (
    id         TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    t          INTEGER NOT NULL,
    note       TEXT,
    asset_path TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_moments_meeting_id ON moments(meeting_id, t);
