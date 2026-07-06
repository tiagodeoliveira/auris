-- Per-pool LLM usage attribution. The single shared LlmClient is
-- being split into chat + background pools (see spec
-- 2026-05-24-split-llm-pool-design.md), each with its own provider,
-- model, and usage tracker. The existing meetings.llm_* columns
-- (migration 0004) collapse a meeting's usage into one row — fine
-- when there was one pool, ambiguous when there are two.
--
-- This table holds one row per (meeting_id, pool), so meeting-stop
-- drain can write each pool's usage independently and downstream
-- cost rollups can aggregate however they need.
--
-- The meetings.llm_* columns from 0004 are intentionally left in
-- place to preserve historical data for meetings closed before
-- this migration. New meetings stop writing to them; a future
-- migration can drop them once consumers move to this table.

CREATE TABLE meeting_llm_usage (
    meeting_id          TEXT    NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    pool                TEXT    NOT NULL,
    provider            TEXT    NOT NULL,
    model_id            TEXT    NOT NULL,
    calls               BIGINT  NOT NULL DEFAULT 0,
    input_tokens        BIGINT  NOT NULL DEFAULT 0,
    output_tokens       BIGINT  NOT NULL DEFAULT 0,
    cached_input_tokens BIGINT  NOT NULL DEFAULT 0,
    PRIMARY KEY (meeting_id, pool)
);
