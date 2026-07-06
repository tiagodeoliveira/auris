//! Error types for LLM initialisation and extraction.

use std::time::Duration;

use thiserror::Error;

/// Errors that can occur during LLM client initialisation.
#[derive(Debug, Error)]
pub enum LlmInitError {
    #[error("LLM provider init failed: {0}")]
    Provider(String),

    #[error(
        "Unknown LLM provider: '{0}'. Accepted values: bedrock, openai, anthropic, gemini, xai"
    )]
    UnknownProvider(String),

    #[error("Missing credentials for provider '{0}'. Check the required env var.")]
    MissingProviderCredentials(String),

    /// A required pool-specific env var (e.g. `AURIS_LLM_CHAT_MODEL_ID`)
    /// is unset or empty. The spec mandates explicit per-pool config —
    /// no silent fallback to a default — so missing vars fail at boot.
    #[error("Missing required env var '{0}' for {1} pool")]
    MissingPoolEnvVar(String, &'static str),
}

/// Returned by [`LlmClient::gate`] when the per-pool circuit breaker
/// is open. Converts into [`ExtractionError::CircuitOpen`] via
/// `From`, so it flows through all `extract_*` return paths without
/// boilerplate at each call site.
#[derive(Debug, Error)]
#[error("circuit breaker open: {0}")]
pub struct CircuitOpenError(pub String);

/// Errors that can occur during metadata extraction.
///
/// Variants intentionally mirror rig's `extractor::ExtractionError`
/// surface so callers / log sites can distinguish "provider rejected
/// the call" (quota, auth, rate limit, network — surface to operator)
/// from "model returned malformed output" (schema drift — retry or
/// log and move on). The previous flat `Extract(String)` blended
/// these and produced "Extraction failed: missing field `…`" log
/// lines for what was actually a billing error.
#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("LLM call exceeded timeout of {0:?}")]
    Timeout(Duration),

    /// The per-pool circuit breaker is open (too many recent
    /// failures). The call was rejected without reaching the
    /// provider — retrying immediately would just hit the breaker
    /// again. Callers should surface a "service temporarily
    /// unavailable" message and let the cooldown expire.
    #[error("circuit open: {0}")]
    CircuitOpen(#[from] CircuitOpenError),

    /// rig's `DeserializationError` — model emitted content but it
    /// didn't satisfy the requested JSON schema. Usually means the
    /// model produced a natural-language refusal / apology instead of
    /// structured output. Not retryable at this layer.
    #[error("LLM output did not match schema: {0}")]
    Schema(String),

    /// rig's `CompletionError` — the provider call itself failed
    /// (HTTP error, rate limit, auth, network — but NOT billing /
    /// quota; those route to [`Self::QuotaExhausted`] via the
    /// [`looks_like_quota`] predicate). Carries the raw provider
    /// message so operators can tell networking from auth.
    #[error("LLM provider error: {0}")]
    Provider(String),

    /// Provider rejected the call because the account is out of
    /// credits / over quota / billing-blocked. Distinct from
    /// `Provider` so downstream can:
    ///   - Skip retries (retrying a billing failure won't fix it).
    ///   - Surface a billing-specific UI affordance ("buy credits")
    ///     instead of a generic "something went wrong."
    ///   - Page differently in monitoring (billing = self-inflicted,
    ///     not a provider outage).
    ///
    /// Detected via [`looks_like_quota`] applied to the rig error
    /// message; see that fn for the keyword strategy.
    #[error("LLM quota exhausted: {0}")]
    QuotaExhausted(String),

    /// rig's `NoData` — model accepted the request but didn't emit a
    /// structured response (didn't call the `submit` tool). Distinct
    /// from `Schema` (output was emitted but malformed).
    #[error("LLM returned no data")]
    NoData,

    /// Local errors in our wrapper that aren't from rig — building
    /// messages, base64-encoding, etc. Kept as a catch-all so we
    /// don't have to invent a variant for every bookkeeping failure.
    #[error("Extraction failed: {0}")]
    Extract(String),
}

/// Returns true when `msg` (the stringified body of a rig
/// `CompletionError`) looks like a quota / billing rejection rather
/// than a transient network or auth issue.
///
/// The five providers we support phrase this differently:
///   - Anthropic:  "Your credit balance is too low to access the
///     Anthropic API. Please go to Plans & Billing..."
///   - OpenAI:     ".....type":"insufficient_quota","code":"insufficient_quota"..."
///   - Bedrock:    "ServiceQuotaExceededException: ..."
///     (NOT ThrottlingException — that's transient and
///     should retry; NOT AccessDeniedException — that's
///     config, not billing.)
///   - xAI:        "...has either used all available credits or
///     reached its monthly spending limit..." (HTTP 403, also seen
///     framed as "Code `403`: ..." via rig's api.rs message()).
///     A plain "Code `429`: rate limit exceeded" is transient and
///     must NOT match.
///   - Gemini:     "You exceeded your current quota, please check
///     your plan and billing details." (HTTP 429,
///     RESOURCE_EXHAUSTED). CAVEAT: Google uses this exact phrase
///     for BOTH transient per-minute rate limits and real
///     daily/billing quota; only the quotaId inside the
///     QuotaFailure details discriminates ("...PerMinute..." vs
///     "...PerDay..."). The predicate therefore requires the
///     phrase AND the absence of "perminute" (lowercased).
///
/// False positives are bad — they'd mask real auth/network issues
/// behind a billing UI. False negatives are recoverable — operators
/// still see the raw string in the `Provider` log. Lean
/// conservative.
///
/// Unit tests below pin each provider's exact phrasing; any
/// deliberate pattern change must update them.
pub(crate) fn looks_like_quota(msg: &str) -> bool {
    // Lowercase once so each substring check stays case-insensitive
    // without per-needle allocation. The patterns are deliberately
    // narrow — they all uniquely identify "your account is out of
    // money/credits" rather than any of the broader failure modes
    // (rate limiting, auth, network, model-not-found). Rate limits
    // are intentionally NOT included: they're transient pressure
    // that retries can resolve, and surfacing a billing UI for a
    // ThrottlingException would be a false positive.
    let m = msg.to_ascii_lowercase();
    m.contains("credit balance")                 // Anthropic
        || m.contains("insufficient_quota")      // OpenAI (JSON key/code)
        || m.contains("servicequotaexceededexception") // Bedrock (account-level)
        || m.contains("used all available credits")    // xAI (credits exhausted)
        || m.contains("monthly spending limit")        // xAI (same body, second clause)
        // Gemini: "You exceeded your current quota" fires for BOTH
        // transient per-minute rate limits and real daily/billing
        // quota — the same phrase, distinguished only by the quotaId
        // in the QuotaFailure details ("...PerMinute..." vs
        // "...PerDay..."). Per-minute = transient pressure that
        // retries resolve, NOT billing, so guard it out. A body
        // listing both violations degrades to Provider — an
        // acceptable false negative per the doctrine above. (OpenAI's
        // insufficient_quota body contains the same phrase; harmless
        // overlap, it's already matched by its own pattern.)
        || (m.contains("exceeded your current quota") && !m.contains("perminute"))
}

/// Map rig's structured extractor error into our [`ExtractionError`].
///
/// This is the single funnel through which rig errors enter our
/// error model — every log site downstream renders whatever variant
/// and `Display` this function produces. Keep the mapping cheap and
/// allocation-light; the slow path (formatting for logs) handles
/// presentation.
pub(crate) fn classify_rig(e: rig_core::extractor::ExtractionError) -> ExtractionError {
    use rig_core::extractor::ExtractionError as Rig;
    match e {
        Rig::NoData => ExtractionError::NoData,
        Rig::DeserializationError(serde) => ExtractionError::Schema(serde.to_string()),
        Rig::CompletionError(c) => {
            let msg = c.to_string();
            if looks_like_quota(&msg) {
                ExtractionError::QuotaExhausted(msg)
            } else {
                ExtractionError::Provider(msg)
            }
        }
    }
}

/// Map an [`ExtractionError`] to the fixed status label recorded on
/// `llm_request_duration_seconds`.
///
/// Shares vocabulary with the live-agent fire paths (`agent/chat.rs`,
/// `agent/active.rs`), which record `"ok" | "rate_limited" | "error"`
/// — quota maps to `"rate_limited"` so one dashboard query covers
/// both layers. `"timeout"` and `"circuit_open"` are extract-path
/// additions (the agent layer cannot produce them here). The set is
/// closed (5 values incl. `"ok"`), so label cardinality stays bounded
/// and no user data enters metric labels.
pub(crate) fn metric_status(e: &ExtractionError) -> &'static str {
    match e {
        ExtractionError::Timeout(_) => "timeout",
        ExtractionError::QuotaExhausted(_) => "rate_limited",
        ExtractionError::CircuitOpen(_) => "circuit_open",
        ExtractionError::Schema(_)
        | ExtractionError::Provider(_)
        | ExtractionError::NoData
        | ExtractionError::Extract(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── looks_like_quota ───────────────────────────────────────────────
    //
    // These cases seed the design. Each string is shaped like what
    // rig actually surfaces from the corresponding provider when
    // billing/quota fails. Implement `looks_like_quota` so the
    // POSITIVES return true and the NEGATIVES return false.

    #[test]
    fn looks_like_quota_anthropic_credits() {
        // Anthropic's "out of credits" body (HTTP 400, invalid_request_error).
        let s = r#"ProviderError: {"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}}"#;
        assert!(
            looks_like_quota(s),
            "Anthropic credit-balance body should match"
        );
    }

    #[test]
    fn looks_like_quota_openai_insufficient() {
        // OpenAI's `insufficient_quota` body (HTTP 429).
        let s = r#"ResponseError: HTTP 429 {"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        assert!(
            looks_like_quota(s),
            "OpenAI insufficient_quota body should match"
        );
    }

    #[test]
    fn looks_like_quota_bedrock_service_quota() {
        // Bedrock's account-level quota (distinct from per-second throttling).
        let s = "ServiceQuotaExceededException: You have exceeded the quota for this account.";
        assert!(
            looks_like_quota(s),
            "Bedrock ServiceQuotaExceededException should match"
        );
    }

    #[test]
    fn looks_like_quota_xai_credits_exhausted() {
        // xAI 403 credits-exhausted body. rig 0.37 surfaces it either as
        // the raw HTTP body (non-2xx path) or via api.rs message() framed
        // as "Code `{code}`: {error}" — the credits text survives both.
        let s = "Code `403`: Your team 1a2b3c has either used all available credits or \
                 reached its monthly spending limit. To continue making API requests, \
                 please purchase more credits or raise your spending limit.";
        assert!(
            looks_like_quota(s),
            "xAI credits-exhausted body should match"
        );
    }

    #[test]
    fn looks_like_quota_gemini_daily_quota() {
        // Gemini 429 RESOURCE_EXHAUSTED with a *daily* quota violation —
        // a real billing/plan limit, not transient pressure.
        let s = r#"ProviderError: {"error":{"code":429,"message":"You exceeded your current quota, please check your plan and billing details.","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaId":"GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]}]}}"#;
        assert!(looks_like_quota(s), "Gemini daily-quota body should match");
    }

    #[test]
    fn looks_like_quota_gemini_per_minute_is_not_quota() {
        // Same RESOURCE_EXHAUSTED phrasing but a per-minute (transient)
        // limit — must NOT match, or we'd show a billing UI / skip
        // retries for a rate-limit blip. The quotaId discriminates.
        let s = r#"ProviderError: {"error":{"code":429,"message":"You exceeded your current quota, please check your plan and billing details.","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaId":"GenerateRequestsPerMinutePerProjectPerModel"}]}]}}"#;
        assert!(
            !looks_like_quota(s),
            "Gemini per-minute 429 must stay Provider"
        );
    }

    #[test]
    fn looks_like_quota_negatives() {
        // Transient / unrelated failures — must NOT match, otherwise
        // we'd surface a billing UI for a network blip or a bad key.
        let cases = [
            "HttpError: connection reset by peer",
            "ResponseError: HTTP 401 {\"error\":\"invalid_api_key\"}",
            "ProviderError: model_not_found",
            "ResponseError: HTTP 500 Internal Server Error",
            // ThrottlingException is per-second pressure, recoverable
            // by retry — debatable. Default: NOT quota. Flip in the
            // predicate if you'd rather surface "please try later".
            "ThrottlingException: Rate exceeded",
            // xAI plain rate limit (HTTP 429) — transient, NOT billing.
            "Code `429`: rate limit exceeded",
        ];
        for s in cases {
            assert!(!looks_like_quota(s), "false positive on: {s}");
        }
    }

    #[test]
    fn classify_rig_routes_quota_through_predicate() {
        // Sanity: when the predicate fires, classify_rig surfaces
        // QuotaExhausted; when it doesn't, Provider. This guards the
        // wiring — separate from whether the predicate itself is
        // accurate (those tests above).
        use rig_core::completion::CompletionError;
        use rig_core::extractor::ExtractionError as Rig;

        let billing = Rig::CompletionError(CompletionError::ProviderError(
            "Your credit balance is too low".to_string(),
        ));
        assert!(matches!(
            classify_rig(billing),
            ExtractionError::QuotaExhausted(_)
        ));

        let network = Rig::CompletionError(CompletionError::ProviderError(
            "connection reset by peer".to_string(),
        ));
        assert!(matches!(
            classify_rig(network),
            ExtractionError::Provider(_)
        ));
    }

    // ─── metric_status ──────────────────────────────────────────────────

    #[test]
    fn metric_status_timeout_maps_to_timeout() {
        let e = ExtractionError::Timeout(Duration::from_secs(30));
        assert_eq!(metric_status(&e), "timeout");
    }

    #[test]
    fn metric_status_quota_maps_to_rate_limited() {
        let e = ExtractionError::QuotaExhausted("Your credit balance is too low".to_string());
        assert_eq!(metric_status(&e), "rate_limited");
    }

    #[test]
    fn metric_status_circuit_open_maps_to_circuit_open() {
        let e = ExtractionError::CircuitOpen(CircuitOpenError("llm-background".to_string()));
        assert_eq!(metric_status(&e), "circuit_open");
    }

    #[test]
    fn metric_status_provider_schema_nodata_extract_map_to_error() {
        let cases = [
            ExtractionError::Provider("HTTP 500 Internal Server Error".to_string()),
            ExtractionError::Schema("missing field `long_summary`".to_string()),
            ExtractionError::NoData,
            ExtractionError::Extract("build message: boom".to_string()),
        ];
        for e in cases {
            assert_eq!(metric_status(&e), "error", "wrong status for {e:?}");
        }
    }
}
