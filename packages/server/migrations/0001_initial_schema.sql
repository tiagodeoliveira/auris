-- Initial Postgres schema. Single migration — pre-existing dev data
-- on SQLite was wiped intentionally rather than ported, see commit
-- log for the rationale.
--
-- Conventions:
--  * IDs are TEXT (UUID v4 stringified) — matches the wire contract
--    (`Item.id`, `Device.id`, `Moment.id`, etc. are all `String`
--    server-side). We mint UUIDs in code; storing them as TEXT keeps
--    SELECT-into-tuple decoding identical between Postgres and the
--    wire shape.
--  * Timestamps are `TIMESTAMPTZ`. The chrono `DateTime<Utc>` decode
--    handles the conversion both directions.
--  * `metadata` is a JSON-as-TEXT column (not `JSONB`). The access
--    pattern is "load the blob, hand it to the client" — there are
--    no server-side filters into the JSON yet. Switching to `JSONB`
--    is a one-line ALTER if that ever changes.

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
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_auth0_sub ON users(auth0_sub);

CREATE TABLE meetings (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    started_at  TIMESTAMPTZ NOT NULL,
    ended_at    TIMESTAMPTZ,
    description TEXT,
    metadata    TEXT NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Listing the user's meetings is the dominant read pattern; a
-- composite index makes "newest first per user" cheap.
CREATE INDEX idx_meetings_user_started ON meetings(user_id, started_at DESC);

-- Boot recovery scans for unfinished meetings; partial index keeps
-- the lookup O(log n) on the size of currently-live meetings only.
CREATE INDEX idx_meetings_active ON meetings(user_id, started_at DESC)
    WHERE ended_at IS NULL;

CREATE TABLE moments (
    id             TEXT PRIMARY KEY,
    meeting_id     TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    -- Discriminator for the moment-creation mode ("manual" today;
    -- future modes might use "interview" etc.). The async summary
    -- worker dispatches on this.
    kind           TEXT NOT NULL DEFAULT 'manual',
    -- Millisecond offset from `meetings.started_at` — matches the
    -- wire contract's `MarkMoment { t, note }`.
    t              BIGINT NOT NULL,
    note           TEXT,
    -- Relative path under `<DATA_DIR>/blobs/...` for screenshot
    -- assets. NULL when no screenshot was captured (e.g., PWA-only
    -- meetings without a screen-capture-capable device).
    asset_path     TEXT,
    summary        TEXT,
    -- 'pending' (just created) | 'done' (worker filled summary) |
    -- 'failed' (worker tried and gave up).
    summary_status TEXT NOT NULL DEFAULT 'pending',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_moments_meeting_id ON moments(meeting_id, t);
