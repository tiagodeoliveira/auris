//! Provider discriminant and pool selection.

use super::errors::LlmInitError;

/// Which LLM backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Bedrock,
    OpenAI,
    Anthropic,
    Gemini,
    Xai,
}

impl Provider {
    /// Stable lower-case identifier for metric labels. Mirrors the tokens
    /// `parse_provider` accepts, so emit and parse are inverses. Distinct
    /// from the `Debug` derive (PascalCase), which logs still use.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
            Provider::OpenAI => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Gemini => "gemini",
            Provider::Xai => "xai",
        }
    }
}

/// Which model pool a `LlmClient` instance serves. Pool selects
/// which `AURIS_LLM_{CHAT,BACKGROUND}_*` env vars `from_env` reads
/// and tags the per-pool usage tracker so meeting-stop drain can
/// attribute cost to the right pool.
///
/// Two pools (not five-per-worker): chat = the stateful agent loop,
/// background = every one-shot structured-output worker (summary,
/// moment, artifact, wrap-up, meeting-start metadata).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmPool {
    Chat,
    Background,
}

impl LlmPool {
    /// Stable lower-case identifier used in env var names, log
    /// fields, and `meeting_llm_usage.pool` rows. Changing these
    /// strings is a breaking change for log consumers and the DB.
    pub fn as_str(self) -> &'static str {
        match self {
            LlmPool::Chat => "chat",
            LlmPool::Background => "background",
        }
    }
}

/// Parse a provider name string (case-insensitive) into a [`Provider`].
pub(crate) fn parse_provider(s: &str) -> Result<Provider, LlmInitError> {
    match s.to_ascii_lowercase().as_str() {
        "bedrock" => Ok(Provider::Bedrock),
        "openai" => Ok(Provider::OpenAI),
        "anthropic" => Ok(Provider::Anthropic),
        "gemini" => Ok(Provider::Gemini),
        "xai" => Ok(Provider::Xai),
        _ => Err(LlmInitError::UnknownProvider(s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_provider("Gemini").unwrap(), Provider::Gemini);
        assert_eq!(parse_provider("XAI").unwrap(), Provider::Xai);
    }

    #[test]
    fn parse_provider_accepts_gemini() {
        assert_eq!(parse_provider("gemini").unwrap(), Provider::Gemini);
    }

    #[test]
    fn parse_provider_accepts_xai() {
        assert_eq!(parse_provider("xai").unwrap(), Provider::Xai);
    }

    #[test]
    fn parse_provider_rejects_unknown() {
        let err = parse_provider("grok").unwrap_err();
        assert!(matches!(err, LlmInitError::UnknownProvider(_)));
    }

    #[test]
    fn provider_as_str_is_lowercase_and_round_trips() {
        // as_str() feeds metric labels; pin the lower-case strings AND
        // the emit→parse inverse so a label-casing regression (the
        // PascalCase-in-Prometheus bug) can't reappear silently.
        for p in [
            Provider::Bedrock,
            Provider::OpenAI,
            Provider::Anthropic,
            Provider::Gemini,
            Provider::Xai,
        ] {
            let s = p.as_str();
            assert_eq!(s, s.to_ascii_lowercase(), "{s} must be lower-case");
            assert_eq!(parse_provider(s).unwrap(), p, "round-trip failed for {s}");
        }
        assert_eq!(Provider::OpenAI.as_str(), "openai");
        assert_eq!(Provider::Xai.as_str(), "xai");
    }

    #[test]
    fn llm_pool_as_str_is_stable() {
        // The Display/as_str output is the source of truth for log
        // fields and DB rows — pin both variants so a rename here
        // doesn't silently change wire-visible strings.
        assert_eq!(LlmPool::Chat.as_str(), "chat");
        assert_eq!(LlmPool::Background.as_str(), "background");
    }
}
