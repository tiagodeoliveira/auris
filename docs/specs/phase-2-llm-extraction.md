# Meeting Companion — Phase 2 Step 16: LLM Metadata Extraction (v3 — multi-provider)

> **Status:** v3 active (multi-provider). v1 (Bedrock-direct) reverted; v2 (rig + Bedrock-pinned) shipped first; v3 adds OpenAI as a second provider with runtime selection via env var.
> **Last updated:** 2026-05-02.
> **Companion to:**
>
> - [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10 step 16 (Phase 2 build order).
> - [`server.md`](server.md) §8.4 — superseded by this document for the extraction implementation; the wire-contract rules in §4.2 / §4.5 still apply.
>
> **Replaces in code:** the deterministic stub `extract_metadata(description) -> HashMap<String, String>` in `packages/server/src/extraction.rs`.
>
> **Framework choice:** [rig](https://github.com/0xPlaygrounds/rig). 20+ provider support, agent abstractions (multi-turn / tool calling), built-in retries via the underlying provider crates, and a memory companion (`cortex-mem`) we'll wire to mnemo for Phase 2 step 18. v3 ships **Bedrock + OpenAI**; runtime selection via `MEETING_COMPANION_LLM_PROVIDER`. Adding more rig-supported providers (Anthropic-direct, Gemini, etc.) is a one-arm-on-the-enum change.

## 1. Purpose & scope

### 1.1 What this component does

- Calls a configurable LLM via rig's `Extractor` pattern. Provider chosen at boot via `MEETING_COMPANION_LLM_PROVIDER` env var. v3 supports `bedrock` (default — Anthropic Claude Sonnet 4.7 via `rig-bedrock`) and `openai` (gpt-4.1-mini by default via `rig-core`'s OpenAI provider).
- Uses rig's structured-extraction layer (Extractor), which generates a tool-use JSON schema from a Rust struct's `JsonSchema` derive and parses the model's response back into the struct. We never hand-roll tool schemas, hand-parse content blocks, or hand-code retries — rig's mature transport layer handles that.
- Merges the extracted struct with manual metadata (manual wins on conflict — unchanged from Phase 0 [`server.md`](server.md) §4.2). The merge happens in `ws.rs::spawn_extraction`, which then broadcasts the follow-up `metadata_changed` event.
- Surfaces failures via the existing `status { error }` event on the WebSocket so the PWA can toast them.

### 1.2 What this component does NOT do

- Real STT for the description — the PWA already does Soniox STT in the listening flow ([`pwa.md`](pwa.md) §5.3); this consumes its output.
- Real summarization of the meeting itself — Phase 2 step 15.
- Caching of LLM responses — descriptions are unique per meeting; caching is unnecessary cost and complexity.
- Multi-turn extraction or chain-of-thought — one synchronous call, one response.
- Streaming responses — extraction is short; we await the full response.
- Multi-agent orchestration with `cortex-mem` memory — that's Phase 2 step 18, when we wire mnemo as the memory backend for richer mode-as-agent behavior.
- Local-only LLMs (Ollama, llama.cpp). rig supports these but we don't ship a config path for them in v3.
- Provider-side comparison or A/B routing — one provider per server boot. To compare providers, restart the server with a different `MEETING_COMPANION_LLM_PROVIDER`.
- Out of scope: Phase 2 steps 15 (audio + STT/summarizer), 17 (dynamic mode catalog), 18 (memory enrichment).

### 1.3 Phases referenced

This is Phase 2 step 16 per [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10. Steps 15, 17, 18 are explicitly out of scope.

## 2. Public interface (Rust)

### 2.1 Function signature

`packages/server/src/extraction.rs` exports:

```rust
pub async fn extract_metadata(
    client: &LlmClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError>;
```

Unchanged from v2. The caller passes the same `&LlmClient` regardless of which provider is in play.

The `merge_manual_wins(extracted, manual)` helper is unchanged from Phase 0.

### 2.2 Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("LLM call exceeded timeout of {0:?}")]
    Timeout(Duration),

    #[error("Extraction failed: {0}")]
    Extract(String),
}
```

Unchanged from v2. The `Extract` variant carries whichever provider's error was returned (rig normalizes them as strings).

A new `LlmInitError::MissingProviderCredentials(String)` variant covers the boot-time case where the user picks `openai` but `OPENAI_API_KEY` is unset (see §6).

### 2.3 Caller integration (`ws.rs::spawn_extraction`)

Unchanged from v2. The caller:

1. Checks `MEETING_COMPANION_LLM_DISABLED` and short-circuits if set.
2. Calls `handle.llm.extract(&description)` with cancellation via `tokio::select!`.
3. On `ExtractionError`: log + broadcast `status { error: Some(...) }` and abandon.
4. On success: merge with manual metadata and broadcast `metadata_changed`.

The fact that `LlmClient` now wraps a multi-variant enum is invisible to the caller.

### 2.4 Wire contract — unchanged

The PWA-facing wire contract is identical across v1, v2, and v3. PWA code requires zero changes for this step.

## 3. LLM client

### 3.1 Module layout

`packages/server/src/llm.rs` exports:

- `LlmClient` — public wrapper holding the dispatch enum.
- `LlmExtractor` — `pub(crate)` enum with one variant per supported provider, each carrying a typed `rig::extractor::Extractor<ProviderModel, ExtractedMetadata>`.
- `Provider` — public enum reflecting the user's selection (`Bedrock`, `OpenAI`). Used by `from_env` parsing + diagnostics.
- `ExtractedMetadata` — the typed extraction target (unchanged from v2).
- `LlmInitError`, `ExtractionError` — error types.

The dispatch decision lives entirely in `from_env` and the `extract` method's match arm. The rest of the module is provider-agnostic.

### 3.2 `ExtractedMetadata` struct

Unchanged from v2:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExtractedMetadata {
    /// Concise meeting title in 8 words or fewer. Empty string if not extractable.
    pub title: String,

    /// Project name if mentioned. Empty string if not extractable.
    pub project: String,
}
```

(The `Serialize` derive is required by `rig-core 0.36`'s Extractor, per the v2 implementation report.)

### 3.3 `LlmClient` and `LlmExtractor` (enum dispatch)

```rust
use std::sync::Arc;
use rig::extractor::Extractor;
use rig_bedrock::completion::CompletionModel as BedrockModel;
use rig::providers::openai::completion::CompletionModel as OpenAIModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Bedrock,
    OpenAI,
}

pub(crate) enum LlmExtractor {
    Bedrock(Arc<Extractor<BedrockModel, ExtractedMetadata>>),
    OpenAI(Arc<Extractor<OpenAIModel, ExtractedMetadata>>),
}

#[derive(Clone)]
pub struct LlmClient {
    extractor: LlmExtractor,
    provider: Provider,  // for logging / diagnostics
}

impl LlmClient {
    pub async fn from_env() -> Result<Self, LlmInitError>;
    pub async fn extract(&self, description: &str) -> Result<HashMap<String, String>, ExtractionError>;
    pub fn provider(&self) -> Provider { self.provider }
}
```

The `Arc` wrapping inside each enum variant lets `LlmClient` be `Clone` cheaply. `LlmExtractor` itself isn't `Clone`-derived because rig's `Extractor` isn't `Clone` (per the v2 implementation finding); the `Arc` provides clone-ability per variant.

Adding a new provider is exactly:

1. Add a variant to `LlmExtractor`.
2. Add a variant to `Provider`.
3. Add a parser arm in `parse_provider`.
4. Add a constructor arm in `from_env`.
5. Add a match arm in `extract`.

~10 lines of Rust per provider. The `ExtractedMetadata`, prompt, schema, and caller code are all provider-agnostic.

### 3.4 `from_env`

```rust
const DEFAULT_BEDROCK_REGION: &str = "us-west-2";
const DEFAULT_BEDROCK_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
const DEFAULT_OPENAI_MODEL_ID: &str = "gpt-4.1-mini";

const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. If a field cannot be confidently extracted from the description, \
return an empty string for that field — do not guess.";

fn parse_provider(s: &str) -> Result<Provider, LlmInitError> {
    match s.to_ascii_lowercase().as_str() {
        "bedrock" => Ok(Provider::Bedrock),
        "openai" => Ok(Provider::OpenAI),
        other => Err(LlmInitError::UnknownProvider(other.to_string())),
    }
}

impl LlmClient {
    pub async fn from_env() -> Result<Self, LlmInitError> {
        let provider_str =
            std::env::var("MEETING_COMPANION_LLM_PROVIDER").unwrap_or_else(|_| "bedrock".to_string());
        let provider = parse_provider(&provider_str)?;

        let extractor = match provider {
            Provider::Bedrock => {
                let region = std::env::var("MEETING_COMPANION_LLM_REGION")
                    .unwrap_or_else(|_| DEFAULT_BEDROCK_REGION.to_string());
                let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
                    .unwrap_or_else(|_| DEFAULT_BEDROCK_MODEL_ID.to_string());

                let client = rig_bedrock::client::ClientBuilder::default()
                    .region(&region)
                    .build()
                    .await
                    .map_err(|e| LlmInitError::Provider(format!("bedrock client init: {e}")))?;
                let inner = client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .build();

                tracing::info!(provider = "bedrock", %region, %model_id, "LLM client initialized");
                LlmExtractor::Bedrock(Arc::new(inner))
            }
            Provider::OpenAI => {
                if std::env::var("OPENAI_API_KEY").is_err() {
                    return Err(LlmInitError::MissingProviderCredentials(
                        "OPENAI_API_KEY env var is required when LLM_PROVIDER=openai".to_string(),
                    ));
                }
                let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
                    .unwrap_or_else(|_| DEFAULT_OPENAI_MODEL_ID.to_string());

                let client = rig::providers::openai::Client::from_env();
                let inner = client
                    .extractor::<ExtractedMetadata>(&model_id)
                    .preamble(SYSTEM_PROMPT)
                    .build();

                tracing::info!(provider = "openai", %model_id, "LLM client initialized");
                LlmExtractor::OpenAI(Arc::new(inner))
            }
        };

        Ok(Self { extractor, provider })
    }
}
```

`from_env` does NOT make a model call. It resolves provider + model id + provider-specific configuration, constructs the rig Extractor, and returns. Credential validation happens at first `extract` call (Bedrock side) or at `Client::from_env()` time (OpenAI's rig client checks `OPENAI_API_KEY` synchronously). The explicit `MissingProviderCredentials` check for OpenAI runs BEFORE `Client::from_env()` so the error message is ours, not rig's.

### 3.5 `extract` (provider-dispatched)

```rust
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

impl LlmClient {
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        let prompt = format!("Meeting description:\n{description}");

        let typed: ExtractedMetadata = match &self.extractor {
            LlmExtractor::Bedrock(e) => {
                tokio::time::timeout(EXTRACTION_TIMEOUT, e.extract(&prompt))
                    .await
                    .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                    .map_err(|err| ExtractionError::Extract(err.to_string()))?
            }
            LlmExtractor::OpenAI(e) => {
                tokio::time::timeout(EXTRACTION_TIMEOUT, e.extract(&prompt))
                    .await
                    .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
                    .map_err(|err| ExtractionError::Extract(err.to_string()))?
            }
        };

        Ok(into_map(typed))
    }
}
```

The duplication across arms is intentional — extracting the timeout+map pattern into a helper would require generalizing over rig's `Extractor<M, T>` type, which fights the type system. Two near-identical arms is the cheapest correct shape. Each new provider adds one more arm.

`into_map` is unchanged from v2.

### 3.6 Retries

rig's provider crates handle retry/backoff internally. We don't write a retry loop. The 8s `tokio::time::timeout` is the wallclock budget for the entire `extract` call, including retries.

## 4. Configuration

### 4.1 Environment variables

| Variable                              | Required when                          | Default                                          | Purpose                                                    |
| ------------------------------------- | -------------------------------------- | ------------------------------------------------ | ---------------------------------------------------------- |
| `MEETING_COMPANION_LLM_PROVIDER`      | no                                     | `bedrock`                                        | One of `bedrock`, `openai`. Selects which provider to use. |
| `MEETING_COMPANION_LLM_DISABLED`      | no                                     | unset                                            | Dev escape hatch — skip extraction entirely.                |
| `MEETING_COMPANION_LLM_MODEL_ID`      | no                                     | provider-specific (see below)                    | Model id (provider-specific format).                       |
| **Bedrock-only**                      |                                        |                                                  |                                                            |
| AWS credentials (any standard chain)  | when `LLM_PROVIDER=bedrock`            | —                                                | Required by `rig-bedrock`                                  |
| `MEETING_COMPANION_LLM_REGION`        | no                                     | `us-west-2`                                      | AWS region for Bedrock                                     |
| (Bedrock model id default)            | —                                      | `us.anthropic.claude-sonnet-4-7-20251015-v1:0`   | Cross-region inference profile                             |
| **OpenAI-only**                       |                                        |                                                  |                                                            |
| `OPENAI_API_KEY`                      | when `LLM_PROVIDER=openai`             | —                                                | OpenAI API key                                             |
| (OpenAI model id default)             | —                                      | `gpt-4.1-mini`                                   |                                                            |

`MEETING_COMPANION_LLM_DISABLED=1` skips extraction regardless of provider. Test infra sets it; useful for offline dev.

### 4.2 Server boot sequence

Phase 0 boot (per `server.md` §6.5) gains one step (unchanged from v2 in shape; only the underlying construction differs):

1. Parse CLI args + env (existing).
2. Validate `MEETING_COMPANION_TOKEN` (existing).
3. **Initialize `LlmClient::from_env().await`. On failure → exit code 3.**
4. Construct `ServerHandle` carrying `Arc<LlmClient>`.
5. Spawn heartbeat task (existing).
6. Run accept loop (existing).

`LlmClient::from_env` may fail with:

- `LlmInitError::UnknownProvider(s)` — `MEETING_COMPANION_LLM_PROVIDER` was set to something other than `bedrock` or `openai`.
- `LlmInitError::MissingProviderCredentials(reason)` — provider chosen but its credentials env var isn't set (e.g. `LLM_PROVIDER=openai` without `OPENAI_API_KEY`).
- `LlmInitError::Provider(reason)` — the underlying provider client constructor returned an error (rare; usually a config/region issue).

All three exit with code 3 and a clear log message.

### 4.3 Local development without LLM credentials

Set `MEETING_COMPANION_LLM_DISABLED=1` to skip extraction entirely. The PWA still works (manual metadata, mock items, glasses display) but no LLM-derived metadata is produced. Default in the test suite (`tests/common/mod.rs`).

For comparing providers, set `MEETING_COMPANION_LLM_PROVIDER=openai`, ensure `OPENAI_API_KEY` is in your environment, restart the server, and rerun the same workflow. The PWA-side experience is identical; only the extracted metadata content differs.

## 5. Test strategy

### 5.1 Unit tests in `llm.rs`

Existing tests (v2):

- `into_map_drops_empty_title_only`
- `into_map_drops_both_when_empty`
- `into_map_keeps_both_when_present`
- `system_prompt_mentions_extraction`
- `default_model_id_is_cross_region_profile` — renamed to `default_bedrock_model_id_is_cross_region_profile` (still exists; just relabeled).

New v3 tests:

- `parse_provider_accepts_bedrock` — `parse_provider("bedrock") == Ok(Provider::Bedrock)`.
- `parse_provider_accepts_openai` — `parse_provider("openai") == Ok(Provider::OpenAI)`.
- `parse_provider_is_case_insensitive` — `parse_provider("OpenAI") == Ok(Provider::OpenAI)`.
- `parse_provider_rejects_unknown` — `parse_provider("vertex") == Err(LlmInitError::UnknownProvider(_))`.
- `default_openai_model_id_is_set` — `DEFAULT_OPENAI_MODEL_ID` is non-empty and starts with `gpt-`.

Total v3 unit tests in `llm.rs`: 5 (v2) + 5 (v3) = 10.

### 5.2 Unit tests in `extraction.rs`

Unchanged from v2. The `merge_manual_wins` tests cover the merge; nothing else lives there.

### 5.3 Integration test (env-gated)

`packages/server/tests/llm_integration.rs`:

```rust
#[tokio::test]
async fn extracts_title_from_real_description() {
    if std::env::var("RUN_LLM_INTEGRATION").is_err() {
        return;
    }
    std::env::remove_var("MEETING_COMPANION_LLM_DISABLED");
    // RUN_LLM_INTEGRATION runs against whatever provider is configured.
    // The user controls provider via MEETING_COMPANION_LLM_PROVIDER + the
    // appropriate creds.

    let client = LlmClient::from_env().await.expect("client");

    tracing::info!(provider = ?client.provider(), "running integration test");

    let result = client
        .extract("Q1 budget review for the helix product launch and rollout plan")
        .await
        .expect("extraction succeeded");

    let title = result.get("title").expect("title key present");
    assert!(!title.is_empty());
    assert!(title.split_whitespace().count() <= 8);
}
```

The same test exercises whichever provider `MEETING_COMPANION_LLM_PROVIDER` selects. To run against both providers in one CI invocation, run the test twice with the env var set differently:

```bash
RUN_LLM_INTEGRATION=1 MEETING_COMPANION_LLM_PROVIDER=bedrock cargo test -p meeting-companion-server --test llm_integration -- --nocapture
RUN_LLM_INTEGRATION=1 MEETING_COMPANION_LLM_PROVIDER=openai cargo test -p meeting-companion-server --test llm_integration -- --nocapture
```

### 5.4 Manual smoke

Justfile recipes (the `just llm-smoke` recipe stays single-provider-aware — selects via env vars set in shell):

```just
llm-smoke description="Q1 budget review for helix":
    cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

llm-smoke-bedrock description="Q1 budget review for helix":
    MEETING_COMPANION_LLM_PROVIDER=bedrock cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

llm-smoke-openai description="Q1 budget review for helix":
    MEETING_COMPANION_LLM_PROVIDER=openai cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

llm-integration:
    RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration -- --nocapture
```

`packages/server/examples/llm_smoke.rs` prints the resolved `client.provider()` alongside the result so the user knows which provider answered.

## 6. Errors & failure modes

### 6.1 Boot-time errors (`LlmInitError`)

| Variant                            | Cause                                                                                   | User-visible |
| ---------------------------------- | --------------------------------------------------------------------------------------- | ------------ |
| `UnknownProvider(s)`               | `MEETING_COMPANION_LLM_PROVIDER` set to something other than `bedrock` or `openai`.     | exit 3 + log |
| `MissingProviderCredentials(s)`    | Provider selected but its required creds env var is unset (e.g. OpenAI without API key). | exit 3 + log |
| `Provider(s)`                      | Underlying provider constructor returned an error (rare).                                | exit 3 + log |

### 6.2 Extraction-time errors (`ExtractionError`)

Same as v2:

| Failure                              | rig behavior                                | Our handling                                                                          |
| ------------------------------------ | ------------------------------------------- | ------------------------------------------------------------------------------------- |
| Throttling / 5xx / transient network | rig retries internally with backoff          | Eventually surfaces as `ExtractionError::Timeout` if all retries collectively exceed 8s |
| `AccessDenied` / `Unauthorized`      | not retried                                 | `ExtractionError::Extract(...)` → status toast                                         |
| Bad model id                         | not retried                                 | Same                                                                                   |
| Tool-use response fails Deserialize  | rig may retry the model or surface deserialize | `ExtractionError::Extract(...)` → status toast                                         |
| Tokio timeout (8s exceeded)          | rig in-flight call dropped                  | `ExtractionError::Timeout` → status toast                                              |

`short_error` mapping unchanged. Server logs carry the detailed cause; user-visible toast is short and provider-agnostic ("Metadata extraction failed").

## 7. Cargo dependencies

```toml
[dependencies]
rig-bedrock = "0.4"            # current at v3 implementation
rig-core = { version = "0.36", features = ["derive"] }   # also brings in OpenAI provider via openai feature; verify at impl time
schemars = "1"
thiserror = "2"
```

If `rig-core`'s OpenAI provider lives behind a feature flag (e.g. `openai`), enable it. The exact feature name varies across rig versions; the v3 implementer confirms by checking `cargo doc --no-deps -p rig-core --open` or the published features list on crates.io.

If OpenAI provider is shipped in a separate crate (e.g. hypothetical `rig-openai`), add that as a peer to `rig-bedrock` instead. As of `rig-core 0.36` it's bundled — confirm at implementation time.

## 8. Out of scope

- Phase 2 steps 15 (audio + STT/summarizer), 17 (dynamic mode catalog), 18 (memory enrichment).
- Local-only LLMs (Ollama, llama.cpp).
- Streaming responses, multi-turn conversations.
- Custom inference parameters (temperature, top-k, top-p) — defaults are fine for extraction.
- Cost monitoring beyond AWS / OpenAI billing alarms.
- Multi-language description support — English only.
- Caching extracted metadata.
- Side-by-side provider comparison (manual: restart server with different `MEETING_COMPANION_LLM_PROVIDER`).
- Anthropic-direct, Gemini, Cohere, etc. — adding any of these is the same template as the OpenAI work; not committing to specific ones in v3.

## 9. Open questions

None at time of writing. Two implementation-time confirmations:

1. Whether OpenAI lives in `rig-core` as a feature flag or as a separate `rig-openai` crate. The v3 implementer checks crates.io at start.
2. Whether the OpenAI default model id (`gpt-4.1-mini`) is current and available — OpenAI's model lineup churns; the implementer confirms or substitutes (e.g. with `gpt-4o-mini` if `gpt-4.1-mini` isn't released yet on the user's account).
