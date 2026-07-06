//! LLM usage tracking (per-user, per-pool counters).

use std::collections::HashMap;

use super::client::LlmClient;
use super::provider::LlmPool;

/// Per-meeting LLM usage snapshot. `calls` counts each successful
/// LLM round-trip; the three token fields come straight from each
/// provider's reported usage (via rig's `Usage` type) so the
/// numbers translate cleanly to actual cost on the provider's
/// pricing page — no chars-to-tokens approximation.
///
/// `cached_input_tokens` is the subset of `input_tokens` that
/// hit the provider's prompt cache (Anthropic's prompt-caching
/// beta, OpenAI's auto-cache). Useful to track separately
/// because it's billed at a fraction of the regular input rate
/// (~10% on Anthropic).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LlmUsage {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

/// Per-user accumulator for `LlmUsage`. Lives on `LlmClient` so any
/// caller of the typed extractors can opt in by passing `user_id`.
/// `take(user_id)` drains the entry — the typical flow is "record on
/// each call, take once at meeting stop and log the summary."
///
/// Each tracker carries its `LlmPool` so the meeting-stop drain can
/// log + persist usage attributed to the right pool.
#[derive(Debug)]
pub struct LlmUsageTracker {
    pool: LlmPool,
    inner: std::sync::Mutex<HashMap<String, LlmUsage>>,
}

impl LlmUsageTracker {
    pub fn new(pool: LlmPool) -> Self {
        Self {
            pool,
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn pool(&self) -> LlmPool {
        self.pool
    }

    pub fn record(
        &self,
        user_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
    ) {
        let mut map = self.inner.lock().expect("usage tracker mutex poisoned");
        let entry = map.entry(user_id.to_string()).or_default();
        entry.calls += 1;
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;
        entry.cached_input_tokens += cached_input_tokens;
    }

    pub fn take(&self, user_id: &str) -> LlmUsage {
        let mut map = self.inner.lock().expect("usage tracker mutex poisoned");
        map.remove(user_id).unwrap_or_default()
    }
}

/// One pool's contribution to the meeting-stop usage drain.
/// Pure data — built by `drain_meeting_usage` for downstream logging
/// and DB persistence. Pulled into its own type so the drain shape
/// is unit-testable without spinning up a real meeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolUsageRecord {
    pub pool: &'static str,
    pub provider: String,
    pub model_id: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

/// Drain both pools and return one record per pool that had calls.
/// Pools with zero calls are skipped so the drain stays quiet on
/// meetings that didn't trigger LLM work (e.g., AURIS_LLM_DISABLED,
/// audio-disabled, or canceled-immediately meetings).
pub(crate) fn drain_meeting_usage(
    user_id: &str,
    chat_llm: &LlmClient,
    background_llm: &LlmClient,
) -> Vec<PoolUsageRecord> {
    let mut out = Vec::new();
    for (pool, client) in [("chat", chat_llm), ("background", background_llm)] {
        let usage = client.take_usage(user_id);
        if usage.calls == 0 {
            continue;
        }
        out.push(PoolUsageRecord {
            pool,
            provider: format!("{:?}", client.provider()).to_lowercase(),
            model_id: client.model_id().to_string(),
            calls: usage.calls,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
        });
    }
    out
}

/// Structured extraction target for meeting metadata.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ExtractedMetadata {
    /// Concise meeting title in 8 words or fewer. Empty string if not extractable.
    pub title: String,

    /// Project name if mentioned. Empty string if not extractable.
    pub project: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_tracker_take_on_empty_returns_default() {
        let t = LlmUsageTracker::new(LlmPool::Chat);
        let u = t.take("user-1");
        assert_eq!(u.calls, 0);
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cached_input_tokens, 0);
    }

    #[test]
    fn usage_tracker_record_creates_entry() {
        let t = LlmUsageTracker::new(LlmPool::Chat);
        t.record("user-1", 100, 20, 5);
        let u = t.take("user-1");
        assert_eq!(u.calls, 1);
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.cached_input_tokens, 5);
    }

    #[test]
    fn usage_tracker_accumulates_across_calls() {
        let t = LlmUsageTracker::new(LlmPool::Chat);
        t.record("user-1", 100, 20, 5);
        t.record("user-1", 50, 10, 0);
        let u = t.take("user-1");
        assert_eq!(u.calls, 2);
        assert_eq!(u.input_tokens, 150);
        assert_eq!(u.output_tokens, 30);
        assert_eq!(u.cached_input_tokens, 5);
    }

    #[test]
    fn usage_tracker_take_clears_user_state() {
        let t = LlmUsageTracker::new(LlmPool::Chat);
        t.record("user-1", 100, 20, 0);
        let _ = t.take("user-1");
        let u2 = t.take("user-1");
        assert_eq!(u2.calls, 0);
        assert_eq!(u2.input_tokens, 0);
    }

    #[test]
    fn usage_tracker_isolates_users() {
        let t = LlmUsageTracker::new(LlmPool::Chat);
        t.record("user-1", 100, 20, 0);
        t.record("user-2", 50, 10, 0);
        let u1 = t.take("user-1");
        assert_eq!(u1.calls, 1);
        assert_eq!(u1.input_tokens, 100);
        let u2 = t.take("user-2");
        assert_eq!(u2.calls, 1);
        assert_eq!(u2.input_tokens, 50);
    }

    #[test]
    fn usage_tracker_carries_pool() {
        let t = LlmUsageTracker::new(LlmPool::Background);
        assert_eq!(t.pool(), LlmPool::Background);
        let t = LlmUsageTracker::new(LlmPool::Chat);
        assert_eq!(t.pool(), LlmPool::Chat);
    }

    #[test]
    fn two_pool_trackers_are_independent() {
        // Wiring assertion: two distinct trackers don't cross-feed.
        // If a future refactor accidentally shares the inner mutex
        // (e.g., a clippy-driven "extract this constant" change),
        // this test catches it before deploy.
        let chat = LlmUsageTracker::new(LlmPool::Chat);
        let background = LlmUsageTracker::new(LlmPool::Background);
        chat.record("u1", 100, 20, 5);
        background.record("u1", 50, 10, 0);
        let c = chat.take("u1");
        let b = background.take("u1");
        assert_eq!(c.input_tokens, 100, "chat tracker leaked into background");
        assert_eq!(b.input_tokens, 50, "background tracker leaked into chat");
    }

    // ─── Meeting-stop usage drain (relocated from ws::control, #3) ───

    /// Process-wide env-mutation lock. The drain tests set
    /// `AURIS_LLM_*_PROVIDER` + `ANTHROPIC_API_KEY` to feed
    /// `LlmClient::from_env`, then clear them. Multiple tests doing
    /// this in parallel under `cargo test` tore each other's reads
    /// (the shared `ANTHROPIC_API_KEY` could vanish mid-build).
    /// Serialise the env-touching span so each test sees a clean
    /// set/build/clear cycle. `tokio::sync::Mutex` (not std::sync)
    /// because the guard must live across `from_env`'s `.await`.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Build a `LlmClient` for tests without going through env vars
    /// from the user's shell. Holds `ENV_LOCK` for the full
    /// set/build/clear cycle so parallel tests don't race on the
    /// shared `ANTHROPIC_API_KEY` slot.
    ///
    /// `from_env` makes no network call — credentials are validated
    /// at first `extract` invocation — so a stub API key is enough
    /// and doesn't need to survive past this function.
    async fn test_client(pool: LlmPool) -> LlmClient {
        let _env_guard = ENV_LOCK.lock().await;
        let p = pool.as_str().to_uppercase();
        let provider_key = format!("AURIS_LLM_{p}_PROVIDER");
        let model_key = format!("AURIS_LLM_{p}_MODEL_ID");
        std::env::set_var(&provider_key, "anthropic");
        std::env::set_var(&model_key, "claude-haiku-4-5-20251001");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-only-not-real");
        let result = LlmClient::from_env(pool, None).await;
        std::env::remove_var(&provider_key);
        std::env::remove_var(&model_key);
        std::env::remove_var("ANTHROPIC_API_KEY");
        result.expect("test client builds")
    }

    #[tokio::test]
    async fn drain_skips_pools_with_zero_calls() {
        // Neither pool recorded usage → drain should return empty.
        // This is the "boring meeting" case (AURIS_LLM_DISABLED, or
        // meeting canceled before any LLM call fired). The drain
        // must NOT emit a log line or DB row for an empty pool.
        let chat = test_client(LlmPool::Chat).await;
        let bg = test_client(LlmPool::Background).await;
        let records = drain_meeting_usage("u1", &chat, &bg);
        assert!(records.is_empty(), "expected empty drain, got {records:?}");
    }

    #[tokio::test]
    async fn drain_emits_one_record_per_pool_with_calls() {
        // Only chat fired → drain returns exactly one record, tagged "chat".
        let chat = test_client(LlmPool::Chat).await;
        let bg = test_client(LlmPool::Background).await;
        chat.record_usage("u1", 100, 20, 5);
        let records = drain_meeting_usage("u1", &chat, &bg);
        assert_eq!(records.len(), 1, "expected one record, got {records:?}");
        assert_eq!(records[0].pool, "chat");
        assert_eq!(records[0].input_tokens, 100);
        assert_eq!(records[0].cached_input_tokens, 5);

        // Now background fires too on a fresh meeting (chat already
        // drained above so its counter is back to zero). Both pools
        // fire → two records, in (chat, background) order.
        chat.record_usage("u2", 10, 2, 0);
        bg.record_usage("u2", 200, 40, 0);
        let records = drain_meeting_usage("u2", &chat, &bg);
        assert_eq!(records.len(), 2, "expected two records, got {records:?}");
        assert_eq!(records[0].pool, "chat", "chat must come first");
        assert_eq!(records[0].input_tokens, 10);
        assert_eq!(records[1].pool, "background");
        assert_eq!(records[1].input_tokens, 200);
    }

    #[tokio::test]
    async fn drain_attributes_provider_and_model_per_pool() {
        // Pool routing regression: if a future refactor swaps the
        // tuple order or aliases the wrong client, this catches the
        // label mismatch.
        let chat = test_client(LlmPool::Chat).await;
        let bg = test_client(LlmPool::Background).await;
        chat.record_usage("u1", 1, 1, 0);
        bg.record_usage("u1", 1, 1, 0);
        let records = drain_meeting_usage("u1", &chat, &bg);
        assert_eq!(records.len(), 2);
        // Both stub clients use Anthropic + Haiku 4.5 — what matters
        // is that the pool label tracks the client identity, not
        // that the model differs (we'd need two providers to assert
        // that, and these tests stay env-free).
        assert_eq!(records[0].pool, "chat");
        assert_eq!(records[1].pool, "background");
        for r in &records {
            assert_eq!(r.provider, "anthropic");
            assert_eq!(r.model_id, "claude-haiku-4-5-20251001");
        }
    }
}
