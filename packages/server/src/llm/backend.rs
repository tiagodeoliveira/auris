//! Raw rig client enum (`LlmBackend`) used to build ad-hoc extractors.

use std::sync::Arc;

// ─── Type aliases for raw client types ───────────────────────────────────────
type OpenAIClientWrapper = rig_core::providers::openai::Client;
type AnthropicClientWrapper = rig_core::providers::anthropic::Client;
type GeminiClientWrapper = rig_core::providers::gemini::Client;
type XaiClientWrapper = rig_core::providers::xai::Client;

/// Stores the raw rig clients and model IDs for building ad-hoc extractors.
///
/// All three client types are `Clone` (they wrap `Arc` internally), so this
/// enum can be cheaply cloned alongside `LlmClient`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LlmBackend {
    Bedrock {
        client: rig_bedrock::client::Client,
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
    Gemini {
        client: Arc<GeminiClientWrapper>,
        model_id: String,
    },
    Xai {
        client: Arc<XaiClientWrapper>,
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
            Self::Gemini { client, model_id } => Self::Gemini {
                client: Arc::clone(client),
                model_id: model_id.clone(),
            },
            Self::Xai { client, model_id } => Self::Xai {
                client: Arc::clone(client),
                model_id: model_id.clone(),
            },
        }
    }
}
