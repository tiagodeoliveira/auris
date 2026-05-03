//! LLM-based metadata extraction via rig + rig-bedrock.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rig::extractor::Extractor;
use rig::prelude::*;
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
pub const DEFAULT_OPENAI_MODEL_ID: &str = "gpt-4.1-mini";
pub const DEFAULT_ANTHROPIC_MODEL_ID: &str = "claude-sonnet-4-5";
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
    backend: LlmBackend,
    provider: Provider,
}

impl LlmClient {
    /// Returns the active provider.
    pub fn provider(&self) -> Provider {
        self.provider
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
    /// prompt and a different `JsonSchema` target type.
    pub async fn extract_with_prompt<T>(
        &self,
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
        let extractor_call = async {
            match &self.backend {
                LlmBackend::Bedrock { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::OpenAI { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
                LlmBackend::Anthropic { client, model_id } => client
                    .extractor::<T>(model_id.as_str())
                    .preamble(system_prompt)
                    .build()
                    .extract(user_input)
                    .await
                    .map_err(|e| ExtractionError::Extract(e.to_string())),
            }
        };
        tokio::time::timeout(EXTRACTION_TIMEOUT, extractor_call)
            .await
            .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
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
}
