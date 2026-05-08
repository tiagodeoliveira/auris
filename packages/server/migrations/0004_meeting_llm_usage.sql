-- Per-meeting LLM usage + model attribution.
--
-- The in-memory LlmUsageTracker drains at meeting stop into one
-- llm_usage_at_stop log line; this migration adds the same numbers
-- to the meetings row so historical cost can be derived from the
-- DB later (the model used is what determines per-token rates, and
-- those rates may change over time — keeping the model_id with the
-- usage means a future cost-rollup view can apply the right rates
-- against the right model for each meeting).
--
-- All counts default to 0 — older meetings (pre-migration) get
-- zero usage attribution. Provider + model are nullable; older
-- meetings have no record of which provider/model ran.

ALTER TABLE meetings
    ADD COLUMN llm_calls               BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN llm_input_tokens        BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN llm_output_tokens       BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN llm_cached_input_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN llm_provider            TEXT,
    ADD COLUMN llm_model_id            TEXT;
