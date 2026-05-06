-- Extend `moments` for screenshot capture + async LLM summary.
--
-- `kind` is the discriminator that future moment-creation modes
-- (e.g., interview question prompts) will set to a different value.
-- The async summary worker dispatches on `kind`, so adding a new
-- mode is a code change, not a schema change.
--
-- `summary_status` tracks the async worker:
--   - `pending`: just created, worker hasn't run yet.
--   - `done`:    worker filled in `summary`.
--   - `failed`:  worker tried and gave up; clients can show "?"
--                without retrying. We don't auto-retry on boot today;
--                the moment + screenshot are still visible.
--
-- All three fields are nullable / defaulted so existing rows from
-- earlier dev sessions read back cleanly without a backfill.

ALTER TABLE moments ADD COLUMN kind TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE moments ADD COLUMN summary TEXT;
ALTER TABLE moments ADD COLUMN summary_status TEXT NOT NULL DEFAULT 'pending';
