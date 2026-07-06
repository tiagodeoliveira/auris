//! LLM provider abstraction (rig-core wrapper) with per-pool
//! dispatch (chat vs background) and per-provider backends
//! (Bedrock / Anthropic / OpenAI / Gemini / xAI).

pub mod backend;
pub mod client;
pub mod errors;
pub mod extractor;
pub mod provider;
pub mod usage;

#[allow(unused_imports)]
pub(crate) use backend::LlmBackend;
pub use client::{BreakerGuard, LlmClient};
pub use errors::{CircuitOpenError, ExtractionError, LlmInitError};
#[allow(unused_imports)]
pub(crate) use extractor::LlmExtractor;
pub use provider::{LlmPool, Provider};
pub use usage::{ExtractedMetadata, LlmUsage, LlmUsageTracker};
