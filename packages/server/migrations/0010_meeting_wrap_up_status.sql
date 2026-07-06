-- Track the post-meeting wrap-up extraction state per meeting. The
-- wrap-up extractor (summarizer/wrap_up.rs) generates actions +
-- open_questions from the full transcript on stop_meeting; before
-- this column it ran silently and any failure was invisible to the
-- user — the past-meeting view just showed empty actions /
-- open_questions sections with no way to distinguish "no actions
-- spoken" from "the LLM call failed."
--
-- Column is nullable + defaults to NULL so existing meetings (which
-- predate the wrap-up extractor entirely) read as "no wrap-up was
-- attempted" rather than masquerading as either success or failure.
-- New meetings start as 'running' the moment the extractor task
-- spawns, transition to 'success' or 'failed' on completion.
--
-- The UI consumes this via /meetings/:id → MeetingDetail.wrap_up_status
-- and renders a banner on the past-meeting view when the value is
-- 'failed'. Values:
--
--   NULL      legacy / pre-extractor meeting; UI shows nothing
--   'running' extractor task is in flight; UI shows a subtle
--             "still extracting…" hint
--   'success' extractor finished cleanly (may have produced zero
--             items if the meeting had nothing to extract — that's
--             distinct from a failure and the banner stays hidden)
--   'failed'  extractor errored (LLM timeout, quota exhaustion,
--             network blip); UI shows the banner with a retry option

ALTER TABLE meetings
  ADD COLUMN wrap_up_status TEXT;
