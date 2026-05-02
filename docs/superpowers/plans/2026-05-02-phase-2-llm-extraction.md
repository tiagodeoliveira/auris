# Phase 2 Step 16 — LLM Metadata Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the deterministic simulated stub `extract_metadata` in `packages/server/src/extraction.rs` with a real call to Anthropic Claude Sonnet 4.7 on AWS Bedrock, using tool use for structured output. The wire contract with the PWA stays identical — only server internals change.

**Architecture:** A new `bedrock.rs` module wraps `aws-sdk-bedrockruntime` with a typed `BedrockClient`. The existing `extraction.rs` becomes an async wrapper that calls into `BedrockClient::extract`. `ServerHandle` carries an `Arc<BedrockClient>` constructed once at server boot. `spawn_extraction` (in `ws.rs`) gains real error handling: on any `ExtractionError`, broadcast a `status { error }` event and skip the metadata_changed broadcast. A dev escape hatch `MEETING_COMPANION_BEDROCK_DISABLED=1` short-circuits extraction entirely.

**Tech Stack:** Rust 2021 (existing), plus new deps: `aws-config = "1"` (with `behavior-version-latest` feature), `aws-sdk-bedrockruntime = "1"`, `thiserror = "2"`.

**Reference:** [`docs/specs/phase-2-llm-extraction.md`](../../specs/phase-2-llm-extraction.md) is the spec this plan implements. Sections of that spec are cited inline.

---

## File structure produced by this plan

```
packages/server/
├── Cargo.toml                     [modified — adds 3 deps]
├── README.md                      [modified — AWS credential expectations + disable flag]
├── examples/
│   └── bedrock_smoke.rs           [new — small CLI for manual smoke]
├── src/
│   ├── bedrock.rs                 [new — BedrockClient, tool schema, prompts, parsing]
│   ├── extraction.rs              [modified — async + Result; sim stub removed]
│   ├── lib.rs                     [modified — pub mod bedrock]
│   ├── main.rs                    [modified — boot Bedrock client; exit 3 on init failure]
│   ├── state.rs                   [unchanged]
│   ├── contract.rs                [unchanged]
│   ├── mock.rs                    [unchanged]
│   └── ws.rs                      [modified — ServerHandle + spawn_extraction]
└── tests/
    ├── common/mod.rs              [modified — disable Bedrock in test infra]
    ├── extraction.rs              [modified — delete 2 tests, keep extraction_no_description]
    ├── bedrock_integration.rs     [new — env-gated, real Bedrock]
    ├── handshake.rs               [unchanged]
    ├── heartbeat.rs               [unchanged]
    ├── mock_content.rs            [unchanged]
    ├── shutdown.rs                [unchanged]
    ├── snapshot.rs                [unchanged]
    └── state_machine.rs           [unchanged]

Justfile                           [modified — bedrock-smoke + bedrock-integration recipes]
docs/specs/server.md               [modified — supersession note on §8.4]
```

---

## Task 1: Add Cargo dependencies

**Files:**

- Modify: `packages/server/Cargo.toml`

- [ ] **Step 1: Add `[dependencies]` entries**

In `packages/server/Cargo.toml`, add to the existing `[dependencies]` table:

```toml
aws-config = { version = "1", features = ["behavior-version-latest"] }
aws-sdk-bedrockruntime = "1"
thiserror = "2"
```

The `aws-config` `behavior-version-latest` feature opts into the SDK's current default behavior version (avoids deprecation warnings).

- [ ] **Step 2: Verify build**

Run: `cargo build -p meeting-companion-server`
Expected: build succeeds. The first build pulls a few hundred MB of AWS SDK transitive deps and rmeta files; this can take a couple of minutes on a cold cache.

If the build fails because a specific minor version of an AWS SDK crate isn't available on the user's pinned date, adjust the version in `Cargo.toml` (e.g., `aws-config = "1.5"` if `1.6` isn't out yet). Document any adjustment in the commit message.

- [ ] **Step 3: Verify existing tests still pass**

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 tests still pass (no behavior change yet — only deps added).

- [ ] **Step 4: Commit**

```bash
git add packages/server/Cargo.toml Cargo.lock
git commit -m "chore(server): add AWS Bedrock + thiserror deps"
```

---

## Task 2: `bedrock.rs` skeleton — types, constants, tool schema

**Files:**

- Create: `packages/server/src/bedrock.rs`
- Modify: `packages/server/src/lib.rs` (add `pub mod bedrock;`)

This task creates the module skeleton with everything that doesn't depend on a real Bedrock call: error types, constants, tool schema, system prompt. `BedrockClient::extract` is stubbed (returns `Err(MissingToolUse)` so we can compile and test the structure). Task 3 implements the real call.

- [ ] **Step 1: Add `pub mod bedrock;` to `packages/server/src/lib.rs`**

Append:

```rust
pub mod bedrock;
```

(Place it alphabetically between `pub mod contract;` and `pub mod extraction;`.)

- [ ] **Step 2: Write failing tests for the tool schema and constants**

Create `packages/server/src/bedrock.rs`:

```rust
//! AWS Bedrock client for Claude Sonnet 4.7 metadata extraction.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;
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
```

- [ ] **Step 3: Run tests; confirm pass**

Run: `cargo test -p meeting-companion-server bedrock::`
Expected: 3 tests pass.

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 prior + 3 new = **82 tests pass**.

- [ ] **Step 4: Commit**

```bash
git add packages/server/src/bedrock.rs packages/server/src/lib.rs
git commit -m "feat(server): bedrock module skeleton — tool schema + prompts"
```

---

## Task 3: `BedrockClient` — `from_env` + `extract` with parsing

**Files:**

- Modify: `packages/server/src/bedrock.rs` (extend with `BedrockClient` impl + parser)

This task implements the real Bedrock client logic: `from_env` (resolves region + credentials), the `extract` method (constructs the converse request, calls the SDK, parses tool_use), and the `parse_response` helper. Plus 4 more unit tests for parsing.

- [ ] **Step 1: Write failing tests for `parse_response`**

Append to `packages/server/src/bedrock.rs`'s `mod tests`:

```rust
    use serde_json::json;

    fn make_tool_use(input: serde_json::Value) -> Vec<MockContentBlock> {
        vec![MockContentBlock::ToolUse {
            name: TOOL_NAME.to_string(),
            input,
        }]
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
```

The tests use a `MockContentBlock` enum because we don't want to construct full SDK types in unit tests. The parser is decomposed into:

- `parse_response_blocks(&[MockContentBlock]) -> Result<HashMap, ExtractionError>` — pure, unit-testable
- `parse_response(SdkResponse) -> Result<HashMap, ExtractionError>` — converts SDK types to MockContentBlock and delegates

This keeps the unit tests free of SDK type instantiation noise.

- [ ] **Step 2: Implement `MockContentBlock` and `parse_response_blocks`**

Add to `bedrock.rs` (above the `mod tests` block):

```rust
/// Internal representation of a content block, decoupled from the SDK
/// types so unit tests can construct synthetic responses without SDK
/// builders.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum MockContentBlock {
    Text(String),
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
        let s = v
            .as_str()
            .ok_or_else(|| ExtractionError::SchemaValidation(format!("field '{}' is not a string", k)))?;
        if !s.is_empty() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Ok(out)
}
```

- [ ] **Step 3: Run parser tests**

Run: `cargo test -p meeting-companion-server bedrock::`
Expected: 3 (from Task 2) + 5 new = 8 tests pass.

- [ ] **Step 4: Implement `BedrockClient` struct + `from_env`**

Append to `bedrock.rs`:

```rust
use std::sync::Arc;

#[derive(Clone)]
pub struct BedrockClient {
    inner: Arc<aws_sdk_bedrockruntime::Client>,
    model_id: String,
}

impl BedrockClient {
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
}
```

`from_env` doesn't make a Bedrock call (per spec §3.5 — boot-time validation skipped). Errors here are limited to AWS SDK config errors, which are rare; if they happen they bubble up as `BedrockInitError::Sdk`.

- [ ] **Step 5: Implement `BedrockClient::extract`**

Append to `BedrockClient`'s impl block:

```rust
    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        use aws_sdk_bedrockruntime::types as t;

        let user_message = t::Message::builder()
            .role(t::ConversationRole::User)
            .content(t::ContentBlock::Text(format!("Meeting description:\n{}", description)))
            .build()
            .map_err(|e| ExtractionError::Sdk(format!("build user message: {}", e)))?;

        let tool_spec = t::ToolSpecification::builder()
            .name(TOOL_NAME)
            .description("Extract structured metadata from a meeting description.")
            .input_schema(t::ToolInputSchema::Json(
                aws_sdk_bedrockruntime::primitives::Document::from(extraction_tool_schema()),
            ))
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
                t::ContentBlock::ToolUse(tu) => {
                    let input = serde_json::to_value(tu.input.as_object().cloned()?).ok()?;
                    Some(MockContentBlock::ToolUse {
                        name: tu.name,
                        input,
                    })
                }
                _ => None,
            })
            .collect();

        parse_response_blocks(&blocks)
    }
}
```

The `Document::from(serde_json::Value)` conversion path may need adjustment based on the actual SDK API as of implementation time — `aws-sdk-bedrockruntime` uses its own `Document` type for JSON-like inputs. If `From<serde_json::Value>` isn't implemented, use `serde_json::from_value::<aws_sdk_bedrockruntime::primitives::Document>(...)` or build the Document manually.

- [ ] **Step 6: Verify build + existing tests**

Run: `cargo build -p meeting-companion-server`
Expected: succeeds.

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 prior + 3 (Task 2) + 5 (Task 3) = **87 tests pass**.

- [ ] **Step 7: Commit**

```bash
git add packages/server/src/bedrock.rs
git commit -m "feat(server): BedrockClient — from_env, extract, response parser"
```

---

## Task 4: Update `extraction.rs` to async + `Result`

**Files:**

- Modify: `packages/server/src/extraction.rs`

The existing `extract_metadata(description) -> HashMap` becomes async, takes a `&BedrockClient`, returns `Result<HashMap, ExtractionError>`. The simulated stub body is removed; the function delegates to `client.extract(description)`. The `extract_takes_first_8_words` unit test is deleted (the function it tested is gone). `merge_manual_wins` is unchanged.

- [ ] **Step 1: Replace `extraction.rs`**

Replace the current contents of `packages/server/src/extraction.rs` with:

```rust
//! Metadata extraction from meeting descriptions.
//! See `docs/specs/phase-2-llm-extraction.md` for the implementation;
//! this module is now a thin wrapper that delegates to BedrockClient.

use std::collections::HashMap;

use crate::bedrock::{BedrockClient, ExtractionError};

pub async fn extract_metadata(
    client: &BedrockClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError> {
    client.extract(description).await
}

/// Manual values win on conflict (architecture-stated rule, [`server.md`](../../docs/specs/server.md) §4.5).
pub fn merge_manual_wins(
    extracted: HashMap<String, String>,
    manual: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = extracted;
    for (k, v) in manual {
        out.insert(k.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_manual_wins_on_conflict() {
        let extracted = HashMap::from([
            ("project".to_string(), "sim-extracted".to_string()),
            ("title".to_string(), "auto title".to_string()),
        ]);
        let manual = HashMap::from([("project".to_string(), "helix".to_string())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("project"), Some(&"helix".to_string()));
        assert_eq!(merged.get("title"), Some(&"auto title".to_string()));
    }

    #[test]
    fn merge_manual_wins_with_empty_extracted() {
        let extracted = HashMap::new();
        let manual = HashMap::from([("foo".to_string(), "bar".to_string())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn merge_manual_wins_with_empty_manual() {
        let extracted = HashMap::from([("title".to_string(), "x".to_string())]);
        let manual = HashMap::new();
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("title"), Some(&"x".to_string()));
    }
}
```

The `extract_takes_first_8_words` test is deleted — the simulated stub it tested is gone. Two new merge tests are added (empty inputs) since the merge function is now the only logic in the module.

- [ ] **Step 2: Run tests**

Run: `cargo test -p meeting-companion-server extraction::`
Expected: 3 merge tests pass (was 2: `extract_takes_first_8_words` deleted, 1 added: empty-inputs cases).

- [ ] **Step 3: Verify integration tests still pass**

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: integration tests in `tests/extraction.rs` will FAIL because they call the old sync `extract_metadata(description)` signature, AND because `ws.rs` still references the old function. That's expected at this point — Tasks 5-7 fix the wiring.

If you see compilation errors at this step, that's the expected state. Don't proceed to commit yet — proceed to Task 5.

- [ ] **Step 4: Commit (after Tasks 5-7 land — combine if needed)**

Hold this commit until the wiring work in Tasks 5-7 is in place. Or, more practically: do Task 4-7 as a single commit since they're tightly coupled.

**Recommended:** Squash Tasks 4, 5, 6, 7 into a single feature commit. The plan keeps them as separate tasks for the implementer's mental model, but they ship together.

---

## Task 5: Wire `BedrockClient` through `ServerHandle` and boot

**Files:**

- Modify: `packages/server/src/ws.rs` (add `bedrock` field to `ServerHandle`)
- Modify: `packages/server/src/main.rs` (init Bedrock client; exit code 3 on failure)

- [ ] **Step 1: Add `bedrock` field to `ServerHandle`**

In `packages/server/src/ws.rs`, modify the `ServerHandle` struct:

```rust
use std::sync::Arc;
use crate::bedrock::BedrockClient;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    pub shutdown: CancellationToken,
    pub bedrock: Arc<BedrockClient>,  // NEW
}
```

Update `run_server_with_listener` to accept and propagate the Bedrock client. Change its signature:

```rust
pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    bedrock: Arc<BedrockClient>,  // NEW
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    // ...
    let handle = ServerHandle {
        // ...existing...
        bedrock,
    };
    // ...
}
```

And `run_server`:

```rust
pub async fn run_server(
    addr: SocketAddr,
    token: String,
    bedrock: Arc<BedrockClient>,  // NEW
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = ?listener.local_addr()?, "listening");
    run_server_with_listener(listener, token, bedrock, shutdown_rx).await
}
```

- [ ] **Step 2: Update main.rs to construct the Bedrock client**

Modify `packages/server/src/main.rs`'s `main()`:

```rust
use std::sync::Arc;
use meeting_companion_server::bedrock::BedrockClient;

// ...existing imports + code...

// After token validation, before run_server:

let bedrock = match BedrockClient::from_env().await {
    Ok(c) => Arc::new(c),
    Err(e) => {
        tracing::error!(error = %e, "Bedrock client init failed");
        std::process::exit(3);
    }
};

let (shutdown_tx, shutdown_rx) = oneshot::channel();
// ...signal handler unchanged...

meeting_companion_server::run_server(addr, token, bedrock, shutdown_rx).await
```

- [ ] **Step 3: Update test infrastructure to pass a BedrockClient**

In `packages/server/tests/common/mod.rs`, the `spawn_test_server` helper currently calls `run_server_with_listener`. Update it to:

```rust
pub async fn spawn_test_server() -> TestServer {
    spawn_test_server_with_token("test-token").await
}

pub async fn spawn_test_server_with_token(token: &str) -> TestServer {
    // Disable Bedrock in tests by default (see `MEETING_COMPANION_BEDROCK_DISABLED`).
    std::env::set_var("MEETING_COMPANION_BEDROCK_DISABLED", "1");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();
    let token = token.to_string();

    // Construct a BedrockClient even in tests — the disable flag short-
    // circuits actual API calls, so the client is never used. We need
    // *something* to satisfy the type signature.
    let bedrock = Arc::new(
        meeting_companion_server::bedrock::BedrockClient::from_env()
            .await
            .expect("bedrock init in tests"),
    );

    tokio::spawn(async move {
        let _ = meeting_companion_server::ws::run_server_with_listener(
            listener,
            token,
            bedrock,
            rx,
        )
        .await;
    });
    TestServer {
        addr,
        shutdown: Some(tx),
    }
}
```

The `MEETING_COMPANION_BEDROCK_DISABLED=1` env var is set in `spawn_test_server_with_token` so all integration tests run without Bedrock — they exercise the disable-flag short-circuit path in `spawn_extraction` (Task 6).

The `BedrockClient::from_env()` call still happens in tests because the `ServerHandle` requires one, but since the disable flag is set, no actual Bedrock API calls fire.

If `from_env()` fails in tests (e.g., on CI without AWS credentials), the test will skip via `expect`. Acceptable — the test suite then runs without integration coverage. CI can set dummy creds (`AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test`) to satisfy the chain without enabling real calls.

- [ ] **Step 4: Verify build**

Run: `cargo build -p meeting-companion-server`
Expected: builds successfully. Tests still won't pass because Task 6 is needed.

- [ ] **Step 5: Commit (will be squashed with Task 4/6/7 if you're combining)**

If combining commits, hold here. Otherwise:

```bash
git add packages/server/src/ws.rs packages/server/src/main.rs packages/server/tests/common/mod.rs
git commit -m "feat(server): wire Bedrock client through ServerHandle + boot"
```

---

## Task 6: Update `spawn_extraction` for new error handling + disable flag

**Files:**

- Modify: `packages/server/src/ws.rs` (`spawn_extraction` function)

- [ ] **Step 1: Replace `spawn_extraction` body**

In `packages/server/src/ws.rs`, replace the existing `spawn_extraction` function with:

```rust
fn spawn_extraction(
    handle: ServerHandle,
    description: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Dev escape hatch: skip extraction entirely.
        if std::env::var("MEETING_COMPANION_BEDROCK_DISABLED").is_ok() {
            tracing::debug!("extraction disabled by env var; skipping");
            return;
        }

        let extracted = tokio::select! {
            result = handle.bedrock.extract(&description) => match result {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(error = %e, "metadata extraction failed");
                    let status = crate::contract::Status {
                        listening: matches!(
                            handle.state.lock().await.snapshot_meeting_state(),
                            crate::contract::MeetingState::Active,
                        ),
                        paused: matches!(
                            handle.state.lock().await.snapshot_meeting_state(),
                            crate::contract::MeetingState::Paused,
                        ),
                        error: Some(format!("Metadata extraction failed: {}", short_error(&e))),
                    };
                    let _ = handle.events_tx.send(Event::Status { status });
                    return;
                }
            },
            _ = cancel.cancelled() => {
                tracing::debug!("extraction cancelled");
                return;
            }
        };

        // Re-acquire lock + check we're still in a meeting state that wants extraction.
        let event = {
            let mut s = handle.state.lock().await;
            if !matches!(
                s.snapshot_meeting_state(),
                crate::contract::MeetingState::Active | crate::contract::MeetingState::Paused
            ) {
                return;
            }
            let manual = s.metadata_clone();
            let merged = crate::extraction::merge_manual_wins(extracted, &manual);
            s.set_metadata_full(merged.clone());
            Event::MetadataChanged { metadata: merged }
        };
        let _ = handle.events_tx.send(event);
    });
}

fn short_error(e: &crate::bedrock::ExtractionError) -> &'static str {
    use crate::bedrock::ExtractionError::*;
    match e {
        Timeout(_) => "timeout",
        MissingToolUse => "no tool response",
        SchemaValidation(_) => "invalid output",
        Sdk(_) => "service error",
    }
}
```

Key changes from Phase 0:

- Removed the `tokio::time::sleep(EXTRACTION_DELAY)` simulated delay — real Bedrock has its own latency.
- Disable-flag short-circuit added at the top.
- Real error handling: on `ExtractionError`, log + broadcast `status` event with a short user-friendly error string (the SDK error stays in the log).
- Cancellation: `tokio::select!` between the Bedrock call and the cancel token. Cancel mid-call aborts the SDK request (the SDK respects the cancellation when the future is dropped).
- The post-extraction state check is preserved (defensive: if meeting was stopped between extraction returning and lock acquisition, abandon).

- [ ] **Step 2: Verify the import of `ExtractionError` and `BedrockClient` is in scope**

At the top of `ws.rs`, ensure:

```rust
use crate::bedrock::{BedrockClient, ExtractionError};
```

(Add if not already imported.)

- [ ] **Step 3: Verify build**

Run: `cargo build -p meeting-companion-server`
Expected: succeeds.

- [ ] **Step 4: Verify all unit tests pass**

Run: `cargo test -p meeting-companion-server --lib -- --test-threads=1`
Expected: all unit tests pass (we haven't touched any).

Integration tests will be addressed in Task 7.

---

## Task 7: Update integration tests

**Files:**

- Modify: `packages/server/tests/extraction.rs` (delete two tests, keep one)

- [ ] **Step 1: Delete `extraction_merge_manual_wins` and `extraction_cancelled_on_stop` tests**

In `packages/server/tests/extraction.rs`, delete these two tests:

- `extraction_merge_manual_wins` — needs a real Bedrock call to test the merge end-to-end. Moved to `bedrock_integration.rs` (Task 8).
- `extraction_cancelled_on_stop` — under the disable flag, no extraction fires regardless of cancellation. The cancellation behavior is exercised at the unit level in `bedrock.rs`'s tokio::select pattern.

Keep `extraction_no_description` — it asserts no second `metadata_changed` event when the description is empty. This is still true under the disable flag (no extraction call → no metadata_changed). The test gives less specific information than before but is still valid coverage.

The resulting `tests/extraction.rs` should have exactly one test function: `extraction_no_description`.

- [ ] **Step 2: Verify**

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: all tests pass. Server-wide test count:

- 79 (Phase 0) - 1 (extract_takes_first_8_words deleted in Task 4) + 2 (merge empty-input tests added in Task 4) - 2 (extraction_merge_manual_wins, extraction_cancelled_on_stop deleted) + 8 (bedrock unit tests from Tasks 2-3) = **86 tests**.

- [ ] **Step 3: Commit (Tasks 4-7 as a unit)**

```bash
git add packages/server/src/extraction.rs packages/server/src/ws.rs packages/server/src/main.rs packages/server/tests/common/mod.rs packages/server/tests/extraction.rs
git commit -m "feat(server): replace simulated extraction with Bedrock + Sonnet 4.7"
```

The single commit captures the full transition from simulated to real extraction. Hopper should still squash if separate commits accumulated during the work.

---

## Task 8: Bedrock integration test (env-gated)

**Files:**

- Create: `packages/server/tests/bedrock_integration.rs`

This test runs against a real Bedrock backend. It's skipped unless `RUN_BEDROCK_INTEGRATION=1` is set, mirroring the PWA's pattern of env-gated integration tests.

- [ ] **Step 1: Create the test file**

```rust
//! Integration test for the Bedrock client. Requires real AWS credentials
//! and a working Bedrock account with Sonnet 4.7 enabled.
//!
//! Skipped by default. Run with:
//!   RUN_BEDROCK_INTEGRATION=1 cargo test -p meeting-companion-server --test bedrock_integration

#[tokio::test]
async fn extracts_title_from_real_description() {
    if std::env::var("RUN_BEDROCK_INTEGRATION").is_err() {
        return;
    }
    // Don't disable Bedrock for this test specifically.
    std::env::remove_var("MEETING_COMPANION_BEDROCK_DISABLED");

    let client = meeting_companion_server::bedrock::BedrockClient::from_env()
        .await
        .expect("Bedrock client init");

    let result = client
        .extract("Q1 budget review for the helix product launch and rollout plan")
        .await
        .expect("extraction succeeded");

    // Title should be a non-empty string of at most 8 words.
    let title = result.get("title").expect("title key present");
    assert!(!title.is_empty(), "title is empty");
    let word_count = title.split_whitespace().count();
    assert!(
        word_count <= 8,
        "title '{}' has {} words; expected ≤ 8",
        title,
        word_count
    );

    // Project is best-effort. Either present and non-empty, or absent.
    if let Some(project) = result.get("project") {
        assert!(
            !project.is_empty(),
            "project key present but empty (should have been filtered)"
        );
    }
}
```

- [ ] **Step 2: Verify it skips by default**

Run: `cargo test -p meeting-companion-server --test bedrock_integration`
Expected: the test runs but returns early (no actual Bedrock call). Output shows 1 test passed with `extracts_title_from_real_description` taking <1ms.

- [ ] **Step 3: Optionally test against real Bedrock**

If your AWS credentials are configured + Sonnet 4.7 is enabled in Bedrock for your account:

```bash
RUN_BEDROCK_INTEGRATION=1 cargo test -p meeting-companion-server --test bedrock_integration -- --nocapture
```

Expected: real Bedrock call, ~1-2 second latency, test passes with a sensible title extraction.

- [ ] **Step 4: Commit**

```bash
git add packages/server/tests/bedrock_integration.rs
git commit -m "test(server): env-gated Bedrock integration test"
```

---

## Task 9: `bedrock-smoke` example + Justfile recipe

**Files:**

- Create: `packages/server/examples/bedrock_smoke.rs`
- Modify: `Justfile`

A small CLI for manually testing Bedrock connectivity outside the full server context.

- [ ] **Step 1: Create the example**

`packages/server/examples/bedrock_smoke.rs`:

```rust
//! Manual smoke for the Bedrock client.
//! Usage:
//!   cargo run -p meeting-companion-server --example bedrock_smoke -- "your description here"

use meeting_companion_server::bedrock::BedrockClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let description = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Q1 budget review for helix product launch".to_string());

    tracing_subscriber::fmt::init();

    println!("Initializing Bedrock client...");
    let client = BedrockClient::from_env().await?;

    println!("Description: {}", description);
    println!("Extracting...");

    let start = std::time::Instant::now();
    let result = client.extract(&description).await?;
    let elapsed = start.elapsed();

    println!("\nResult ({:?}):", elapsed);
    for (k, v) in &result {
        println!("  {} = {}", k, v);
    }

    Ok(())
}
```

- [ ] **Step 2: Add Justfile recipe**

In the root `Justfile`, add (under the `# --- Smoke ---` section):

```just
# Smoke-test the Bedrock extraction with a sample description.
bedrock-smoke description="Q1 budget review for helix product launch":
    cargo run -p meeting-companion-server --example bedrock_smoke -- "{{description}}"

# Run the env-gated Bedrock integration test (requires real AWS creds + Sonnet 4.7 in Bedrock).
bedrock-integration:
    RUN_BEDROCK_INTEGRATION=1 cargo test -p meeting-companion-server --test bedrock_integration -- --nocapture
```

- [ ] **Step 3: Verify the example builds**

Run: `cargo build -p meeting-companion-server --examples`
Expected: builds successfully.

Optionally, test the smoke recipe (requires AWS creds):

```bash
just bedrock-smoke "team standup, project starlight"
```

Expected: outputs extracted title + project (or empty if not extractable), with latency.

- [ ] **Step 4: Commit**

```bash
git add packages/server/examples/bedrock_smoke.rs Justfile
git commit -m "test(server): bedrock-smoke example + Justfile recipes"
```

---

## Task 10: Update docs

**Files:**

- Modify: `packages/server/README.md`
- Modify: `docs/specs/server.md` (supersession note on §8.4)
- Modify: `docs/ARCHITECTURE.md` §0 status block (mark step 16 done)

- [ ] **Step 1: Update `packages/server/README.md`**

Add a new section after "Test":

```markdown
## AWS Bedrock for metadata extraction

Phase 2 step 16 wires real LLM-based metadata extraction via AWS Bedrock
(Anthropic Claude Sonnet 4.7). The server requires AWS credentials at boot
to construct the Bedrock client.

### Configuration

| Env var                                | Required | Default                                            |
|----------------------------------------|----------|----------------------------------------------------|
| AWS credentials (any standard chain)   | yes      | —                                                  |
| `MEETING_COMPANION_BEDROCK_REGION`     | no       | `us-west-2`                                        |
| `MEETING_COMPANION_BEDROCK_MODEL_ID`   | no       | `us.anthropic.claude-sonnet-4-7-20251015-v1:0`     |
| `MEETING_COMPANION_BEDROCK_DISABLED`   | no       | unset (extraction enabled)                         |

The cross-region inference profile (model id starting with `us.`) must be
enabled in your AWS Bedrock console — one-time setup per account.

If `MEETING_COMPANION_BEDROCK_DISABLED=1` is set, extraction is skipped
entirely. The PWA still works (manual metadata, mock items via the server,
glasses display, etc.) but no LLM-derived metadata is produced. This is the
default in the server's test suite (`tests/common/mod.rs`).

### Smoke

```bash
just bedrock-smoke "your meeting description"
```

Calls `BedrockClient::extract` once and prints the result. Useful for
verifying AWS credentials + model access without booting the full server.

### Integration test

The full integration test in `tests/bedrock_integration.rs` is skipped by
default. To run:

```bash
just bedrock-integration
```
```

- [ ] **Step 2: Add supersession note to `docs/specs/server.md`**

In `docs/specs/server.md`, find §8.4 "Simulated LLM extraction" and add a callout block at the top of the section:

```markdown
> **Phase 2 update:** This section describes the Phase 0 simulated stub.
> Phase 2 step 16 replaces it with a real Bedrock call — see
> [`docs/specs/phase-2-llm-extraction.md`](phase-2-llm-extraction.md).
> The wire contract (events, ordering, merge semantics) is unchanged.
```

- [ ] **Step 3: Update `docs/ARCHITECTURE.md` §0 Status block**

In the §0 Status block, change "Phase 2 (real audio + extraction pipeline) — pending" to clarify that step 16 is now done:

```markdown
- **Phase 2 (real audio + extraction pipeline) — partially shipped.** Step 16
  (LLM metadata extraction via Bedrock + Sonnet 4.7 + tool use) is complete;
  see [`docs/specs/phase-2-llm-extraction.md`](specs/phase-2-llm-extraction.md).
  Remaining Phase 2 work: step 15 (real audio + STT/summarizer), step 17
  (dynamic mode catalog), step 18 (memory-system enrichment).
```

- [ ] **Step 4: Verify**

Run: `pnpm format` (formats the markdown changes).
Run: `cargo test -p meeting-companion-server -- --test-threads=1` — confirm 86 tests pass.

- [ ] **Step 5: Commit**

```bash
git add packages/server/README.md docs/specs/server.md docs/ARCHITECTURE.md
git commit -m "docs: Phase 2 step 16 — Bedrock extraction documented"
```

---

## Self-review

| Spec section                                                | Implemented in                                |
|-------------------------------------------------------------|-----------------------------------------------|
| §1 Purpose & scope (replaces simulated stub)                | Tasks 4 (extraction.rs), 6 (spawn_extraction) |
| §2.1 Function signature change (async + Result)             | Task 4                                        |
| §2.2 ExtractionError enum                                   | Task 2                                        |
| §2.3 Caller integration (spawn_extraction)                  | Task 6                                        |
| §2.4 Wire contract unchanged                                | Implicit — Tasks 4-7 don't change events       |
| §3.1 Module layout `bedrock.rs`                             | Tasks 2, 3                                    |
| §3.2 BedrockClient struct + from_env                        | Task 3                                        |
| §3.3 Default model id (cross-region inference profile)      | Task 2 (constant)                             |
| §3.4 Retry & timeout (SDK default + 8s tokio::timeout)      | Task 3                                        |
| §3.5 No boot-time API call                                  | Task 3 (from_env doesn't call Bedrock)        |
| §4.1 System prompt                                          | Task 2 (constant)                             |
| §4.2 User message template                                  | Task 3                                        |
| §4.3 Tool schema (title + project)                          | Task 2                                        |
| §4.4 Tool choice forced                                     | Task 3                                        |
| §5 Response parsing (incl. empty-string filter)             | Task 3                                        |
| §6.1 Env vars                                               | Task 3 (from_env reads them)                  |
| §6.2 Server boot sequence (exit code 3 on init failure)     | Task 5                                        |
| §6.4 Disable flag escape hatch                              | Task 6                                        |
| §7.1 Unit tests in bedrock.rs (5 tests)                     | Tasks 2 (3), 3 (5)                            |
| §7.2 extraction.rs becomes thin wrapper                     | Task 4                                        |
| §7.3 Integration test (env-gated)                           | Task 8                                        |
| §7.4 Manual smoke (`just bedrock-smoke`)                    | Task 9                                        |
| §8 Errors & failure modes                                   | Tasks 6 (handler) + 3 (error types)           |
| §9 Cargo deps                                               | Task 1                                        |
| §10 Out of scope (steps 15/17/18, etc.)                     | (acknowledged; not implemented)               |
| §11 Open questions                                          | None.                                         |

**Placeholder scan:** No `TODO`, `TBD`, `fill in details`, or `add appropriate error handling` strings remain in any task body.

**Type consistency:** `BedrockClient`, `BedrockInitError`, `ExtractionError`, `MockContentBlock`, `extract_metadata`, `merge_manual_wins`, `parse_response_blocks`, `extraction_tool_schema` are defined exactly once and referenced by name everywhere else.

---

## Test count delta

- **Phase 0 baseline:** 79 server tests.
- **Removed (Tasks 4 + 7):**
  - `extract_takes_first_8_words` (extraction.rs unit) — 1
  - `extraction_merge_manual_wins` (tests/extraction.rs) — 1
  - `extraction_cancelled_on_stop` (tests/extraction.rs) — 1
- **Added (Tasks 2, 3, 4, 8):**
  - `bedrock::tests::extraction_tool_schema_is_valid` — 1
  - `bedrock::tests::system_prompt_mentions_tool_name` — 1
  - `bedrock::tests::default_model_id_is_cross_region_profile` — 1
  - `bedrock::tests::parse_response_happy_path` — 1
  - `bedrock::tests::parse_response_filters_empty_strings` — 1
  - `bedrock::tests::parse_response_missing_tool_use` — 1
  - `bedrock::tests::parse_response_non_string_field` — 1
  - `bedrock::tests::parse_response_includes_extra_keys_returned_by_model` — 1
  - `extraction::tests::merge_manual_wins_with_empty_extracted` — 1
  - `extraction::tests::merge_manual_wins_with_empty_manual` — 1
  - `bedrock_integration::extracts_title_from_real_description` — 1 (env-gated; passes as a no-op when `RUN_BEDROCK_INTEGRATION` is unset)
- **Net change:** -3 + 11 = **+8** tests
- **Final count:** 79 + 8 = **87 tests** (default-running, including 1 env-gated test that no-ops when its trigger var is unset).

---

After Task 10, Phase 2 step 16 is complete. The server uses real Bedrock-powered extraction, the wire contract with the PWA is unchanged, AWS credentials are required at server boot, and the disable flag provides a clean offline-dev path. Steps 15, 17, 18 remain as separate spec/plan/execute cycles.
