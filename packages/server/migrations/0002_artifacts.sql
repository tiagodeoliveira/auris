-- Per-meeting artifacts subsystem (PLAN.md §3.7). Library-first:
-- artifacts live at the user level; meetings reference them via a
-- many-to-many join. Cascade rules let users delete artifacts
-- without orphaning past meeting attachments — the join row goes,
-- the artifact row stays for any other meeting that referenced it.

CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    mime_type       TEXT NOT NULL,
    -- Relative path under `<DATA_DIR>/blobs/artifacts/<user_id>/`.
    -- Stored as relative so a `DATA_DIR` rebase doesn't break links.
    asset_path      TEXT NOT NULL,
    -- ~50-token summary, included in every agent prompt as part of
    -- the items-as-memory pre-load.
    short_summary   TEXT,
    -- ~500-token summary, fetched on demand via the
    -- `fetch_artifact_summary` tool when the agent wants more than
    -- the pre-load gives.
    long_summary    TEXT,
    -- 'pending' (uploaded, summarizer hasn't run) | 'done'
    -- (summaries populated) | 'failed' (summarizer gave up).
    -- Attach is gated on 'done' — see `POST /api/meetings/:id/artifacts`.
    summary_status  TEXT NOT NULL DEFAULT 'pending',
    size_bytes      BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Listing the user's library is the dominant read pattern; mirror
-- the `meetings` index shape for cheap "newest first" queries.
CREATE INDEX idx_artifacts_user ON artifacts(user_id, created_at DESC);

CREATE TABLE meeting_artifacts (
    meeting_id   TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    attached_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (meeting_id, artifact_id)
);

-- Reverse-lookup: "which meetings used this artifact?" (informational
-- query; unused today, cheap to add now).
CREATE INDEX idx_meeting_artifacts_artifact ON meeting_artifacts(artifact_id);
