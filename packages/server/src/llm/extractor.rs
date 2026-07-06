//! Typed rig `Extractor` enum (`LlmExtractor`).

use std::sync::Arc;

use rig_core::extractor::Extractor;

use super::usage::ExtractedMetadata;

// ─── Type aliases for model types ────────────────────────────────────────────
type BedrockModel = rig_bedrock::completion::CompletionModel;
type OpenAIModel = rig_core::providers::openai::responses_api::ResponsesCompletionModel;
type AnthropicModel = rig_core::providers::anthropic::completion::CompletionModel;
type GeminiModel = rig_core::providers::gemini::completion::CompletionModel;
type XaiModel = rig_core::providers::xai::completion::CompletionModel;

/// Enum wrapping a typed rig `Extractor` for each supported provider.
///
/// `pub(crate)` — callers only interact with `LlmClient`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LlmExtractor {
    Bedrock(Arc<Extractor<BedrockModel, ExtractedMetadata>>),
    OpenAI(Arc<Extractor<OpenAIModel, ExtractedMetadata>>),
    Anthropic(Arc<Extractor<AnthropicModel, ExtractedMetadata>>),
    Gemini(Arc<Extractor<GeminiModel, ExtractedMetadata>>),
    Xai(Arc<Extractor<XaiModel, ExtractedMetadata>>),
}

// Manual Clone: Arc makes each variant cheap to clone.
impl Clone for LlmExtractor {
    fn clone(&self) -> Self {
        match self {
            Self::Bedrock(e) => Self::Bedrock(Arc::clone(e)),
            Self::OpenAI(e) => Self::OpenAI(Arc::clone(e)),
            Self::Anthropic(e) => Self::Anthropic(Arc::clone(e)),
            Self::Gemini(e) => Self::Gemini(Arc::clone(e)),
            Self::Xai(e) => Self::Xai(Arc::clone(e)),
        }
    }
}
