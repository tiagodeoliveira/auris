//! AWS Bedrock client for Claude Sonnet 4.7 metadata extraction.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::time::Duration;

use thiserror::Error;

pub const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. Use the extract_metadata tool to return your answer. If a field \
cannot be confidently extracted from the description, return an empty string \
for that field — do not guess.";

pub const DEFAULT_REGION: &str = "us-west-2";
pub const DEFAULT_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
pub const TOOL_NAME: &str = "extract_metadata";
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Error)]
pub enum BedrockInitError {
    #[error("invalid region: {0}")]
    InvalidRegion(String),
    #[error("AWS SDK init failed: {0}")]
    Sdk(String),
}

#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("Bedrock call exceeded timeout of {0:?}")]
    Timeout(Duration),
    #[error("Bedrock returned no tool_use response (got text or no content)")]
    MissingToolUse,
    #[error("Tool input failed schema validation: {0}")]
    SchemaValidation(String),
    #[error("Bedrock SDK error: {0}")]
    Sdk(String),
}

pub fn extraction_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "description": "Concise meeting title in 8 words or fewer. Empty string if not extractable from the description."
            },
            "project": {
                "type": "string",
                "description": "Project name if mentioned in the description. Empty string if not extractable."
            }
        },
        "required": ["title", "project"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_tool_schema_is_valid() {
        let schema = extraction_tool_schema();
        let obj = schema.as_object().unwrap();
        assert_eq!(obj["type"], "object");

        let props = obj["properties"].as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(props.contains_key("project"));

        let required = obj["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "title"));
        assert!(required.iter().any(|v| v == "project"));
    }

    #[test]
    fn system_prompt_mentions_tool_name() {
        assert!(SYSTEM_PROMPT.contains(TOOL_NAME));
    }

    #[test]
    fn default_model_id_is_cross_region_profile() {
        assert!(DEFAULT_MODEL_ID.starts_with("us."));
        assert!(DEFAULT_MODEL_ID.contains("claude"));
    }
}
