-- Per-meeting "attached meetings" subsystem. Lets a meeting reference
-- past meetings so the agent can pull their mnemo-stored context via
-- the `fetch_meeting_summary` / `fetch_meeting` tools.
--
-- Same shape as `meeting_artifacts` (0002_artifacts.sql):
-- many-to-many join with cascade deletes on either end. Deleting the
-- parent removes the attachment rows; deleting the attached past
-- meeting also removes them — the agent only ever references
-- attached meetings whose summary still lives in mnemo + the meetings
-- table, no orphaned ids.

CREATE TABLE meeting_attachments (
    parent_meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    attached_meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    attached_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (parent_meeting_id, attached_meeting_id),
    -- Defensive: a meeting can't attach itself. Avoids the agent
    -- recursing into its own (still-empty) mnemo namespace.
    CHECK (parent_meeting_id <> attached_meeting_id)
);

-- Reverse lookup: "which meetings reference this one as context?"
-- Informational query (not used today) but cheap to add and useful
-- once the UI grows a "where was this meeting referenced from"
-- crumb on the detail view.
CREATE INDEX idx_meeting_attachments_attached
    ON meeting_attachments(attached_meeting_id);
