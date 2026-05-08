-- Per-meeting items table. Persists everything the live overlay
-- shows EXCEPT transcript-mode items — those continue to live in
-- the per-meeting `transcription.jsonl` blob (sufficient for
-- downstream consumers: moment-summary windows, mnemo push,
-- meeting-detail transcript view). Storing transcripts here too
-- would double-write the highest-volume mode for no read-path
-- benefit.
--
-- The remaining modes (highlights / actions / open_questions /
-- summary / chat) all land here, written on every `ItemsUpdate`
-- broadcast. Replace-strategy modes (highlights / summary / chat)
-- atomically delete-then-insert per mode in a transaction; append
-- modes (actions / open_questions) just insert with conflict-skip.

CREATE TABLE items (
    -- Server-assigned id from the broadcast (e.g. h-<uuid>,
    -- a-<uuid>, summary-<uuid>, chat-q-<uuid>). Keyed in DB so
    -- `INSERT ... ON CONFLICT DO NOTHING` is safe under retry.
    id          TEXT PRIMARY KEY,
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    mode        TEXT NOT NULL,
    text        TEXT NOT NULL,
    -- Currently nullable; expand_item placeholder lives here once
    -- that flow lands a real LLM body.
    detail      TEXT,
    -- Milliseconds from meeting start. 0 for modes where the
    -- timestamp is meaningless (summary / chat).
    t_ms        BIGINT NOT NULL,
    -- Mode-specific blob — `{owner, due}` for actions,
    -- `{importance}` for highlights, `{kind, context}` for
    -- open_questions, `{role, pending?}` for chat. Future fields
    -- land without a schema migration.
    meta        JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Single index covers the meeting-detail query
-- (`WHERE meeting_id = $1 ORDER BY mode, created_at`) and the
-- mode-specific delete that Replace strategy uses
-- (`WHERE meeting_id = $1 AND mode = $2`).
CREATE INDEX idx_items_meeting_mode_created
    ON items (meeting_id, mode, created_at);
