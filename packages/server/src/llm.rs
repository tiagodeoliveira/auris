//! LLM-based metadata extraction via rig + rig-bedrock.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rig::completion::message::{
    Document, DocumentMediaType, DocumentSourceKind, ImageDetail, ImageMediaType, Message,
    UserContent,
};
use rig::extractor::Extractor;
use rig::prelude::*;
use rig::OneOrMany;
use rig_bedrock::client::Client as BedrockClient;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

// ─── Type aliases for model types ────────────────────────────────────────────
type BedrockModel = rig_bedrock::completion::CompletionModel;
type OpenAIModel = rig::providers::openai::responses_api::ResponsesCompletionModel;
type AnthropicModel = rig::providers::anthropic::completion::CompletionModel;

// ─── Type aliases for raw client types ───────────────────────────────────────
type OpenAIClientWrapper = rig::providers::openai::Client;
type AnthropicClientWrapper = rig::providers::anthropic::Client;

// ─── Constants ────────────────────────────────────────────────────────────────
pub const DEFAULT_BEDROCK_REGION: &str = "us-west-2";
pub const DEFAULT_BEDROCK_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
// gpt-4o is materially better than gpt-4.1-mini at multi-tool
// reasoning (the agent loop's core need); gpt-4.1-mini struggled
// to follow the dedup + mode-discrimination rules. Override with
// `MEETING_COMPANION_LLM_MODEL_ID` if cost-sensitive.
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
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

// ─── Provider discriminant ───────────────────────────────────────────────────

/// Which LLM backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Bedrock,
    OpenAI,
    Anthropic,
}

// ─── Extractor enum ──────────────────────────────────────────────────────────

/// Enum wrapping a typed rig `Extractor` for each supported provider.
///
/// `pub(crate)` — callers only interact with `LlmClient`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LlmExtractor {
    Bedrock(Arc<Extractor<BedrockModel, ExtractedMetadata>>),
    OpenAI(Arc<Extractor<OpenAIModel, ExtractedMetadata>>),
    Anthropic(Arc<Extractor<AnthropicModel, ExtractedMetadata>>),
}

// Manual Clone: Arc makes each variant cheap to clone.
impl Clone for LlmExtractor {
    fn clone(&self) -> Self {
        match self {
            Self::Bedrock(e) => Self::Bedrock(Arc::clone(e)),
            Self::OpenAI(e) => Self::OpenAI(Arc::clone(e)),
            Self::Anthropic(e) => Self::Anthropic(Arc::clone(e)),
        }
    }
}

// ─── Backend enum (raw clients for ad-hoc extraction) ────────────────────────

/// Stores the raw rig clients and model IDs for building ad-hoc extractors.
///
/// All three client types are `Clone` (they wrap `Arc` internally), so this
/// enum can be cheaply cloned alongside `LlmClient`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LlmBackend {
    Bedrock {
        client: BedrockClient,
        model_id: String,
    },
    OpenAI {
        client: Arc<OpenAIClientWrapper>,
        model_id: String,
    },
    Anthropic {
        client: Arc<AnthropicClientWrapper>,
        model_id: String,
    },
}

impl Clone for LlmBackend {
    fn clone(&self) -> Self {
        match self {
            Self::Bedrock { client, model_id } => Self::Bedrock {
                client: client.clone(),
                model_id: model_id.clone(),
            },
            Self::OpenAI { client, model_id } => Self::OpenAI {
                client: Arc::clone(client),
                model_id: model_id.clone(),
            },
            Self::Anthropic { client, model_id } => Self::Anthropic {
                client: Arc::clone(client),
                model_id: model_id.clone(),
            },
        }
    }
}

// ─── Usage tracker ───────────────────────────────────────────────────────────

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
#[derive(Debug, Default)]
pub struct LlmUsageTracker {
    inner: std::sync::Mutex<HashMap<String, LlmUsage>>,
}

impl LlmUsageTracker {
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

// ─── Error types ─────────────────────────────────────────────────────────────

/// Structured extraction target for meeting metadata.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExtractedMetadata {
    /// Concise meeting title in 8 words or fewer. Empty string if not extractable.
    pub title: String,

    /// Project name if mentioned. Empty string if not extractable.
    pub project: String,
}

/// Errors that can occur during LLM client initialisation.
#[derive(Debug, Error)]
pub enum LlmInitError {
    #[error("LLM provider init failed: {0}")]
    Provider(String),

    #[error("Unknown LLM provider: '{0}'. Accepted values: bedrock, openai, anthropic")]
    UnknownProvider(String),

    #[error("Missing credentials for provider '{0}'. Check the required env var.")]
    MissingProviderCredentials(String),
}

/// Errors that can occur during metadata extraction.
#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("LLM call exceeded timeout of {0:?}")]
    Timeout(Duration),

    #[error("Extraction failed: {0}")]
    Extract(String),
}

// ─── Provider parsing ─────────────────────────────────────────────────────────

/// Parse a provider name string (case-insensitive) into a [`Provider`].
fn parse_provider(s: &str) -> Result<Provider, LlmInitError> {
    match s.to_ascii_lowercase().as_str() {
        "bedrock" => Ok(Provider::Bedrock),
        "openai" => Ok(Provider::OpenAI),
        "anthropic" => Ok(Provider::Anthropic),
        _ => Err(LlmInitError::UnknownProvider(s.to_string())),
    }
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

    /// Construct a `LlmClient` from environment variables.
    ///
    /// Reads `MEETING_COMPANION_LLM_PROVIDER` (default: `bedrock`) and
    /// provider-specific configuration variables.
    ///
    /// Does **not** make an API call. Credential validation happens at first
    /// `extract` invocation.
    pub async fn from_env() -> Result<Self, LlmInitError> {
        let provider_str = std::env::var("MEETING_COMPANION_LLM_PROVIDER")
            .unwrap_or_else(|_| "bedrock".to_string());
        let provider = parse_provider(&provider_str)?;

        let (extractor, backend) = match provider {
            Provider::Bedrock => {
                let region = std::env::var("MEETING_COMPANION_LLM_REGION")
                    .unwrap_or_else(|_| DEFAULT_BEDROCK_REGION.to_string());
                let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
                    .unwrap_or_else(|_| DEFAULT_BEDROCK_MODEL_ID.to_string());

                let bedrock_client: BedrockClient = rig_bedrock::client::ClientBuilder::default()
                    .region(&region)
                    .build()
                    .await;

                let extractor = bedrock_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .build();

                info!(%region, %model_id, "LLM client initialised (rig + bedrock)");
                let backend = LlmBackend::Bedrock {
                    client: bedrock_client.clone(),
                    model_id,
                };
                (LlmExtractor::Bedrock(Arc::new(extractor)), backend)
            }

            Provider::OpenAI => {
                let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
                    LlmInitError::MissingProviderCredentials(
                        "OPENAI_API_KEY is required when MEETING_COMPANION_LLM_PROVIDER=openai"
                            .to_string(),
                    )
                })?;
                let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
                    .unwrap_or_else(|_| DEFAULT_OPENAI_MODEL_ID.to_string());

                let openai_client = rig::providers::openai::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = openai_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .build();

                info!(%model_id, "LLM client initialised (rig + openai)");
                let backend = LlmBackend::OpenAI {
                    client: Arc::new(openai_client),
                    model_id,
                };
                (LlmExtractor::OpenAI(Arc::new(extractor)), backend)
            }

            Provider::Anthropic => {
                let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
                    LlmInitError::MissingProviderCredentials(
                        "ANTHROPIC_API_KEY is required when MEETING_COMPANION_LLM_PROVIDER=anthropic"
                            .to_string(),
                    )
                })?;
                let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
                    .unwrap_or_else(|_| DEFAULT_ANTHROPIC_MODEL_ID.to_string());

                let anthropic_client = rig::providers::anthropic::Client::new(&api_key)
                    .map_err(|e| LlmInitError::Provider(e.to_string()))?;

                let extractor = anthropic_client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .build();

                info!(%model_id, "LLM client initialised (rig + anthropic-direct)");
                let backend = LlmBackend::Anthropic {
                    client: Arc::new(anthropic_client),
                    model_id,
                };
                (LlmExtractor::Anthropic(Arc::new(extractor)), backend)
            }
        };

        Ok(Self {
            extractor,
            backend,
            provider,
            usage: Arc::new(LlmUsageTracker::default()),
        })
    }

    /// Extract meeting metadata from a free-text description.
    ///
    /// Wraps the rig Extractor call in an 8-second timeout. Returns a
    /// `HashMap` with only non-empty fields (empty values are dropped).
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        let prompt = format!("Meeting description:\n{description}");

        let typed = match &self.extractor {
            LlmExtractor::Bedrock(e) => {
                tokio::time::timeout(EXTRACTION_TIMEOUT, e.extract(prompt.as_str()))
                    .await
                    .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                    .map_err(|e| ExtractionError::Extract(e.to_string()))?
            }
            LlmExtractor::OpenAI(e) => {
                tokio::time::timeout(EXTRACTION_TIMEOUT, e.extract(prompt.as_str()))
                    .await
                    .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                    .map_err(|e| ExtractionError::Extract(e.to_string()))?
            }
            LlmExtractor::Anthropic(e) => {
                tokio::time::timeout(EXTRACTION_TIMEOUT, e.extract(prompt.as_str()))
                    .await
                    .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                    .map_err(|e| ExtractionError::Extract(e.to_string()))?
            }
        };

        Ok(into_map(typed))
    }

    /// Run an ad-hoc rig `Extractor` with an arbitrary system prompt and output
    /// schema, building the extractor per call using the stored backend client.
    ///
    /// Used by summarizers (highlights, actions) that each need a different
    /// prompt and a different `JsonSchema` target type. `user_id` is used
    /// only for the per-meeting usage tracker — pass any stable id, including
    /// a synthetic one for non-meeting callers.
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
        let extractor_call = async {
            match &self.backend {
                LlmBackend::Bedrock { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::OpenAI { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::Anthropic { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
            }
        };
        let resp = tokio::time::timeout(EXTRACTION_TIMEOUT, extractor_call)
            .await
            .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))??;
        let usage = resp.usage;
        self.usage.record(
            user_id,
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_input_tokens,
        );
        info!(
            user_id,
            provider = ?self.provider,
            call = "extract_with_prompt",
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            latency_ms = started.elapsed().as_millis() as u64,
            "llm_call"
        );
        Ok(resp.data)
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
        let extractor_call = async {
            match &self.backend {
                LlmBackend::Bedrock { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::OpenAI { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::Anthropic { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
            }
        };
        // Vision calls run longer than text-only — bump from
        // `EXTRACTION_TIMEOUT` (8s) to a per-image budget. Most
        // providers return a screenshot summary in 5-15s; a 30s cap
        // gives headroom for slow paths without freezing the worker
        // permanently if the API hangs.
        const VISION_TIMEOUT: Duration = Duration::from_secs(30);
        let resp = tokio::time::timeout(VISION_TIMEOUT, extractor_call)
            .await
            .map_err(|_| ExtractionError::Timeout(VISION_TIMEOUT))??;
        let usage = resp.usage;
        self.usage.record(
            user_id,
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_input_tokens,
        );
        info!(
            user_id,
            provider = ?self.provider,
            call = "extract_with_prompt_and_image",
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            image_bytes_b64 = image_chars,
            latency_ms = started.elapsed().as_millis() as u64,
            "llm_call"
        );
        Ok(resp.data)
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
        let extractor_call = async {
            match &self.backend {
                LlmBackend::Bedrock { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::OpenAI { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::Anthropic { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract_with_usage(message.clone())
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
            }
        };
        // PDF processing runs longer than image — multi-page docs
        // routinely need 30-60s. Cap at 90s so a hung API doesn't
        // freeze the worker forever.
        const PDF_TIMEOUT: Duration = Duration::from_secs(90);
        let resp = tokio::time::timeout(PDF_TIMEOUT, extractor_call)
            .await
            .map_err(|_| ExtractionError::Timeout(PDF_TIMEOUT))??;
        let usage = resp.usage;
        self.usage.record(
            user_id,
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_input_tokens,
        );
        info!(
            user_id,
            provider = ?self.provider,
            call = "extract_with_prompt_and_document_pdf",
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            doc_bytes_b64 = doc_chars,
            latency_ms = started.elapsed().as_millis() as u64,
            "llm_call"
        );
        Ok(resp.data)
    }
}

/// Convert `ExtractedMetadata` into a `HashMap`, dropping any empty-string fields.
///
/// Per spec §1.1: an empty string means "the model couldn't extract this field"
/// and should not pollute manual metadata via the merge.
fn into_map(m: ExtractedMetadata) -> HashMap<String, String> {
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
    fn parse_provider_accepts_bedrock() {
        assert_eq!(parse_provider("bedrock").unwrap(), Provider::Bedrock);
    }

    #[test]
    fn parse_provider_accepts_openai() {
        assert_eq!(parse_provider("openai").unwrap(), Provider::OpenAI);
    }

    #[test]
    fn parse_provider_accepts_anthropic() {
        assert_eq!(parse_provider("anthropic").unwrap(), Provider::Anthropic);
    }

    #[test]
    fn parse_provider_is_case_insensitive() {
        assert_eq!(parse_provider("OpenAI").unwrap(), Provider::OpenAI);
        assert_eq!(parse_provider("BEDROCK").unwrap(), Provider::Bedrock);
        assert_eq!(parse_provider("Anthropic").unwrap(), Provider::Anthropic);
    }

    #[test]
    fn parse_provider_rejects_unknown() {
        let err = parse_provider("grok").unwrap_err();
        assert!(matches!(err, LlmInitError::UnknownProvider(_)));
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
    fn usage_tracker_take_on_empty_returns_default() {
        let t = LlmUsageTracker::default();
        let u = t.take("user-1");
        assert_eq!(u.calls, 0);
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cached_input_tokens, 0);
    }

    #[test]
    fn usage_tracker_record_creates_entry() {
        let t = LlmUsageTracker::default();
        t.record("user-1", 100, 20, 5);
        let u = t.take("user-1");
        assert_eq!(u.calls, 1);
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.cached_input_tokens, 5);
    }

    #[test]
    fn usage_tracker_accumulates_across_calls() {
        let t = LlmUsageTracker::default();
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
        let t = LlmUsageTracker::default();
        t.record("user-1", 100, 20, 0);
        let _ = t.take("user-1");
        let u2 = t.take("user-1");
        assert_eq!(u2.calls, 0);
        assert_eq!(u2.input_tokens, 0);
    }

    #[test]
    fn usage_tracker_isolates_users() {
        let t = LlmUsageTracker::default();
        t.record("user-1", 100, 20, 0);
        t.record("user-2", 50, 10, 0);
        let u1 = t.take("user-1");
        assert_eq!(u1.calls, 1);
        assert_eq!(u1.input_tokens, 100);
        let u2 = t.take("user-2");
        assert_eq!(u2.calls, 1);
        assert_eq!(u2.input_tokens, 50);
    }
}
