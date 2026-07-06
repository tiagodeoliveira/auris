-- Convert `quick_asks.id` from UUID to TEXT to match the rest of the
-- schema's id convention (artifacts.id, meetings.id, etc. are all
-- TEXT). The original 0008 declared id as UUID, which broke at decode
-- time: sqlx has no implicit UUID -> String conversion, and the
-- QuickAskRow struct uses String. INSERT worked via an explicit
-- $1::uuid cast in upsert_quick_ask but SELECT failed.
--
-- USING id::text is a no-op on existing data because UUIDs already
-- serialize cleanly to canonical 8-4-4-4-12 hex.

ALTER TABLE quick_asks
    ALTER COLUMN id TYPE TEXT USING id::text;
