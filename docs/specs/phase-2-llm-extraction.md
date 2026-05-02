# Meeting Companion — Phase 2 Step 16: LLM Metadata Extraction (v1)

> **Status:** Draft, pending review.
> **Last updated:** 2026-05-02.
> **Companion to:**
>
> - [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10 step 16 (Phase 2 build order entry).
> - [`server.md`](server.md) §8.4 (the simulated extraction this replaces) and §0–§7 which stay valid (only internals of `extract_metadata` change).
>
> **Replaces in code:** the deterministic stub `extract_metadata(description) -> HashMap<String, String>` in `packages/server/src/extraction.rs` (currently returns `title: first8words(description)` + `project: "sim-extracted"`).

## 1. Purpose & scope

### 1.1 What this component does

- Calls Anthropic Claude Sonnet 4.7 on AWS Bedrock with the meeting description (the spoken text the PWA captured via Soniox STT, sent over the WS as `start_meeting.description`).
- Uses Bedrock's **tool use** to force the model to return a structured JSON object matching a defined schema. No free-text → regex parsing path.
- Parses the tool_use response into a `HashMap<String, String>` matching the existing `extract_metadata` interface.
- The caller (`spawn_extraction` in `ws.rs`) merges the result with manual metadata (manual wins on conflict — unchanged from Phase 0 [`server.md`](server.md) §8.4) and broadcasts a follow-up `metadata_changed` event.
- Surfaces failures via the existing `status { error }` event on the WebSocket so the PWA can toast them.

### 1.2 What this component does NOT do

- Real STT for the description — the PWA already does Soniox STT in the listening flow ([`pwa.md`](pwa.md) §5.3); this component consumes that text.
- Real summarization of the meeting itself — Phase 2 step 15 (the audio + summarizer pipeline).
- Caching of LLM responses — every meeting gets a fresh extraction. Descriptions are typically unique; caching adds complexity for marginal cost savings.
- Multi-turn extraction or chain-of-thought — one synchronous call, one response.
- Streaming responses — extraction is short; we await the full response.
- Non-Anthropic models — Bedrock has many; we pin Sonnet 4.7.
- Multi-region failover beyond what Bedrock cross-region inference profiles already provide.
- Local-only LLMs (Ollama, llama.cpp, etc.) — would require a different abstraction; not on the roadmap.
- The wider Phase 2 work: steps 15 (audio + summarizer), 17 (dynamic mode catalog), 18 (memory-system enrichment) are out of scope here, each gets its own spec.

### 1.3 Phases referenced

This is Phase 2 step 16 per [`ARCHITECTURE.md`](../ARCHITECTURE.md) §10. Steps 15, 17, 18 are explicitly out of scope.

## 2. Public interface (Rust)

### 2.1 Function signature change

The existing `packages/server/src/extraction.rs` exports:

```rust
pub fn extract_metadata(description: &str) -> HashMap<String, String>;
```

This becomes async, takes a `&BedrockClient`, and returns a `Result`:

```rust
pub async fn extract_metadata(
    client: &BedrockClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError>;
```

The `merge_manual_wins(extracted, manual)` helper in the same file is **unchanged**.

### 2.2 Error type

```rust
#[derive(Debug, thiserror::Error)]
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
```

`Sdk` carries the SDK error as a string (rather than wrapping the typed error) so `extraction.rs` doesn't have to import the entire SDK error tree. The conversion happens at the `bedrock.rs` boundary.

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
    let extracted = match handle.bedrock.extract(description).await {
        Ok(map) => map,
        Err(e) => {
            warn!(error = %e, "metadata extraction failed");
            broadcast(status { error: Some("metadata extraction failed") });
            return;
        }
    };
    let merged = merge_manual_wins(extracted, &manual);
    state.metadata = merged;
    broadcast(metadata_changed { merged });
}
```

Differences:

- **No `sleep(1500ms)`** — the simulated 1.5s delay is removed. Real Bedrock has its own latency (typically 1-3s for short input).
- **Error handling**: on any `ExtractionError`, log + broadcast a `status` event with a short error message. The PWA's toast machinery surfaces it. Manual metadata is unaffected (the initial `metadata_changed { manual }` event from `start_meeting` already fired before extraction started).
- **The 8s timeout** lives inside `bedrock.rs`'s `extract` method, not in `spawn_extraction`'s scope.

### 2.4 Wire contract — unchanged

The PWA-facing wire contract is **identical** to Phase 0:

- `start_meeting { description, metadata }` triggers extraction.
- After `start_meeting` startup completes, the PWA receives in order: `meeting_state_changed { active }`, initial `metadata_changed { manual }`, `mode_changed`. Then later (when extraction returns):
  - On success: a second `metadata_changed { merged }` event.
  - On failure: a `status` event with `error: "metadata extraction failed"`. No second `metadata_changed`.

PWA code requires zero changes for this step.

## 3. Bedrock client

### 3.1 Module layout

New module: `packages/server/src/bedrock.rs`. Wraps the AWS SDK's `bedrockruntime::Client`. Exports `BedrockClient` and `BedrockInitError`.

### 3.2 `BedrockClient` struct

```rust
pub struct BedrockClient {
    inner: aws_sdk_bedrockruntime::Client,
    model_id: String,
}

impl BedrockClient {
    pub async fn from_env() -> Result<Self, BedrockInitError>;
    pub async fn extract(&self, description: &str) -> Result<HashMap<String, String>, ExtractionError>;
}
```

`from_env` resolves credentials and config:

1. Read `MEETING_COMPANION_BEDROCK_REGION` env var (default `us-west-2`).
2. Read `MEETING_COMPANION_BEDROCK_MODEL_ID` env var (default: a constant pointing at the cross-region Sonnet 4.7 inference profile, see §3.3).
3. Build the AWS SDK config via `aws_config::defaults(BehaviorVersion::latest()).region(...).load().await` — this picks up credentials from the standard chain (env vars, `~/.aws/credentials`, IAM role / IMDS, etc.).
4. Construct `aws_sdk_bedrockruntime::Client::new(&config)`.
5. (Optional sanity check, see §3.5.) Return `Ok(BedrockClient { inner, model_id })`.

If any step fails: return `BedrockInitError::*` with a clear message. The server boot sequence fails fast (exit code 3).

### 3.3 Default model id

```rust
const DEFAULT_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
```

(Exact date suffix to be confirmed at implementation time — Bedrock's model id naming is `provider.<name>-<release-date>-v<version>` and the user must enable the cross-region inference profile in the AWS console for their account.)

The `us.` prefix denotes the cross-region inference profile (US region pool). This trades a few ms of routing latency for automatic load balancing and failover across `us-east-1`, `us-east-2`, `us-west-2` capacity.

### 3.4 Retry & timeout

Retries: AWS SDK's `RetryConfig::standard()` is used by default — up to 3 attempts with exponential backoff and jitter. We do **not** write our own retry loop. The SDK retries on:

- `ThrottlingException` (rate limits)
- 5xx server errors
- Connection errors / transient network issues

It does NOT retry on:

- 4xx client errors (bad model id, unauthorized, schema rejection)
- Successful responses with `stop_reason != "end_turn"` or no tool_use (we handle those as application-level errors)

Wallclock timeout: enforced by `tokio::time::timeout(Duration::from_secs(8), ...)` wrapping the `.send()` call. If the entire operation (including SDK internal retries) exceeds 8s, we abort with `ExtractionError::Timeout`.

### 3.5 Boot-time sanity check

`from_env` does NOT make a Bedrock API call to verify the model is reachable (that would slow boot and require Bedrock charges every restart). The first real extraction call surfaces any auth/permission/model-id issues. Server boot only verifies that:

- Credentials resolve (no error from the credential chain).
- Region parses.

If credentials are missing or invalid, the credential chain succeeds (yielding default credentials that Bedrock then rejects on first call), so this check is necessarily superficial — full validation is at first-use. That's acceptable: the failure surfaces as a `status { error }` toast on the first meeting, with the actual SDK error in server logs.

## 4. Prompt design

### 4.1 System prompt

```
You are a meeting metadata extractor. Given a short spoken description of a
meeting (transcribed by an STT system, may contain disfluencies and filler
words), extract concise structured metadata. Use the extract_metadata tool
to return your answer. If a field cannot be confidently extracted from the
description, return an empty string for that field — do not guess.
```

Approximately 70 tokens. Constant, defined as a `const SYSTEM_PROMPT: &str` in `bedrock.rs`.

Bedrock's converse API accepts a `system` parameter — we pass the prompt there (not as a synthetic first user message).

Prompt caching: Bedrock supports prompt caching for large system prompts, but ours is too short to benefit (caching kicks in at ~1024 tokens for most models). Skip for v1.

### 4.2 User message

The user message body is just the description text, wrapped in a minimal scaffold:

```
Meeting description:
{description}
```

No additional context. The description is bounded by the PWA's 25s soft cap on Soniox listening (~150 words).

### 4.3 Tool schema

```rust
fn extraction_tool() -> Tool {
    Tool::ToolSpec(ToolSpecification::builder()
        .name("extract_metadata")
        .description("Extract structured metadata from a meeting description.")
        .input_schema(ToolInputSchema::Json(serde_json::json!({
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
        })))
        .build()
        .expect("valid tool spec"))
}
```

Initial schema is intentionally minimal: `title` + `project` only. This matches the Phase 0 stub's output and the architecture's stated "relevant keys" (§3 server responsibilities). Adding fields is a one-file change.

### 4.4 Tool choice

The `converse` request sets:

```rust
.tool_config(
    ToolConfiguration::builder()
        .tools(extraction_tool())
        .tool_choice(ToolChoice::Tool(SpecificToolChoice::builder()
            .name("extract_metadata")
            .build()?))
        .build()?
)
```

`ToolChoice::Tool(...)` forces the model to call `extract_metadata`. This guarantees the response is a tool_use block, never free text.

## 5. Response parsing

### 5.1 Successful response shape

The Bedrock `converse` response on a tool-forced call returns:

```rust
ConverseOutput::Message(Message {
    role: ConversationRole::Assistant,
    content: vec![
        ContentBlock::ToolUse(ToolUseBlock {
            tool_use_id: "...",      // we don't use this
            name: "extract_metadata",
            input: serde_json::Value::Object({...}),
        }),
        // Possibly leading text content blocks if the model "thinks aloud"
        // before calling the tool — we ignore those.
    ]
})
```

### 5.2 Parsing algorithm

```rust
fn parse_response(message: Message) -> Result<HashMap<String, String>, ExtractionError> {
    // Find the first ToolUse block named "extract_metadata".
    let tool_use = message.content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse(t) if t.name == "extract_metadata" => Some(t),
            _ => None,
        })
        .ok_or(ExtractionError::MissingToolUse)?;

    // Convert input JSON to HashMap<String, String>, dropping empty values.
    let obj = tool_use.input.as_object()
        .ok_or_else(|| ExtractionError::SchemaValidation("tool input is not an object".into()))?;

    let mut out = HashMap::new();
    for (k, v) in obj {
        let s = v.as_str()
            .ok_or_else(|| ExtractionError::SchemaValidation(format!("field '{}' is not a string", k)))?;
        if !s.is_empty() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Ok(out)
}
```

Empty-string values are dropped — they represent "the model couldn't extract this" and should NOT pollute the manual metadata via the merge.

### 5.3 Edge cases

- Model returns text-only response (shouldn't happen with `ToolChoice::Tool` but defensive): `MissingToolUse`.
- Model calls a different tool: `MissingToolUse` (the find_map filters by name).
- Tool input has extra keys not in the schema: silently included in the result. Bedrock validates `required` fields but doesn't reject extras. This is fine — if the model invents `client` even though we didn't ask, it just goes through.
- Tool input missing required keys: Bedrock typically rejects this and resends to the model. If it slips through: `SchemaValidation`.

## 6. Configuration

### 6.1 Environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `MEETING_COMPANION_BEDROCK_REGION` | no | `us-west-2` | AWS region |
| `MEETING_COMPANION_BEDROCK_MODEL_ID` | no | `us.anthropic.claude-sonnet-4-7-...` (see §3.3) | Bedrock model id |
| `AWS_PROFILE` | no | `default` | AWS credential profile |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` | no | — | Static credentials (or use IAM role / IMDS) |
| `AWS_REGION` | no | — | Falls back here if `MEETING_COMPANION_BEDROCK_REGION` is unset |

The credentials chain follows the standard AWS SDK precedence: env vars → shared credentials file → IAM role / IMDS / SSO. No bespoke handling.

### 6.2 Server boot sequence

The current boot sequence ([`server.md`](server.md) §6.5) gains one step:

1. Parse CLI args + env (existing).
2. **NEW: Initialize `BedrockClient::from_env().await`. On failure → exit code 3 with logged error.**
3. Construct `ServerHandle` carrying `Arc<BedrockClient>` (existing handle gets one more field).
4. Spawn heartbeat task (existing).
5. Run accept loop (existing).

### 6.3 Local development without AWS

For developers without AWS credentials configured, the server still boots (the credential chain provides default credentials that Bedrock rejects on first call). Meetings can be started with no description (the existing path: no `description` → no extraction → no Bedrock call). Listening flow's `start_meeting { description }` triggers an extraction that fails at first call → `status { error }` toast in the PWA. Manual metadata still works.

If the user wants to do extraction-free local dev, they can set `MEETING_COMPANION_BEDROCK_DISABLED=1` to skip extraction entirely (this is a small dev-only escape hatch — see §6.4).

### 6.4 Disable flag (dev-only escape hatch)

```rust
// At the top of spawn_extraction:
if std::env::var("MEETING_COMPANION_BEDROCK_DISABLED").is_ok() {
    debug!("extraction disabled by env var; skipping");
    return;
}
```

Documented in the server README. Only intended for local dev sessions where the user wants the full pipeline minus AWS.

## 7. Test strategy

### 7.1 Unit tests in `bedrock.rs`

- `extraction_tool()` returns the expected schema (snapshot test using `serde_json::to_value`).
- `parse_response()` happy path: given a constructed `Message` with a valid tool_use, returns the expected map.
- `parse_response()` empty-string filtering: input `{"title": "Q1", "project": ""}` returns `{"title": "Q1"}` (project dropped).
- `parse_response()` missing tool_use: returns `ExtractionError::MissingToolUse`.
- `parse_response()` non-string field: returns `ExtractionError::SchemaValidation`.

### 7.2 Unit tests in `extraction.rs`

The existing `merge_manual_wins` test stays unchanged. The existing `extract_takes_first_8_words` test for the simulated stub is **deleted** (the function it tested is gone).

A new test for the Phase 2 `extract_metadata` would require a mock `BedrockClient`. Two approaches:

- **(a, chosen) Tests live in `bedrock.rs` only.** `extraction.rs` becomes a thin async wrapper that just calls into `BedrockClient::extract`. The wrapper has no logic to test independently.
- (b) Trait-abstract `BedrockClient` and provide a `MockBedrockClient`. Adds a layer; defer until needed.

### 7.3 Integration test (env-gated)

`packages/server/tests/bedrock_integration.rs`:

```rust
#[tokio::test]
async fn extracts_title_and_project_from_real_description() {
    if std::env::var("RUN_BEDROCK_INTEGRATION").is_err() {
        return; // skip — same pattern as PWA simulator integration tests
    }
    let client = BedrockClient::from_env().await.expect("client");
    let result = client.extract("Q1 budget review for the helix product launch")
        .await
        .expect("extraction");
    assert!(result.contains_key("title"));
    let title = &result["title"];
    assert!(!title.is_empty());
    assert!(title.split_whitespace().count() <= 8);
    // project is best-effort — might be "helix" or empty depending on model judgment.
}
```

### 7.4 Manual smoke

A `Justfile` recipe `just bedrock-smoke`:

```just
bedrock-smoke:
    cargo run -p meeting-companion-server --bin bedrock-smoke -- "Q1 budget review for helix"
```

Where `bedrock-smoke` is a small `examples/bedrock_smoke.rs` (or a `[[bin]]` alongside the server) that:

1. Boots `BedrockClient::from_env()`.
2. Calls `extract` with the CLI argument.
3. Prints the resulting map.

Useful for sanity-checking AWS credential setup outside the full server context.

## 8. Errors & failure modes

| Failure                                | SDK behavior                                   | Our handling                                                                           |
|----------------------------------------|------------------------------------------------|----------------------------------------------------------------------------------------|
| Throttling (`ThrottlingException`)     | SDK retries with exponential backoff (3 attempts) | Eventually surfaces as `ExtractionError::Timeout` if all retries exceed 8s          |
| Service 5xx error                      | SDK retries                                    | Same as above                                                                          |
| Connection error (DNS, TCP)            | SDK retries                                    | Same as above                                                                          |
| `AccessDeniedException` (bad IAM)      | Not retried                                    | `ExtractionError::Sdk("AccessDeniedException: ...")` → status toast                    |
| `ResourceNotFoundException` (bad model id) | Not retried                                | Same                                                                                   |
| Schema rejection (model output bad)    | Bedrock typically resends to model             | If still bad after retry: `SchemaValidation` → status toast                            |
| Model returns text instead of tool_use | n/a (with ToolChoice::Tool)                    | `MissingToolUse` → status toast                                                        |
| Tokio timeout (8s exceeded)            | SDK in-flight call aborted                     | `Timeout` → status toast                                                               |

All errors land as `status { error: "metadata extraction failed" }` events to the PWA. The server log carries the detailed cause for the developer.

The PWA's existing `error` toast machinery surfaces these. Manual metadata is preserved.

## 9. Cargo dependencies

Adds to `packages/server/Cargo.toml`:

```toml
[dependencies]
aws-config = { version = "1", features = ["behavior-version-latest"] }
aws-sdk-bedrockruntime = "1"
thiserror = "2"
```

Versions reflect what's stable as of 2026-05-02; the implementer pins to current actuals.

## 10. Out of scope

- Real audio capture / STT / summarization — Phase 2 step 15 (separate spec).
- Dynamic mode catalog — Phase 2 step 17 (separate spec).
- Memory-system enrichment — Phase 2 step 18 (separate spec).
- Local-only LLMs (Ollama, llama.cpp).
- Custom inference parameters (temperature, top-k, top-p) — defaults are fine for extraction.
- Multi-turn or chain-of-thought prompts.
- Streaming responses.
- Multi-language description input — English only for v1.
- Cost monitoring beyond AWS billing alarms.
- Using Bedrock Converse API features beyond tool use (e.g., images, document inputs).

## 11. Open questions

None at time of writing. The exact `MEETING_COMPANION_BEDROCK_MODEL_ID` default constant value is a Phase 2 implementation-time confirmation against current Bedrock model id naming (the user must also enable the cross-region inference profile in the AWS console — one-time per account).
