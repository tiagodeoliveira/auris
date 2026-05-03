//! LLM-based metadata extraction via rig + rig-bedrock.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rig::extractor::Extractor;
use rig::prelude::*;
use rig_bedrock::client::Client as BedrockClient;
use rig_bedrock::completion::CompletionModel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

pub const DEFAULT_REGION: &str = "us-west-2";
pub const DEFAULT_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
pub const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. If a field cannot be confidently extracted from the description, \
return an empty string for that field — do not guess.";
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

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
}

/// Errors that can occur during metadata extraction.
#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("LLM call exceeded timeout of {0:?}")]
    Timeout(Duration),

    #[error("Extraction failed: {0}")]
    Extract(String),
}

/// Thin wrapper around a rig `Extractor` backed by AWS Bedrock.
///
/// `Extractor<M, T>` does not implement `Clone`, so the inner extractor is
/// wrapped in an `Arc` to allow cheap cloning of `LlmClient` across tasks.
#[derive(Clone)]
pub struct LlmClient {
    extractor: Arc<Extractor<CompletionModel, ExtractedMetadata>>,
}

impl LlmClient {
    /// Construct a `LlmClient` from environment variables.
    ///
    /// Reads:
    /// - `MEETING_COMPANION_LLM_REGION` (default: `us-west-2`)
    /// - `MEETING_COMPANION_LLM_MODEL_ID` (default: cross-region Sonnet 4.7 profile)
    ///
    /// Does **not** make an API call. AWS credential validation happens at first
    /// `extract` invocation.
    pub async fn from_env() -> Result<Self, LlmInitError> {
        let region = std::env::var("MEETING_COMPANION_LLM_REGION")
            .unwrap_or_else(|_| DEFAULT_REGION.to_string());
        let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
            .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string());

        // ClientBuilder::default() + .region() + .build().await — async, infallible.
        let bedrock_client: BedrockClient = rig_bedrock::client::ClientBuilder::default()
            .region(&region)
            .build()
            .await;

        let extractor = bedrock_client
            .extractor::<ExtractedMetadata>(&model_id)
            .preamble(SYSTEM_PROMPT)
            .build();

        info!(%region, %model_id, "LLM client initialised (rig + bedrock)");

        Ok(Self {
            extractor: Arc::new(extractor),
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

        let typed =
            tokio::time::timeout(EXTRACTION_TIMEOUT, self.extractor.extract(prompt.as_str()))
                .await
                .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                .map_err(|e| ExtractionError::Extract(e.to_string()))?;

        Ok(into_map(typed))
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
    fn default_model_id_is_cross_region_profile() {
        assert!(DEFAULT_MODEL_ID.starts_with("us."));
        assert!(DEFAULT_MODEL_ID.contains("claude"));
    }
}
