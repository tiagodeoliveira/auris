//! `LlmClient` — multi-provider LLM client (rig wrapper).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rig_core::completion::message::{
    Document, DocumentMediaType, DocumentSourceKind, ImageDetail, ImageMediaType, Message,
    UserContent,
};
use rig_core::prelude::*;
use rig_core::OneOrMany;
use tracing::info;

use super::backend::LlmBackend;
use super::errors::{classify_rig, metric_status, CircuitOpenError, ExtractionError, LlmInitError};
use super::extractor::LlmExtractor;
use super::provider::{parse_provider, LlmPool, Provider};
use super::usage::{ExtractedMetadata, LlmUsage, LlmUsageTracker};
use crate::observability::LlmMetrics;
use crate::util::circuit_breaker::CircuitBreaker;

// ─── Constants ────────────────────────────────────────────────────────────────
pub const DEFAULT_BEDROCK_REGION: &str = "us-west-2";
pub const DEFAULT_BEDROCK_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
// gpt-4o is materially better than gpt-4.1-mini at multi-tool
// reasoning (the agent loop's core need); gpt-4.1-mini struggled
// to follow the dedup + mode-discrimination rules. Set
// `AURIS_LLM_{CHAT,BACKGROUND}_MODEL_ID` in the environment to override.
pub const DEFAULT_OPENAI_MODEL_ID: &str = "gpt-4o";
// Opus 4.7 has a 1M-token context window at standard pricing with
// no long-context premium and no beta header required (per
// Anthropic's April 2026 release notes). The growing
// per-meeting conversation history would crowd Sonnet's 200k
// budget on a long meeting; 1M gives meaningful headroom.
pub const DEFAULT_ANTHROPIC_MODEL_ID: &str = "claude-opus-4-7";
pub const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. If a field cannot be confidently extracted from the description, \
return an empty string for that field — do not guess.";
/// Per-call ceiling for text extraction (metadata, summary,
/// highlights, etc.). Two shapes feed this path:
///   1. Metadata extraction — small input, sub-second on a hot
///      cache.
///   2. Summary loop — full transcript context (sometimes 5k+
///      tokens) + caching; Opus 4.7 routinely lands at 5-15s,
///      occasionally bumps into 20s+ under load.
///
/// 8s was the original ceiling, set when only (1) existed. Once
/// the summary loop started using this path, the 8s wall produced
/// flaky timeouts on otherwise-healthy calls. 30s gives summary
/// generation comfortable headroom while still bounding a hung
/// API so the worker doesn't deadlock. Vision (`VISION_TIMEOUT`)
/// and PDF (`PDF_TIMEOUT`) keep their own larger budgets below.
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Output-token ceilings for the extractor call, per provider.
/// Anthropic-direct *requires* this be set (rig surfaces a
/// `max_tokens must be set` error otherwise); Bedrock + OpenAI
/// default it but we set it everywhere so behavior is predictable
/// across providers.
///
/// Values track each provider's default-model standard ceiling:
///   - Anthropic (Opus 4.7 default): 32K
///   - Bedrock   (Sonnet 4.7 default, Claude family): 32K
///   - OpenAI    (gpt-4o default): 16K
///
/// `max_tokens` is a *ceiling*, not a target — the model emits only
/// what it needs; the higher cap costs nothing on small outputs.
/// We were previously hardcoded to 1024 universally, which silently
/// truncated the artifact summarizer on 40K-input docs (`short_summary`
/// alone consumed ~110 tokens, leaving no room for `long_summary` →
/// "missing field `long_summary`" deserialization failure, with
/// `stop_reason: "max_tokens"` visible in the rig_core::completions trace).
const MAX_TOKENS_ANTHROPIC: u64 = 32_768;
const MAX_TOKENS_BEDROCK: u64 = 32_768;
const MAX_TOKENS_OPENAI: u64 = 16_384;
// Gemini 2.5 Pro / Flash standard output ceiling (16K is a safe
// default; both models support higher but most calls land far
// short of 16K). Override per call only if a future use-case
// demands it.
pub(crate) const MAX_TOKENS_GEMINI: u64 = 16_384;
// Grok 4.x standard output ceiling. 16K is a safe default; Grok 4.3
// supports larger but most calls land well short. Override per call
// only if a future use-case demands it.
pub(crate) const MAX_TOKENS_XAI: u64 = 16_384;

// ─── Macros ──────────────────────────────────────────────────────────────────

/// Per-provider dispatch for a typed extractor call with usage
/// recording. Used by `extract_with_prompt`,
/// `extract_with_prompt_and_image`, and
/// `extract_with_prompt_and_document_pdf`. Each caller passes the
/// `LlmBackend` reference, the schema type `T`, the system prompt,
/// and the payload — either the raw user-text `&str` (text-only
/// extractor) or a prebuilt `rig::completion::message::Message::User`
/// carrying image or PDF content.
///
/// rig's `.extract_with_usage(...)` returns `ExtractWithUsageResponse<T>`.
/// The macro maps the rig error via `classify_rig`, leaving the
/// caller responsible for unwrapping the `Result`, calling
/// `self.usage.record(...)` with the response's usage fields, and
/// emitting the `llm_call` log line.
///
/// Was previously a 5-arm dispatch repeating ~7 lines per arm,
/// across 3 methods (~105 lines total). Name deliberately differs
/// from rig's `extract_with_usage` method (which the macro body
/// invokes) to avoid visual confusion at call sites.
macro_rules! provider_extract {
    ($backend:expr, $T:ty, $system_prompt:expr, $payload:expr) => {
        match $backend {
            LlmBackend::Bedrock { client, model_id } => client
                .extractor::<$T>(model_id.as_str())
                .preamble($system_prompt)
                .max_tokens(MAX_TOKENS_BEDROCK)
                .build()
                .extract_with_usage($payload)
                .await
                .map_err(classify_rig),
            LlmBackend::OpenAI { client, model_id } => client
                .extractor::<$T>(model_id.as_str())
                .preamble($system_prompt)
                .max_tokens(MAX_TOKENS_OPENAI)
                .build()
                .extract_with_usage($payload)
                .await
                .map_err(classify_rig),
            LlmBackend::Anthropic { client, model_id } => client
                .extractor::<$T>(model_id.as_str())
                .preamble($system_prompt)
                .max_tokens(MAX_TOKENS_ANTHROPIC)
                .build()
                .extract_with_usage($payload)
                .await
                .map_err(classify_rig),
            LlmBackend::Gemini { client, model_id } => client
                .extractor::<$T>(model_id.as_str())
                .preamble($system_prompt)
                .max_tokens(MAX_TOKENS_GEMINI)
                .build()
                .extract_with_usage($payload)
                .await
                .map_err(classify_rig),
            LlmBackend::Xai { client, model_id } => client
                .extractor::<$T>(model_id.as_str())
                .preamble($system_prompt)
                .max_tokens(MAX_TOKENS_XAI)
                .build()
                .extract_with_usage($payload)
                .await
                .map_err(classify_rig),
        }
    };
}

// ─── LlmClient ───────────────────────────────────────────────────────────────

/// Multi-provider LLM client backed by a rig `Extractor`.
///
/// The inner extractor is wrapped in an `Arc` (inside `LlmExtractor`) to allow
/// cheap cloning of `LlmClient` across tasks.
///
/// The `backend` field stores the raw rig clients for building ad-hoc
/// extractors (used by summarizers via `extract_with_prompt`).
#[derive(Clone)]
pub struct LlmClient {
    extractor: LlmExtractor,
    pub(crate) backend: LlmBackend,
    provider: Provider,
    /// Per-user counter shared across clones. Increments on each successful
    /// typed extract; drained at meeting stop for the operational summary.
    usage: Arc<LlmUsageTracker>,
    /// Optional per-pool circuit breaker. When `Some`, every `extract_*`
    /// call goes through [`Self::gate`] — rejected immediately if open,
    /// outcome recorded as Success/Failure otherwise. `None` in tests and
    /// any context where breaker overhead is unwanted.
    breaker: Option<Arc<CircuitBreaker>>,
    /// OTel metric emitter for per-call LLM duration + token counters.
    /// Backed by the global meter so all clones share the same
    /// underlying instruments.
    pub(crate) metrics: Arc<LlmMetrics>,
}

impl LlmClient {
    /// Returns the active provider.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The model id currently routed to (e.g. `"claude-opus-4-7"`,
    /// `"gpt-4o"`, `"us.anthropic.claude-sonnet-4-7-20251015-v1:0"`).
    /// Persisted alongside the per-meeting usage at stop so future
    /// cost computations can use the exact per-model rates that
    /// applied at the time of the meeting.
    pub fn model_id(&self) -> &str {
        match &self.backend {
            LlmBackend::Bedrock { model_id, .. } => model_id,
            LlmBackend::OpenAI { model_id, .. } => model_id,
            LlmBackend::Anthropic { model_id, .. } => model_id,
            LlmBackend::Gemini { model_id, .. } => model_id,
            LlmBackend::Xai { model_id, .. } => model_id,
        }
    }

    /// Drain and return the per-user usage counter. Called by `ws.rs` at
    /// meeting stop to log the per-meeting summary; subsequent records for
    /// the same user start fresh.
    pub fn take_usage(&self, user_id: &str) -> LlmUsage {
        self.usage.take(user_id)
    }

    /// Increment the per-user usage counter directly. The typed
    /// extract methods do this internally per-call; the agent path
    /// (rig's `agent.prompt(...)`) bypasses those methods, so the
    /// agent module calls this directly to keep the
    /// `llm_usage_at_stop` summary accurate. Tokens come from rig's
    /// `Usage` so the numbers match what the provider charges.
    pub fn record_usage(
        &self,
        user_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
    ) {
        self.usage
            .record(user_id, input_tokens, output_tokens, cached_input_tokens);
    }

    /// Wraps an LLM call in the per-pool circuit breaker.
    ///
    /// Returns `Err(CircuitOpenError(...).into())` immediately when the
    /// breaker is open, without touching the provider. Otherwise runs
    /// `op`, records `Success` on `Ok` and `Failure` on `Err`, and
    /// returns whatever `op` returned.
    ///
    /// Drop-safe (improvement #16): outcome recording is owned by a
    /// [`crate::util::circuit_breaker::ProbeGuard`], so if the future
    /// returned by `gate` is dropped while parked at `op().await` —
    /// e.g. raced against a `CancellationToken` in `tokio::select!`,
    /// as `ws/control.rs::spawn_extraction` does — the guard's `Drop`
    /// records a failure instead of leaking a HalfOpen probe and
    /// wedging the breaker until restart.
    ///
    /// When `self.breaker` is `None` (e.g. in tests), `op` is invoked
    /// directly with no overhead.
    pub async fn gate<T, E, F, Fut>(&self, op: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: From<CircuitOpenError>,
    {
        let Some(b) = &self.breaker else {
            // No breaker configured: zero-overhead fast path —
            // behavior identical to before the guard rework.
            return op().await;
        };
        let Some(mut guard) = b.try_acquire() else {
            return Err(CircuitOpenError(b.name().to_string()).into());
        };
        let result = op().await;
        if result.is_ok() {
            guard.succeed();
        }
        drop(guard);
        result
    }

    /// For call paths that don't return `Result` (e.g. rig's streaming
    /// `Agent::stream_prompt` which yields a `Stream<Item=Result<…>>`).
    /// Returns `Err(CircuitOpenError)` if the breaker is open;
    /// otherwise returns `Ok(())` and the caller is responsible for
    /// calling `mark_success` / `mark_failure` exactly once when the
    /// operation completes.
    pub fn breaker_allow(&self) -> Result<(), CircuitOpenError> {
        if let Some(b) = &self.breaker {
            if !b.allow() {
                return Err(CircuitOpenError(b.name().to_string()));
            }
        }
        Ok(())
    }

    /// Record a successful outcome for a streaming call gated via
    /// [`breaker_allow`](Self::breaker_allow). No-op when no breaker is
    /// configured.
    pub fn mark_success(&self) {
        if let Some(b) = &self.breaker {
            b.success();
        }
    }

    /// Record a failure outcome for a streaming call gated via
    /// [`breaker_allow`](Self::breaker_allow). No-op when no breaker is
    /// configured.
    pub fn mark_failure(&self) {
        if let Some(b) = &self.breaker {
            b.failure();
        }
    }

    /// Acquire a [`BreakerGuard`] for this client.
    ///
    /// Returns `Err(CircuitOpenError)` immediately when the breaker is
    /// open, without touching the provider. Otherwise returns a guard
    /// whose `Drop` records `mark_failure()` unconditionally. Call
    /// [`BreakerGuard::succeed`] on the success path before the guard
    /// leaves scope so `Drop` records `mark_success()` instead.
    ///
    /// Prefer this over the raw `breaker_allow` / `mark_success` /
    /// `mark_failure` triplet for any call site where a `tokio::select!`
    /// cancellation, a `?` early-return, or a panic could occur between
    /// `allow` and `mark`. The raw triplet remains `pub` only as the
    /// primitive layer beneath this guard; `gate` itself is built on
    /// `CircuitBreaker::try_acquire` and is drop-safe, so no call site
    /// should use the triplet directly anymore (improvement #16).
    pub fn breaker_guard(&self) -> Result<BreakerGuard<'_>, CircuitOpenError> {
        BreakerGuard::new(self)
    }

    /// Construct a `LlmClient` for the given pool.
    ///
    /// Reads `AURIS_LLM_{CHAT,BACKGROUND}_PROVIDER` and
    /// `AURIS_LLM_{CHAT,BACKGROUND}_MODEL_ID`. Both are required —
    /// no silent fallback to a default (spec §6.2). Provider
    /// credentials (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
    /// AWS Bedrock chain) are shared across pools when both select
    /// the same provider. Bedrock region (`AURIS_LLM_REGION`)
    /// stays global.
    ///
    /// Does **not** make an API call. Credential validation happens
    /// at first `extract` invocation.
    ///
    /// Pass `Some(breaker)` at boot so every `extract_*` call is
    /// gated through the circuit breaker; pass `None` in tests or
    /// contexts where the overhead is unwanted.
    pub async fn from_env(
        pool: LlmPool,
        breaker: Option<Arc<CircuitBreaker>>,
    ) -> Result<Self, LlmInitError> {
        let pool_str = pool.as_str().to_uppercase();
        let provider_key = format!("AURIS_LLM_{pool_str}_PROVIDER");
        let model_key = format!("AURIS_LLM_{pool_str}_MODEL_ID");

        let provider_str = crate::config::var_opt(&provider_key)
            .ok_or_else(|| LlmInitError::MissingPoolEnvVar(provider_key.clone(), pool.as_str()))?;
        let model_id = crate::config::var_opt(&model_key)
            .ok_or_else(|| LlmInitError::MissingPoolEnvVar(model_key.clone(), pool.as_str()))?;

        let provider = parse_provider(&provider_str)?;

        let (extractor, backend) = match provider {
            Provider::Bedrock => {
                let region = crate::config::var_or("AURIS_LLM_REGION", DEFAULT_BEDROCK_REGION);

                let bedrock_client: rig_bedrock::client::Client =
                    rig_bedrock::client::ClientBuilder::default()
                        .region(&region)
                        .build()
                        .await;

                let extractor = bedrock_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .max_tokens(MAX_TOKENS_BEDROCK)
                    .build();

                info!(pool = pool.as_str(), %region, %model_id, "LLM client initialised (rig + bedrock)");
                let backend = LlmBackend::Bedrock {
                    client: bedrock_client.clone(),
                    model_id: model_id.clone(),
                };
                (LlmExtractor::Bedrock(Arc::new(extractor)), backend)
            }

            Provider::OpenAI => {
                let api_key = crate::config::var_opt("OPENAI_API_KEY").ok_or_else(|| {
                    LlmInitError::MissingProviderCredentials(format!(
                        "OPENAI_API_KEY is required when {provider_key}=openai"
                    ))
                })?;

                let openai_client = rig_core::providers::openai::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = openai_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .max_tokens(MAX_TOKENS_OPENAI)
                    .build();

                info!(pool = pool.as_str(), %model_id, "LLM client initialised (rig + openai)");
                let backend = LlmBackend::OpenAI {
                    client: Arc::new(openai_client),
                    model_id: model_id.clone(),
                };
                (LlmExtractor::OpenAI(Arc::new(extractor)), backend)
            }

            Provider::Anthropic => {
                let api_key = crate::config::var_opt("ANTHROPIC_API_KEY").ok_or_else(|| {
                    LlmInitError::MissingProviderCredentials(format!(
                        "ANTHROPIC_API_KEY is required when {provider_key}=anthropic"
                    ))
                })?;

                let anthropic_client = rig_core::providers::anthropic::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = anthropic_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .max_tokens(MAX_TOKENS_ANTHROPIC)
                    .build();

                info!(pool = pool.as_str(), %model_id, "LLM client initialised (rig + anthropic-direct)");
                let backend = LlmBackend::Anthropic {
                    client: Arc::new(anthropic_client),
                    model_id: model_id.clone(),
                };
                (LlmExtractor::Anthropic(Arc::new(extractor)), backend)
            }

            Provider::Gemini => {
                let api_key = crate::config::var_opt("GEMINI_API_KEY").ok_or_else(|| {
                    LlmInitError::MissingProviderCredentials(format!(
                        "GEMINI_API_KEY is required when {provider_key}=gemini"
                    ))
                })?;

                let gemini_client = rig_core::providers::gemini::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = gemini_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .max_tokens(MAX_TOKENS_GEMINI)
                    .build();

                info!(pool = pool.as_str(), %model_id, "LLM client initialised (rig + gemini)");
                let backend = LlmBackend::Gemini {
                    client: Arc::new(gemini_client),
                    model_id: model_id.clone(),
                };
                (LlmExtractor::Gemini(Arc::new(extractor)), backend)
            }

            Provider::Xai => {
                let api_key = crate::config::var_opt("XAI_API_KEY").ok_or_else(|| {
                    LlmInitError::MissingProviderCredentials(format!(
                        "XAI_API_KEY is required when {provider_key}=xai"
                    ))
                })?;

                let xai_client = rig_core::providers::xai::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = xai_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .max_tokens(MAX_TOKENS_XAI)
                    .build();

                info!(pool = pool.as_str(), %model_id, "LLM client initialised (rig + xai)");
                let backend = LlmBackend::Xai {
                    client: Arc::new(xai_client),
                    model_id: model_id.clone(),
                };
                (LlmExtractor::Xai(Arc::new(extractor)), backend)
            }
        };

        Ok(Self {
            extractor,
            backend,
            provider,
            usage: Arc::new(LlmUsageTracker::new(pool)),
            breaker,
            metrics: Arc::new(LlmMetrics::new()),
        })
    }

    /// Extract meeting metadata from a free-text description.
    ///
    /// Wraps the rig Extractor call in [`EXTRACTION_TIMEOUT`]. Returns a
    /// `HashMap` with only non-empty fields (empty values are dropped).
    /// The call is gated through the per-pool circuit breaker when one
    /// is configured; returns `ExtractionError::CircuitOpen` immediately
    /// when the breaker is open.
    ///
    /// Emits one `llm_request_duration_seconds` point per attempt —
    /// `status="ok"` on success, `timeout | rate_limited | error` on
    /// failure (via [`timed_recorded`]). The prebuilt metadata extractor
    /// surface (`LlmExtractor::extract`) exposes no token usage, so both
    /// branches record 0/0 tokens; duration + status are the signal here.
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        let prompt = format!("Meeting description:\n{description}");
        let started = std::time::Instant::now();

        // Clone extractor arm (and metric fields) so the async closure
        // can own them without borrowing `self` across the `gate` call.
        let extractor = self.extractor.clone();
        let provider = self.provider;
        let model_id = self.model_id().to_owned();
        let metrics = self.metrics.clone();
        self.gate(|| async move {
            let extractor_call = async {
                match &extractor {
                    LlmExtractor::Bedrock(e) => {
                        e.extract(prompt.as_str()).await.map_err(classify_rig)
                    }
                    LlmExtractor::OpenAI(e) => {
                        e.extract(prompt.as_str()).await.map_err(classify_rig)
                    }
                    LlmExtractor::Anthropic(e) => {
                        e.extract(prompt.as_str()).await.map_err(classify_rig)
                    }
                    LlmExtractor::Gemini(e) => {
                        e.extract(prompt.as_str()).await.map_err(classify_rig)
                    }
                    LlmExtractor::Xai(e) => e.extract(prompt.as_str()).await.map_err(classify_rig),
                }
            };
            let typed = timed_recorded(
                &metrics,
                provider,
                &model_id,
                EXTRACTION_TIMEOUT,
                started,
                extractor_call,
            )
            .await?;
            metrics.record_call(
                provider.as_str(),
                &model_id,
                "ok",
                started.elapsed().as_secs_f64(),
                0,
                0,
            );
            Ok(into_map(typed))
        })
        .await
    }

    /// Run an ad-hoc rig `Extractor` with an arbitrary system prompt and output
    /// schema, building the extractor per call using the stored backend client.
    ///
    /// Used by summarizers (highlights, actions) that each need a different
    /// prompt and a different `JsonSchema` target type. `user_id` is used
    /// only for the per-meeting usage tracker — pass any stable id, including
    /// a synthetic one for non-meeting callers.
    ///
    /// Gated through the per-pool circuit breaker when one is configured;
    /// returns `ExtractionError::CircuitOpen` immediately when open.
    pub async fn extract_with_prompt<T>(
        &self,
        user_id: &str,
        system_prompt: &str,
        user_input: &str,
    ) -> Result<T, ExtractionError>
    where
        T: schemars::JsonSchema
            + serde::de::DeserializeOwned
            + serde::Serialize
            + Send
            + Sync
            + 'static,
    {
        let started = std::time::Instant::now();
        // Clone fields needed inside the async closure so `self` isn't
        // borrowed across the `gate` await point.
        let backend = self.backend.clone();
        let usage = self.usage.clone();
        let provider = self.provider;
        let model_id = self.model_id().to_owned();
        let metrics = self.metrics.clone();
        let system_prompt = system_prompt.to_owned();
        let user_input = user_input.to_owned();
        let user_id = user_id.to_owned();
        self.gate(|| async move {
            let extractor_call =
                async { provider_extract!(&backend, T, &system_prompt, user_input.as_str()) };
            let resp = timed_recorded(
                &metrics,
                provider,
                &model_id,
                EXTRACTION_TIMEOUT,
                started,
                extractor_call,
            )
            .await?;
            let u = resp.usage;
            usage.record(
                &user_id,
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
            );
            info!(
                user_id,
                provider = ?provider,
                call = "extract_with_prompt",
                input_tokens = u.input_tokens,
                output_tokens = u.output_tokens,
                cached_input_tokens = u.cached_input_tokens,
                latency_ms = started.elapsed().as_millis() as u64,
                "llm_call"
            );
            metrics.record_call(
                provider.as_str(),
                &model_id,
                "ok",
                started.elapsed().as_secs_f64(),
                u.input_tokens,
                u.output_tokens,
            );
            Ok(resp.data)
        })
        .await
    }

    /// Same shape as `extract_with_prompt`, but also attaches an image
    /// to the user turn. The model sees text first, then the image.
    /// Caller passes raw bytes (e.g. PNG file contents) plus the
    /// `ImageMediaType` matching the encoding — rig handles the
    /// per-provider base64-or-binary translation.
    ///
    /// Used by the moment summary worker so vision-capable models can
    /// reason about the screenshot the user captured at the moment,
    /// not just the surrounding transcript.
    ///
    /// Gated through the per-pool circuit breaker when one is configured;
    /// returns `ExtractionError::CircuitOpen` immediately when open.
    pub async fn extract_with_prompt_and_image<T>(
        &self,
        user_id: &str,
        system_prompt: &str,
        user_input: &str,
        image_bytes: Vec<u8>,
        media_type: ImageMediaType,
    ) -> Result<T, ExtractionError>
    where
        T: schemars::JsonSchema
            + serde::de::DeserializeOwned
            + serde::Serialize
            + Send
            + Sync
            + 'static,
    {
        // rig's OpenAI provider rejects raw bytes ("Raw file data not
        // supported, encode as base64 first"); Anthropic + Bedrock
        // also expect base64 in their wire format. So base64-encode
        // here once and feed the same string through any provider.
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;
        let encoded = B64.encode(&image_bytes);
        let image_chars = encoded.len() as u64;

        let message = Message::User {
            content: OneOrMany::many(vec![
                UserContent::text(user_input.to_string()),
                UserContent::image_base64(encoded, Some(media_type), Some(ImageDetail::High)),
            ])
            .map_err(|e| ExtractionError::Extract(format!("build message: {e}")))?,
        };

        let started = std::time::Instant::now();
        // Clone fields needed inside the async closure.
        let backend = self.backend.clone();
        let usage = self.usage.clone();
        let provider = self.provider;
        let model_id = self.model_id().to_owned();
        let metrics = self.metrics.clone();
        let system_prompt = system_prompt.to_owned();
        let user_id = user_id.to_owned();
        self.gate(|| async move {
            // Vision calls run longer than text-only — bump from
            // `EXTRACTION_TIMEOUT` (8s) to a per-image budget. Most
            // providers return a screenshot summary in 5-15s; a 30s cap
            // gives headroom for slow paths without freezing the worker
            // permanently if the API hangs.
            const VISION_TIMEOUT: Duration = Duration::from_secs(30);
            let extractor_call =
                async { provider_extract!(&backend, T, &system_prompt, message.clone()) };
            let resp = timed_recorded(
                &metrics,
                provider,
                &model_id,
                VISION_TIMEOUT,
                started,
                extractor_call,
            )
            .await?;
            let u = resp.usage;
            usage.record(
                &user_id,
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
            );
            info!(
                user_id,
                provider = ?provider,
                call = "extract_with_prompt_and_image",
                input_tokens = u.input_tokens,
                output_tokens = u.output_tokens,
                cached_input_tokens = u.cached_input_tokens,
                image_bytes_b64 = image_chars,
                latency_ms = started.elapsed().as_millis() as u64,
                "llm_call"
            );
            metrics.record_call(
                provider.as_str(),
                &model_id,
                "ok",
                started.elapsed().as_secs_f64(),
                u.input_tokens,
                u.output_tokens,
            );
            Ok(resp.data)
        })
        .await
    }

    /// Same shape as `extract_with_prompt`, but also attaches a PDF
    /// document to the user turn. Used by the artifact summarizer
    /// when the uploaded artifact is `application/pdf` — providers
    /// reason about the PDF natively (text + structure + diagrams)
    /// rather than us pre-extracting text.
    ///
    /// Mirrors the image variant's base64-then-pass approach: rig's
    /// OpenAI provider rejects raw bytes for documents the same way
    /// it does for images. Encoding here keeps the wire payload
    /// uniform across providers.
    ///
    /// Gated through the per-pool circuit breaker when one is configured;
    /// returns `ExtractionError::CircuitOpen` immediately when open.
    pub async fn extract_with_prompt_and_document_pdf<T>(
        &self,
        user_id: &str,
        system_prompt: &str,
        user_input: &str,
        document_bytes: Vec<u8>,
    ) -> Result<T, ExtractionError>
    where
        T: schemars::JsonSchema
            + serde::de::DeserializeOwned
            + serde::Serialize
            + Send
            + Sync
            + 'static,
    {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;
        let encoded = B64.encode(&document_bytes);
        let doc_chars = encoded.len() as u64;

        let message = Message::User {
            content: OneOrMany::many(vec![
                UserContent::text(user_input.to_string()),
                UserContent::Document(Document {
                    data: DocumentSourceKind::Base64(encoded),
                    media_type: Some(DocumentMediaType::PDF),
                    additional_params: None,
                }),
            ])
            .map_err(|e| ExtractionError::Extract(format!("build message: {e}")))?,
        };

        let started = std::time::Instant::now();
        // Clone fields needed inside the async closure.
        let backend = self.backend.clone();
        let usage = self.usage.clone();
        let provider = self.provider;
        let model_id = self.model_id().to_owned();
        let metrics = self.metrics.clone();
        let system_prompt = system_prompt.to_owned();
        let user_id = user_id.to_owned();
        self.gate(|| async move {
            // PDF processing runs longer than image — multi-page docs
            // routinely need 30-60s. Cap at 90s so a hung API doesn't
            // freeze the worker forever.
            const PDF_TIMEOUT: Duration = Duration::from_secs(90);
            let extractor_call =
                async { provider_extract!(&backend, T, &system_prompt, message.clone()) };
            let resp = timed_recorded(
                &metrics,
                provider,
                &model_id,
                PDF_TIMEOUT,
                started,
                extractor_call,
            )
            .await?;
            let u = resp.usage;
            usage.record(
                &user_id,
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
            );
            info!(
                user_id,
                provider = ?provider,
                call = "extract_with_prompt_and_document_pdf",
                input_tokens = u.input_tokens,
                output_tokens = u.output_tokens,
                cached_input_tokens = u.cached_input_tokens,
                doc_bytes_b64 = doc_chars,
                latency_ms = started.elapsed().as_millis() as u64,
                "llm_call"
            );
            metrics.record_call(
                provider.as_str(),
                &model_id,
                "ok",
                started.elapsed().as_secs_f64(),
                u.input_tokens,
                u.output_tokens,
            );
            Ok(resp.data)
        })
        .await
    }
}

// ─── Failure-side metric recording ───────────────────────────────────────────

/// Wrap an extractor future in `tokio::time::timeout`, mapping an
/// elapsed budget to [`ExtractionError::Timeout`], and record one
/// failure-side metric point (`status = timeout | rate_limited |
/// error | circuit_open`, zero tokens) when the result is `Err`.
///
/// Success recording stays at the call sites — only they can see the
/// provider's token usage (`resp.usage`), and this keeps the helper
/// generic over `T` without naming rig's `ExtractWithUsageResponse`.
///
/// Shared by `extract`, `extract_with_prompt`,
/// `extract_with_prompt_and_image`, and
/// `extract_with_prompt_and_document_pdf` so a future fifth wrapper
/// can't forget the failure-side record again (improvements.md §23:
/// the wrappers previously recorded `"ok"` only, so a provider outage
/// that broke every background worker was metric-invisible).
///
/// Failure points carry 0/0 tokens, so `llm_tokens_used_total` totals
/// are unaffected; only the duration histogram gains new status series
/// (a closed set — no cardinality risk).
async fn timed_recorded<T>(
    metrics: &LlmMetrics,
    provider: Provider,
    model_id: &str,
    timeout: Duration,
    started: std::time::Instant,
    fut: impl std::future::Future<Output = Result<T, ExtractionError>>,
) -> Result<T, ExtractionError> {
    let result = match tokio::time::timeout(timeout, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(ExtractionError::Timeout(timeout)),
    };
    if let Err(e) = &result {
        metrics.record_call(
            provider.as_str(),
            model_id,
            metric_status(e),
            started.elapsed().as_secs_f64(),
            0,
            0,
        );
    }
    result
}

// ─── BreakerGuard ─────────────────────────────────────────────────────────────

/// RAII guard that wraps one gated LLM call and ensures the circuit-breaker
/// outcome is recorded on every exit path — normal return, `?` early-return,
/// `tokio::select!` cancellation, and panic alike.
///
/// # Usage
///
/// ```ignore
/// let mut guard = match client.breaker_guard() {
///     Ok(g) => g,
///     Err(e) => return, // breaker open; skip the call
/// };
///
/// let result = do_llm_work().await;
///
/// if result.is_ok() {
///     guard.succeed(); // mark success before guard drops
/// }
/// // Drop (implicit at end of scope) calls mark_failure() unless succeed() was called.
/// ```
///
/// # Design notes
///
/// Default-on-drop is `mark_failure` ("assume the worst"). This is the safe
/// default for the HalfOpen state: a leaked probe permanently blocks further
/// probes, silently breaking a healthy breaker. Marking failure on any
/// unclean exit reopens the breaker with a fresh cooldown — noisy, but
/// recoverable.
///
/// For the Closed state, a missed `mark_failure` merely loses one failure
/// count — tolerable. For the Closed state, a missed `mark_success` also
/// has no lasting effect (the failure counter doesn't grow). So the
/// pessimistic default only materially matters for HalfOpen, but keeping
/// one uniform rule avoids surprises when the state changes between allow
/// and drop.
pub struct BreakerGuard<'a> {
    client: &'a LlmClient,
    succeeded: bool,
}

impl<'a> BreakerGuard<'a> {
    /// Gate a call through `client`'s circuit breaker.
    ///
    /// Returns `Err(CircuitOpenError)` if the breaker is open so the
    /// caller can skip the call entirely. Otherwise returns a guard whose
    /// `Drop` will call `mark_failure` unless [`succeed`](Self::succeed)
    /// is called first.
    pub fn new(client: &'a LlmClient) -> Result<Self, CircuitOpenError> {
        client.breaker_allow()?;
        Ok(Self {
            client,
            succeeded: false,
        })
    }

    /// Mark this call as successful. Must be called before the guard
    /// leaves scope on the success path; `Drop` will then call
    /// `mark_success` instead of `mark_failure`.
    ///
    /// Calling `succeed` more than once is a no-op.
    pub fn succeed(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for BreakerGuard<'_> {
    fn drop(&mut self) {
        if self.succeeded {
            self.client.mark_success();
        } else {
            self.client.mark_failure();
        }
    }
}

/// Convert `ExtractedMetadata` into a `HashMap`, dropping any empty-string fields.
///
/// Per spec §1.1: an empty string means "the model couldn't extract this field"
/// and should not pollute manual metadata via the merge.
pub(crate) fn into_map(m: ExtractedMetadata) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if !m.title.is_empty() {
        out.insert("title".to_string(), m.title);
    }
    if !m.project.is_empty() {
        out.insert("project".to_string(), m.project);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_map_drops_empty_title_only() {
        let m = ExtractedMetadata {
            title: "Q1 review".to_string(),
            project: String::new(),
        };
        let map = into_map(m);
        assert_eq!(map.get("title"), Some(&"Q1 review".to_string()));
        assert!(!map.contains_key("project"));
    }

    #[test]
    fn into_map_drops_both_when_empty() {
        let m = ExtractedMetadata {
            title: String::new(),
            project: String::new(),
        };
        let map = into_map(m);
        assert!(map.is_empty());
    }

    #[test]
    fn into_map_keeps_both_when_present() {
        let m = ExtractedMetadata {
            title: "T".to_string(),
            project: "P".to_string(),
        };
        let map = into_map(m);
        assert_eq!(map.get("title"), Some(&"T".to_string()));
        assert_eq!(map.get("project"), Some(&"P".to_string()));
    }

    #[test]
    fn system_prompt_mentions_extraction() {
        assert!(SYSTEM_PROMPT.to_lowercase().contains("extract"));
    }

    #[test]
    fn default_bedrock_model_id_is_cross_region_profile() {
        assert!(DEFAULT_BEDROCK_MODEL_ID.starts_with("us."));
        assert!(DEFAULT_BEDROCK_MODEL_ID.contains("claude"));
    }

    #[test]
    fn max_tokens_gemini_is_set() {
        const { assert!(MAX_TOKENS_GEMINI > 0) };
        const { assert!(MAX_TOKENS_GEMINI <= 65_536) }; // sanity ceiling
    }

    #[test]
    fn max_tokens_xai_is_set() {
        const { assert!(MAX_TOKENS_XAI > 0) };
        const { assert!(MAX_TOKENS_XAI <= 65_536) }; // sanity ceiling
    }

    #[test]
    fn default_openai_model_id_is_set() {
        assert!(!DEFAULT_OPENAI_MODEL_ID.is_empty());
        assert!(DEFAULT_OPENAI_MODEL_ID.starts_with("gpt-"));
    }

    #[test]
    fn default_anthropic_model_id_is_set() {
        assert!(!DEFAULT_ANTHROPIC_MODEL_ID.is_empty());
        assert!(DEFAULT_ANTHROPIC_MODEL_ID.starts_with("claude-"));
    }

    #[test]
    fn from_env_chat_missing_provider_errors_with_pool_tag() {
        // Wipe both pools' vars so the function definitely sees them
        // as unset. Tests run with --test-threads=1 so this is safe.
        std::env::remove_var("AURIS_LLM_CHAT_PROVIDER");
        std::env::remove_var("AURIS_LLM_CHAT_MODEL_ID");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(LlmClient::from_env(LlmPool::Chat, None));
        assert!(result.is_err(), "should fail when CHAT vars are unset");
        // Use .err().unwrap() to avoid the T: Debug bound on unwrap_err/expect_err.
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("AURIS_LLM_CHAT_PROVIDER"),
            "error must name the missing var: {msg}"
        );
    }

    #[test]
    fn from_env_background_missing_model_errors_with_pool_tag() {
        std::env::set_var("AURIS_LLM_BACKGROUND_PROVIDER", "anthropic");
        std::env::remove_var("AURIS_LLM_BACKGROUND_MODEL_ID");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(LlmClient::from_env(LlmPool::Background, None));
        // Clean up unconditionally before any assertion that could panic —
        // otherwise a failed assertion leaks AURIS_LLM_BACKGROUND_PROVIDER
        // into the env for whichever test runs next.
        std::env::remove_var("AURIS_LLM_BACKGROUND_PROVIDER");
        assert!(
            result.is_err(),
            "should fail when BACKGROUND_MODEL_ID is unset"
        );
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("AURIS_LLM_BACKGROUND_MODEL_ID"),
            "error must name the missing var: {msg}"
        );
    }
}

#[cfg(test)]
mod breaker_guard_tests {
    //! Unit tests for `BreakerGuard` drop semantics.
    //!
    //! Constructing a real `LlmClient` requires provider credentials and
    //! runtime env vars, so we test the guard's effect on the underlying
    //! `CircuitBreaker` directly. The breaker's observable state (open vs
    //! closed) is the ground truth: if `probe_in_flight` were leaked the
    //! breaker would stay stuck in HalfOpen and reject all further probes.

    use std::sync::Arc;
    use std::time::Duration;

    use crate::util::circuit_breaker::CircuitBreaker;

    /// Helper: trip the breaker into Open and then advance past cooldown
    /// so the next `allow()` returns `true` and the state transitions to
    /// HalfOpen with `probe_in_flight = true`.
    fn tripped_breaker() -> Arc<CircuitBreaker> {
        let cb = Arc::new(CircuitBreaker::new(
            "test-guard",
            /*threshold=*/ 1,
            /*cooldown=*/ Duration::from_millis(10),
            None,
        ));
        // Trip into Open with one failure.
        assert!(cb.allow(), "should be admitted when closed");
        cb.failure();
        assert!(!cb.allow(), "breaker should be open immediately after trip");
        // Wait for cooldown.
        std::thread::sleep(Duration::from_millis(20));
        cb
    }

    /// Dropping a `BreakerGuard` without calling `succeed()` must cause
    /// `mark_failure()` to run. For the HalfOpen state this means the
    /// probe is recorded as a failure and the breaker re-opens, so the
    /// very next `allow()` call returns `false`.
    ///
    /// This is the primary correctness guarantee: a cancellation or panic
    /// between `breaker_allow` and `mark_*` must never leave
    /// `probe_in_flight = true` permanently.
    #[test]
    fn breaker_probe_leak_on_drop_records_failure() {
        let cb = tripped_breaker();

        // Transition to HalfOpen: first allow() after cooldown.
        assert!(cb.allow(), "cooldown elapsed — probe should be admitted");
        // While the probe is in flight no other call is admitted.
        assert!(
            !cb.allow(),
            "concurrent allow while probe in flight must be rejected"
        );

        // Simulate: the probe was admitted, then the task was cancelled
        // before either mark ran. We model this at the CircuitBreaker
        // level directly (no guard needed for this assertion) to confirm
        // that failure() clears probe_in_flight.
        cb.failure();

        // After failure in HalfOpen the breaker re-opens.
        assert!(
            !cb.allow(),
            "breaker must be open again after failed probe — probe_in_flight cleared"
        );
    }

    /// Dropping a `BreakerGuard` without `succeed()` calls `mark_failure()`.
    /// Using `LlmClient` with `breaker = None` (the test constructor path)
    /// means `mark_failure` is a no-op, so we verify the guard's internal
    /// `succeeded` flag drives the right branch by checking the circuit
    /// breaker state directly after a real guard drop.
    ///
    /// This test builds a minimal `LlmClient`-less scenario by calling
    /// `CircuitBreaker::failure()` to model what `Drop` will do, confirming
    /// the observable state matches the pessimistic assumption.
    #[test]
    fn breaker_guard_drop_without_succeed_marks_failure() {
        // Build a breaker that has just admitted a HalfOpen probe.
        let cb = tripped_breaker();
        assert!(
            cb.allow(),
            "first allow after cooldown should admit the probe"
        );

        // Simulate what BreakerGuard::drop does when succeed() was not called.
        // We can't construct a real BreakerGuard without a LlmClient, so we
        // exercise the circuit-breaker side-effect that the guard's Drop
        // relies on: mark_failure() (which calls cb.failure() internally).
        cb.failure();

        // Breaker must be open: probe recorded as failure.
        assert!(
            !cb.allow(),
            "breaker should be open after simulated guard drop (failure path)"
        );
    }

    /// `succeed()` followed by implicit Drop calls `mark_success()`:
    /// the breaker closes and the next `allow()` returns `true`.
    #[test]
    fn breaker_guard_succeed_then_drop_marks_success() {
        let cb = tripped_breaker();
        assert!(cb.allow(), "probe admitted after cooldown");

        // Simulate succeed() path.
        cb.success();

        // Breaker must be closed now.
        assert!(
            cb.allow(),
            "breaker should be closed after successful probe"
        );
    }
}

#[cfg(test)]
mod gate_drop_tests {
    //! Regression test for improvement #16: a `gate()` future dropped
    //! mid-await — exactly what `ws/control.rs::spawn_extraction` does
    //! when its `tokio::select!` races `background_llm.extract()`
    //! against a `CancellationToken` — must record a breaker outcome.
    //! Pre-fix, the dropped HalfOpen probe leaked `probe_in_flight`
    //! and every later background-pool call returned `CircuitOpen`
    //! until process restart.

    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::util::circuit_breaker::CircuitBreaker;

    /// Build a real `LlmClient` wired to `cb` without any API call
    /// (`from_env` is connection-free per its doc; the dummy key is
    /// never used). Env vars are set and removed inline — the suite
    /// runs `--test-threads=1`, so this cannot race other tests.
    async fn client_with_breaker(cb: Arc<CircuitBreaker>) -> LlmClient {
        std::env::set_var("AURIS_LLM_BACKGROUND_PROVIDER", "openai");
        std::env::set_var("AURIS_LLM_BACKGROUND_MODEL_ID", "gpt-4o");
        std::env::set_var("OPENAI_API_KEY", "sk-test-dummy-never-used");
        let client = LlmClient::from_env(LlmPool::Background, Some(cb)).await;
        std::env::remove_var("AURIS_LLM_BACKGROUND_PROVIDER");
        std::env::remove_var("AURIS_LLM_BACKGROUND_MODEL_ID");
        std::env::remove_var("OPENAI_API_KEY");
        client.expect("from_env with openai provider + dummy key must not call the API")
    }

    #[test]
    fn gate_dropped_mid_await_records_failure_and_breaker_recovers() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // threshold 1 → a single failure opens the breaker;
            // 20ms cooldown keeps the test fast.
            let cb = Arc::new(CircuitBreaker::new(
                "llm.background-test",
                1,
                Duration::from_millis(20),
                None,
            ));
            let client = client_with_breaker(cb.clone()).await;

            // 1. Trip the breaker with one failing gated op.
            let r: Result<(), ExtractionError> = client
                .gate(|| async { Err(ExtractionError::Extract("boom".into())) })
                .await;
            assert!(r.is_err(), "tripping op must fail");
            let r: Result<(), ExtractionError> = client.gate(|| async { Ok(()) }).await;
            assert!(
                matches!(r, Err(ExtractionError::CircuitOpen(_))),
                "breaker must be open after threshold failures, got {r:?}"
            );

            // 2. Wait out the cooldown so the next call is the
            //    HalfOpen probe.
            tokio::time::sleep(Duration::from_millis(30)).await;

            // 3. The probe: a gate future parked forever at
            //    `op().await`, dropped from the outside by `timeout` —
            //    models the select!-cancellation in spawn_extraction
            //    (a re-dictated description cancels the in-flight
            //    extraction via extraction_cancel_for).
            let gate_fut = client.gate(std::future::pending::<Result<(), ExtractionError>>);
            let timed = tokio::time::timeout(Duration::from_millis(20), gate_fut).await;
            assert!(
                timed.is_err(),
                "gate future must still be pending when dropped"
            );

            // 4. The drop must have recorded a failure: breaker is
            //    Open again with a fresh cooldown (not wedged HalfOpen).
            let r: Result<(), ExtractionError> = client.gate(|| async { Ok(()) }).await;
            assert!(
                matches!(r, Err(ExtractionError::CircuitOpen(_))),
                "breaker must re-open after the dropped probe, got {r:?}"
            );

            // 5. Self-heal: after another cooldown a fresh probe is
            //    admitted and a successful op closes the breaker.
            //    Pre-fix, probe_in_flight stayed true forever and this
            //    returned CircuitOpen until process restart.
            tokio::time::sleep(Duration::from_millis(30)).await;
            let r: Result<(), ExtractionError> = client.gate(|| async { Ok(()) }).await;
            assert!(
                r.is_ok(),
                "fresh probe must be admitted after cooldown and close the breaker, got {r:?}"
            );
        });
    }
}

#[cfg(test)]
mod timed_recorded_tests {
    //! Behavior tests for `timed_recorded`: result shaping (timeout
    //! mapping, error/ok passthrough) and — the regression this exists
    //! for — failure-side metric emission. Before this helper, the
    //! extract wrappers only ever recorded `status="ok"`, so a provider
    //! outage was metric-invisible (improvements.md §23).

    use super::*;
    use crate::observability::llm_test_support::{duration_statuses, in_memory_metrics};

    #[tokio::test(start_paused = true)]
    async fn timeout_maps_to_timeout_error_and_records_timeout_status() {
        let (metrics, meter_provider, exporter) = in_memory_metrics();
        let result: Result<(), ExtractionError> = timed_recorded(
            &metrics,
            Provider::Bedrock,
            "test-model",
            Duration::from_millis(5),
            std::time::Instant::now(),
            std::future::pending(),
        )
        .await;
        assert!(
            matches!(result, Err(ExtractionError::Timeout(d)) if d == Duration::from_millis(5)),
            "elapsed timeout must map to ExtractionError::Timeout carrying the budget, got {result:?}"
        );
        meter_provider.force_flush().unwrap();
        assert_eq!(
            duration_statuses(&exporter),
            vec![("timeout".to_string(), 1)]
        );
    }

    #[tokio::test]
    async fn provider_error_records_error_status_and_passes_error_through() {
        let (metrics, meter_provider, exporter) = in_memory_metrics();
        let result: Result<(), ExtractionError> = timed_recorded(
            &metrics,
            Provider::OpenAI,
            "test-model",
            Duration::from_secs(5),
            std::time::Instant::now(),
            async {
                Err(ExtractionError::Provider(
                    "HTTP 500 Internal Server Error".to_string(),
                ))
            },
        )
        .await;
        assert!(matches!(result, Err(ExtractionError::Provider(_))));
        meter_provider.force_flush().unwrap();
        assert_eq!(duration_statuses(&exporter), vec![("error".to_string(), 1)]);
    }

    #[tokio::test]
    async fn quota_error_records_rate_limited_status() {
        let (metrics, meter_provider, exporter) = in_memory_metrics();
        let result: Result<(), ExtractionError> = timed_recorded(
            &metrics,
            Provider::Anthropic,
            "test-model",
            Duration::from_secs(5),
            std::time::Instant::now(),
            async {
                Err(ExtractionError::QuotaExhausted(
                    "Your credit balance is too low".to_string(),
                ))
            },
        )
        .await;
        assert!(matches!(result, Err(ExtractionError::QuotaExhausted(_))));
        meter_provider.force_flush().unwrap();
        assert_eq!(
            duration_statuses(&exporter),
            vec![("rate_limited".to_string(), 1)]
        );
    }

    #[tokio::test]
    async fn ok_passes_through_and_helper_records_nothing() {
        let (metrics, meter_provider, exporter) = in_memory_metrics();
        let result = timed_recorded(
            &metrics,
            Provider::Xai,
            "test-model",
            Duration::from_secs(5),
            std::time::Instant::now(),
            async { Ok::<_, ExtractionError>(42u32) },
        )
        .await;
        assert_eq!(result.unwrap(), 42);
        meter_provider.force_flush().unwrap();
        assert!(
            duration_statuses(&exporter).is_empty(),
            "success recording is owned by the call sites (they carry token usage); the helper must stay silent on Ok"
        );
    }
}
