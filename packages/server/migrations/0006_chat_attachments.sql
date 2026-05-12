-- 0006_chat_attachments.sql
--
-- Adds the per-meeting chat-attachment table backing the Mac "attach
-- screenshots to a chat message" feature. Cascade-deletes with the
-- meeting; the on-disk PNGs live under
--   <data_dir>/blobs/meetings/<meeting_id>/chat/<attachment_id>.png
-- and are wiped by the existing meetings-delete handler's recursive
-- remove_dir_all on <data_dir>/blobs/meetings/<meeting_id>.

CREATE TABLE chat_attachments (
    id          TEXT PRIMARY KEY,
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    mime        TEXT NOT NULL,
    bytes_path  TEXT NOT NULL,
    bytes_size  BIGINT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX chat_attachments_meeting_id_idx ON chat_attachments (meeting_id);
