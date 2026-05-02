# Meeting Companion — Phase 2 Step 16: LLM Metadata Extraction (v2 — rig)

> **Status:** Draft, pending review.
> **Last updated:** 2026-05-02.
> **Companion to:**
>
> - [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10 step 16 (Phase 2 build order).
> - [`server.md`](server.md) §8.4 — superseded by this document for the extraction implementation; the wire-contract rules in §4.2 / §4.5 still apply.
>
> **Replaces in code:** the deterministic stub `extract_metadata(description) -> HashMap<String, String>` in `packages/server/src/extraction.rs`.
>
> **Framework choice:** [rig](https://github.com/0xPlaygrounds/rig). 20+ provider support, agent abstractions (multi-turn / tool calling), built-in retries via the underlying provider crates, and a memory companion (`cortex-mem`) we'll wire to mnemo for Phase 2 step 18. Our v1 implementation pins Bedrock + Sonnet 4.7 via `rig-bedrock`, but the abstraction stays at rig's `Extractor` level so swapping providers is a constructor change, not a redesign.

## 1. Purpose & scope

### 1.1 What this component does

- Calls Anthropic Claude Sonnet 4.7 via rig's `Extractor` pattern. Default provider is AWS Bedrock through the `rig-bedrock` companion crate; the abstraction is provider-agnostic so other rig providers (Anthropic-direct, OpenAI, etc.) become a one-file swap.
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
- Local-only LLMs (Ollama, llama.cpp). rig supports these but we don't ship a config path for them in v1.
- Out of scope: Phase 2 steps 15 (audio + STT/summarizer), 17 (dynamic mode catalog), 18 (memory enrichment).

### 1.3 Phases referenced

This is Phase 2 step 16 per [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10. Steps 15, 17, 18 are explicitly out of scope.

## 2. Public interface (Rust)

### 2.1 Function signature change

The existing `packages/server/src/extraction.rs` exports:

```rust
pub fn extract_metadata(description: &str) -> HashMap<String, String>;
```

This becomes async, takes a `&LlmClient`, and returns a `Result`:

```rust
pub async fn extract_metadata(
    client: &LlmClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError>;
```

The `merge_manual_wins(extracted, manual)` helper in the same file is **unchanged**.

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

Simpler than the Bedrock-direct draft because rig's Extractor handles tool-use parsing and SDK plumbing for us — failures collapse into a single `Extract(String)` variant with the rig error formatted as a string. The `Timeout` variant remains separate so the caller can distinguish "service-too-slow" from "service-said-no" if it wants.

### 2.3 Caller integration (ws.rs `spawn_extraction`)

Today's `spawn_extraction` (Phase 0):

```rust
spawn_extraction(handle, description, cancel) {
    sleep(1500ms);
    extracted = extract_metadata(&description);  // pure
    merged = merge_manual_wins(extracted, &manual);
    state.metadata = merged;
    broadcast(metadata_changed { merged });
}
```

Becomes (Phase 2 step 16):

```rust
spawn_extraction(handle, description, cancel) {
    if MEETING_COMPANION_LLM_DISABLED is set: return;

    let extracted = match handle.llm.extract(&description).await {
        Ok(map) => map,
        Err(e) => {
            warn!(error = %e, "metadata extraction failed");
            broadcast(status { error: Some(short_error(&e)) });
            return;
        }
    };
    let merged = merge_manual_wins(extracted, &manual);
    state.metadata = merged;
    broadcast(metadata_changed { merged });
}
```

Differences from the Phase 0 simulated path:

- **No `sleep(1500ms)`** — the simulated 1.5s delay is removed. Real LLM calls have their own latency.
- **Disable flag short-circuit** at the top: `MEETING_COMPANION_LLM_DISABLED=1` skips extraction. Used by the test infra so default `cargo test` runs don't require credentials.
- **Real error handling**: on any `ExtractionError`, log + broadcast a `status` event with a short user-friendly error message. The PWA toast machinery surfaces it. Manual metadata is preserved (the initial `metadata_changed { manual }` from `start_meeting` already fired).
- **The 8s wallclock cap** lives inside `LlmClient::extract`, not here.

### 2.4 Wire contract — unchanged

The PWA-facing wire contract is **identical** to Phase 0:

- `start_meeting { description, metadata }` triggers extraction.
- After `start_meeting` startup completes, the PWA receives in order: `meeting_state_changed { active }`, initial `metadata_changed { manual }`, `mode_changed`.
- Then later (when extraction returns):
  - On success: a second `metadata_changed { merged }` event.
  - On failure: a `status` event with `error: Some("Metadata extraction failed: <short>")`. No second `metadata_changed`.

PWA code requires zero changes for this step.

## 3. LLM client

### 3.1 Module layout

New module: `packages/server/src/llm.rs`. Wraps a rig `Extractor<ExtractedMetadata>` and the provider client that produced it. Exports `LlmClient`, `ExtractedMetadata`, `LlmInitError`, and the public `extract` method.

### 3.2 `ExtractedMetadata` struct (the typed extraction target)

```rust
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExtractedMetadata {
    /// Concise meeting title in 8 words or fewer. Empty string if not extractable.
    pub title: String,

    /// Project name if mentioned. Empty string if not extractable.
    pub project: String,
}
```

The `JsonSchema` derive generates the tool-input JSON Schema rig sends to the model. The doc comments become the field descriptions in the generated schema (rig + schemars surface these to the model). The `Deserialize` impl is what rig's Extractor uses to parse the model's tool_use response back into a typed Rust value.

Initial schema is intentionally minimal: `title` + `project` only (matches the Phase 0 stub's output and the architecture's "relevant keys" in §3 server responsibilities). Adding fields means adding fields to the struct — one-line change, no separate schema document to maintain.

### 3.3 `LlmClient` struct

```rust
use rig::providers::bedrock;
use rig::extractor::Extractor;
use std::sync::Arc;

#[derive(Clone)]
pub struct LlmClient {
    extractor: Arc<Extractor<bedrock::completion::CompletionModel, ExtractedMetadata>>,
}

impl LlmClient {
    pub async fn from_env() -> Result<Self, LlmInitError>;
    pub async fn extract(&self, description: &str) -> Result<HashMap<String, String>, ExtractionError>;
}
```

**Note on the type parameter**: rig's `Extractor` is generic over the completion model and the target type. We pin the model to `bedrock::completion::CompletionModel` for v1. To swap providers later (e.g. to `anthropic::completion::CompletionModel`), the type parameter changes — that's a single-line edit + a Cargo feature swap, no API surface change.

If we want runtime-configurable provider selection (env var picks bedrock vs. anthropic-direct), that requires either a trait object (`Arc<dyn ExtractorTrait>`) or a non-generic enum wrapper around all the provider Extractors we support. **Deferred** — v1 ships Bedrock-pinned; runtime provider selection lands in a Phase 2 step 16 v2 if and when the second provider is needed.

### 3.4 `from_env`

```rust
const DEFAULT_REGION: &str = "us-west-2";
const DEFAULT_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";

const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. If a field cannot be confidently extracted from the description, \
return an empty string for that field — do not guess.";

impl LlmClient {
    pub async fn from_env() -> Result<Self, LlmInitError> {
        let region = std::env::var("MEETING_COMPANION_LLM_REGION")
            .unwrap_or_else(|_| DEFAULT_REGION.to_string());
        let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
            .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string());

        let bedrock_client = bedrock::Client::new(&region).await
            .map_err(|e| LlmInitError::Provider(format!("bedrock client init: {e}")))?;

        let extractor = bedrock_client
            .extractor::<ExtractedMetadata>(&model_id)
            .preamble(SYSTEM_PROMPT)
            .build();

        info!(%region, %model_id, "LLM client initialized (rig + bedrock)");

        Ok(Self {
            extractor: Arc::new(extractor),
        })
    }
}
```

`from_env` does NOT make a model call. It resolves region + model id, constructs the rig+bedrock provider client (which itself uses the AWS SDK credential chain — env vars / `~/.aws/credentials` / IMDS / etc.), and builds the Extractor. AWS credential validation happens at first use.

### 3.5 `extract`

```rust
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

impl LlmClient {
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        let prompt = format!("Meeting description:\n{description}");

        let typed: ExtractedMetadata = tokio::time::timeout(
            EXTRACTION_TIMEOUT,
            self.extractor.extract(&prompt),
        )
        .await
        .map_err(|_| ExtractionError::Timeout(EXTRACTION_TIMEOUT))?
        .map_err(|e| ExtractionError::Extract(e.to_string()))?;

        Ok(into_map(typed))
    }
}

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
```

Empty-string fields are dropped (per §1.1 — empty means "the model couldn't extract this" and shouldn't pollute manual metadata via the merge). `into_map` is a pure function, separately unit-testable.

The `Timeout` wraps the entire `extractor.extract` call, including any internal retries rig performs.

### 3.6 Retries

rig's provider crates handle retry/backoff internally for transient errors (rate limits, 5xx, transient network). We do not write our own retry loop. The 8s `tokio::time::timeout` is the wallclock budget — if rig's retry attempts collectively exceed 8s, we abort with `ExtractionError::Timeout`.

## 4. Configuration

### 4.1 Environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| AWS credentials (any standard chain) | yes (in production) | — | Required by `rig-bedrock` |
| `MEETING_COMPANION_LLM_REGION` | no | `us-west-2` | AWS region for Bedrock |
| `MEETING_COMPANION_LLM_MODEL_ID` | no | `us.anthropic.claude-sonnet-4-7-20251015-v1:0` | Bedrock model id (cross-region inference profile) |
| `MEETING_COMPANION_LLM_DISABLED` | no | unset | Dev escape hatch — skip extraction entirely (test infra sets this; users can set it for offline dev) |

The cross-region inference profile (model id starting with `us.`) must be enabled in your AWS Bedrock console — one-time setup per account.

When swapping providers in a future version, the env var prefix `MEETING_COMPANION_LLM_*` stays stable; only the values + provider-specific creds change.

### 4.2 Server boot sequence

Phase 0 boot (per `server.md` §6.5) gains one step:

1. Parse CLI args + env (existing).
2. Validate `MEETING_COMPANION_TOKEN` (existing).
3. **NEW: Initialize `LlmClient::from_env().await`. On failure → exit code 3.**
4. Construct `ServerHandle` carrying `Arc<LlmClient>` (existing handle gains one more field).
5. Spawn heartbeat task (existing).
6. Run accept loop (existing).

### 4.3 Local development without AWS

Set `MEETING_COMPANION_LLM_DISABLED=1` to skip extraction entirely. The PWA still works (manual metadata, mock items via the server, glasses display, etc.) but no LLM-derived metadata is produced. This is the default in the server's test suite (`tests/common/mod.rs`).

## 5. Test strategy

### 5.1 Unit tests in `llm.rs`

- `into_map` filters empty-string fields:
  - `into_map(ExtractedMetadata { title: "Q1", project: "" })` returns `{"title": "Q1"}`.
  - `into_map(ExtractedMetadata { title: "", project: "" })` returns `{}` (empty map).
  - `into_map(ExtractedMetadata { title: "T", project: "P" })` returns both keys.
- `system_prompt_mentions_extraction` (sanity check that the constant is correct).
- `default_model_id_is_cross_region_profile` — starts with `us.`, contains `claude`.

3 unit tests total, no SDK mocking required because the surface is so much thinner than the Bedrock-direct draft.

The Bedrock-direct draft's parser tests (`parse_response_*`) **don't exist** in this version — rig handles tool-use parsing internally. We trust rig the way we trust serde.

### 5.2 Unit tests in `extraction.rs`

The existing `merge_manual_wins` test stays (the conflict-merging rule is the same). Two new tests covering empty extracted / empty manual edge cases. The Phase 0 `extract_takes_first_8_words` test is **deleted** — the function it tested is gone.

### 5.3 Integration test (env-gated)

`packages/server/tests/llm_integration.rs`:

```rust
#[tokio::test]
async fn extracts_title_from_real_description() {
    if std::env::var("RUN_LLM_INTEGRATION").is_err() {
        return; // skip — same pattern as PWA simulator integration tests
    }
    std::env::remove_var("MEETING_COMPANION_LLM_DISABLED");

    let client = LlmClient::from_env().await.expect("client");
    let result = client.extract("Q1 budget review for the helix product launch")
        .await
        .expect("extraction");

    let title = result.get("title").expect("title key");
    assert!(!title.is_empty());
    assert!(title.split_whitespace().count() <= 8);
    // project: best-effort; either present or filtered out (empty)
}
```

### 5.4 Manual smoke

`Justfile` recipe:

```just
llm-smoke description="Q1 budget review for helix":
    cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

llm-integration:
    RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration -- --nocapture
```

`packages/server/examples/llm_smoke.rs` — boots LlmClient + calls extract + prints result + latency.

## 6. Errors & failure modes

| Failure | rig behavior | Our handling |
|---|---|---|
| Throttling / 5xx / transient network | rig retries internally with backoff | Eventually surfaces as `ExtractionError::Timeout` if all retries collectively exceed 8s |
| `AccessDeniedException` (bad IAM) | not retried | `ExtractionError::Extract("...AccessDenied...")` → status toast |
| `ResourceNotFoundException` (bad model id) | not retried | Same |
| Tool-use response fails Deserialize | rig either retries the model or surfaces a deserialize error | `ExtractionError::Extract(...)` → status toast |
| Tokio timeout (8s exceeded) | rig in-flight call dropped (cooperative cancellation) | `ExtractionError::Timeout` → status toast |

All failures land as `status { error: Some("Metadata extraction failed: <short>") }` events on the WS. The server log carries the detailed cause for the developer. The PWA's existing toast machinery surfaces them. Manual metadata is preserved.

`short_error` mapping:

```rust
fn short_error(e: &ExtractionError) -> String {
    use ExtractionError::*;
    match e {
        Timeout(_) => "Metadata extraction timed out".into(),
        Extract(_) => "Metadata extraction failed".into(),
    }
}
```

We deliberately do NOT include the underlying rig error string in the user-visible toast — it's noisy (stack traces, AWS-specific terminology) and not actionable for the user. Server logs carry the detail.

## 7. Cargo dependencies

Adds to `packages/server/Cargo.toml`:

```toml
[dependencies]
rig-core = { version = "0.36", features = ["derive"] }
rig-bedrock = "0.4"            # version pinned to current at implementation time
schemars = "0.8"
thiserror = "2"
```

`rig-core` `derive` feature enables the `JsonSchema` derive integration. `rig-bedrock` brings in the AWS SDK transitively, so we don't need direct `aws-config` / `aws-sdk-bedrockruntime` deps.

## 8. Out of scope

- Phase 2 steps 15 (audio + STT/summarizer), 17 (dynamic mode catalog), 18 (memory enrichment).
- Local-only LLMs (Ollama, llama.cpp).
- Streaming responses, multi-turn conversations.
- Custom inference parameters (temperature, top-k, top-p) — defaults are fine for extraction.
- Cost monitoring beyond AWS billing alarms.
- Runtime-configurable provider selection (Bedrock pinned in v1; Anthropic-direct or OpenAI become a future version's concern).
- Multi-language description support — English only for v1.
- Caching extracted metadata (descriptions are unique per meeting).

## 9. Open questions

None at time of writing. Two implementation-time confirmations:

1. The exact `rig-bedrock` version current as of implementation (the `0.4` here is approximate); verify against crates.io.
2. The exact rig Extractor API as of `rig-core 0.36` — the calls in §3.4 and §3.5 are written against the documented surface but may need minor tweaks for actual constructor names, argument shapes, etc. The implementer subagent reads the current rig docs at start.
