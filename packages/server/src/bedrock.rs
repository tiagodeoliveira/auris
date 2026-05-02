//! AWS Bedrock client for Claude Sonnet 4.7 metadata extraction.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;
use std::sync::Arc;
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

/// Recursively convert a `serde_json::Value` into an `aws_smithy_types::Document`.
/// Used to pass the tool schema to the Bedrock SDK.
fn json_to_document(value: serde_json::Value) -> aws_smithy_types::Document {
    match value {
        serde_json::Value::Null => aws_smithy_types::Document::Null,
        serde_json::Value::Bool(b) => aws_smithy_types::Document::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i < 0 {
                    aws_smithy_types::Document::Number(aws_smithy_types::Number::NegInt(i))
                } else {
                    aws_smithy_types::Document::Number(aws_smithy_types::Number::PosInt(i as u64))
                }
            } else if let Some(f) = n.as_f64() {
                aws_smithy_types::Document::Number(aws_smithy_types::Number::Float(f))
            } else {
                aws_smithy_types::Document::Null
            }
        }
        serde_json::Value::String(s) => aws_smithy_types::Document::String(s),
        serde_json::Value::Array(arr) => {
            aws_smithy_types::Document::Array(arr.into_iter().map(json_to_document).collect())
        }
        serde_json::Value::Object(map) => aws_smithy_types::Document::Object(
            map.into_iter()
                .map(|(k, v)| (k, json_to_document(v)))
                .collect(),
        ),
    }
}

/// Recursively convert an `aws_smithy_types::Document` into a `serde_json::Value`.
/// Used to parse the tool_use response from the Bedrock SDK.
fn document_to_json(doc: aws_smithy_types::Document) -> serde_json::Value {
    match doc {
        aws_smithy_types::Document::Null => serde_json::Value::Null,
        aws_smithy_types::Document::Bool(b) => serde_json::Value::Bool(b),
        aws_smithy_types::Document::Number(n) => match n {
            aws_smithy_types::Number::PosInt(u) => serde_json::json!(u),
            aws_smithy_types::Number::NegInt(i) => serde_json::json!(i),
            aws_smithy_types::Number::Float(f) => serde_json::json!(f),
        },
        aws_smithy_types::Document::String(s) => serde_json::Value::String(s),
        aws_smithy_types::Document::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(document_to_json).collect())
        }
        aws_smithy_types::Document::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, document_to_json(v)))
                .collect(),
        ),
    }
}

/// Internal representation of a content block, decoupled from the SDK
/// types so unit tests can construct synthetic responses without SDK
/// builders.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum MockContentBlock {
    Text(#[allow(dead_code)] String),
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
}

pub(crate) fn parse_response_blocks(
    blocks: &[MockContentBlock],
) -> Result<HashMap<String, String>, ExtractionError> {
    let tool_input = blocks
        .iter()
        .find_map(|block| match block {
            MockContentBlock::ToolUse { name, input } if name == TOOL_NAME => Some(input),
            _ => None,
        })
        .ok_or(ExtractionError::MissingToolUse)?;

    let obj = tool_input
        .as_object()
        .ok_or_else(|| ExtractionError::SchemaValidation("tool input is not an object".into()))?;

    let mut out = HashMap::new();
    for (k, v) in obj {
        let s = v.as_str().ok_or_else(|| {
            ExtractionError::SchemaValidation(format!("field '{}' is not a string", k))
        })?;
        if !s.is_empty() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Ok(out)
}

/// AWS Bedrock client for Claude Sonnet 4.7 metadata extraction.
#[derive(Clone)]
pub struct BedrockClient {
    inner: Arc<aws_sdk_bedrockruntime::Client>,
    model_id: String,
}

impl BedrockClient {
    /// Construct a `BedrockClient` from environment variables and the standard
    /// AWS credential chain. Does NOT make a Bedrock API call (spec §3.5).
    pub async fn from_env() -> Result<Self, BedrockInitError> {
        let region_str = std::env::var("MEETING_COMPANION_BEDROCK_REGION")
            .unwrap_or_else(|_| DEFAULT_REGION.to_string());
        let model_id = std::env::var("MEETING_COMPANION_BEDROCK_MODEL_ID")
            .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string());

        let region = aws_sdk_bedrockruntime::config::Region::new(region_str);
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region)
            .load()
            .await;

        let client = aws_sdk_bedrockruntime::Client::new(&config);

        tracing::info!(
            region = %config.region().map(|r| r.as_ref()).unwrap_or("?"),
            %model_id,
            "Bedrock client initialized"
        );

        Ok(Self {
            inner: Arc::new(client),
            model_id,
        })
    }

    /// Extract structured metadata from a meeting description using Claude Sonnet 4.7
    /// via the Bedrock Converse API with forced tool use.
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        use aws_sdk_bedrockruntime::types as t;

        let user_message = t::Message::builder()
            .role(t::ConversationRole::User)
            .content(t::ContentBlock::Text(format!(
                "Meeting description:\n{}",
                description
            )))
            .build()
            .map_err(|e| ExtractionError::Sdk(format!("build user message: {}", e)))?;

        let tool_spec = t::ToolSpecification::builder()
            .name(TOOL_NAME)
            .description("Extract structured metadata from a meeting description.")
            .input_schema(t::ToolInputSchema::Json(json_to_document(
                extraction_tool_schema(),
            )))
            .build()
            .map_err(|e| ExtractionError::Sdk(format!("build tool spec: {}", e)))?;

        let tool_choice = t::ToolChoice::Tool(
            t::SpecificToolChoice::builder()
                .name(TOOL_NAME)
                .build()
                .map_err(|e| ExtractionError::Sdk(format!("build tool choice: {}", e)))?,
        );

        let tool_config = t::ToolConfiguration::builder()
            .tools(t::Tool::ToolSpec(tool_spec))
            .tool_choice(tool_choice)
            .build()
            .map_err(|e| ExtractionError::Sdk(format!("build tool config: {}", e)))?;

        let system_block = t::SystemContentBlock::Text(SYSTEM_PROMPT.to_string());

        let req = self
            .inner
            .converse()
            .model_id(&self.model_id)
            .messages(user_message)
            .system(system_block)
            .tool_config(tool_config);

        let response = tokio::time::timeout(EXTRACTION_TIMEOUT, req.send())
            .await
            .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
            .map_err(|e| ExtractionError::Sdk(format!("converse: {}", e)))?;

        let output = response
            .output
            .ok_or_else(|| ExtractionError::Sdk("response missing output".into()))?;

        let message = match output {
            t::ConverseOutput::Message(m) => m,
            other => {
                return Err(ExtractionError::Sdk(format!(
                    "unexpected output variant: {:?}",
                    other
                )));
            }
        };

        let blocks: Vec<MockContentBlock> = message
            .content
            .into_iter()
            .filter_map(|block| match block {
                t::ContentBlock::Text(s) => Some(MockContentBlock::Text(s)),
                t::ContentBlock::ToolUse(tu) => Some(MockContentBlock::ToolUse {
                    name: tu.name,
                    input: document_to_json(tu.input),
                }),
                _ => None,
            })
            .collect();

        parse_response_blocks(&blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool_use(input: serde_json::Value) -> Vec<MockContentBlock> {
        vec![MockContentBlock::ToolUse {
            name: TOOL_NAME.to_string(),
            input,
        }]
    }

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

    #[test]
    fn parse_response_happy_path() {
        let blocks = make_tool_use(json!({
            "title": "Q1 budget review",
            "project": "helix"
        }));
        let result = parse_response_blocks(&blocks).unwrap();
        assert_eq!(result.get("title"), Some(&"Q1 budget review".to_string()));
        assert_eq!(result.get("project"), Some(&"helix".to_string()));
    }

    #[test]
    fn parse_response_filters_empty_strings() {
        let blocks = make_tool_use(json!({
            "title": "Q1 review",
            "project": ""
        }));
        let result = parse_response_blocks(&blocks).unwrap();
        assert_eq!(result.get("title"), Some(&"Q1 review".to_string()));
        assert!(!result.contains_key("project"));
    }

    #[test]
    fn parse_response_missing_tool_use() {
        let blocks: Vec<MockContentBlock> = vec![MockContentBlock::Text("just text".to_string())];
        let result = parse_response_blocks(&blocks);
        assert!(matches!(result, Err(ExtractionError::MissingToolUse)));
    }

    #[test]
    fn parse_response_non_string_field() {
        let blocks = make_tool_use(json!({
            "title": "Q1",
            "project": 42
        }));
        let result = parse_response_blocks(&blocks);
        assert!(matches!(result, Err(ExtractionError::SchemaValidation(_))));
    }

    #[test]
    fn parse_response_includes_extra_keys_returned_by_model() {
        let blocks = make_tool_use(json!({
            "title": "Q1",
            "project": "helix",
            "client": "bonus"
        }));
        let result = parse_response_blocks(&blocks).unwrap();
        assert_eq!(result.get("client"), Some(&"bonus".to_string()));
    }
}
