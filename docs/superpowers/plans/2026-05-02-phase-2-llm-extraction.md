# Phase 2 Step 16 — LLM Metadata Extraction (rig) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the deterministic simulated stub `extract_metadata` in `packages/server/src/extraction.rs` with a real call to Anthropic Claude Sonnet 4.7 via the [rig](https://github.com/0xPlaygrounds/rig) framework's `Extractor` pattern, using `rig-bedrock` as the provider crate. The wire contract with the PWA is unchanged.

**Architecture:** A new `llm.rs` module wraps a rig `Extractor<bedrock::CompletionModel, ExtractedMetadata>`. The `ExtractedMetadata` struct uses `JsonSchema` + `Deserialize` derives so rig generates the tool-use schema and parses the response back into a typed value automatically. `LlmClient::from_env` resolves region + model id + AWS credentials (via the standard SDK chain that `rig-bedrock` consumes), constructs the Bedrock provider client, and builds the Extractor. `LlmClient::extract` wraps the Extractor call in `tokio::time::timeout(8s)` and converts the typed result into `HashMap<String, String>` (filtering empty fields). `ServerHandle` carries an `Arc<LlmClient>`. `spawn_extraction` in `ws.rs` gains real error handling: on any `ExtractionError`, broadcast a `status { error }` event. A dev escape hatch `MEETING_COMPANION_LLM_DISABLED=1` short-circuits extraction.

**Tech Stack:** Rust 2021 (existing), plus new deps: `rig-core` ~`0.36` with `derive` feature, `rig-bedrock` ~`0.4`, `schemars` ~`0.8`, `thiserror` ~`2`.

**Reference:** [`docs/specs/phase-2-llm-extraction.md`](../../specs/phase-2-llm-extraction.md) is the spec this plan implements. Sections of that spec are cited inline.

---

## Why this plan is shorter than the Bedrock-direct draft

The Bedrock-direct plan was 10 tasks because it manually constructed tool schemas, manually parsed `ContentBlock::ToolUse` variants, manually mapped serde_json ↔ AWS Document types, and threaded the raw SDK client through. rig collapses all of that:

- Schema generation: `#[derive(JsonSchema)]` on `ExtractedMetadata`.
- Tool wiring: `extractor::<ExtractedMetadata>(model_id).build()`.
- Response parsing: `extractor.extract(&prompt).await -> Result<ExtractedMetadata, _>`.
- Retries: handled inside rig's transport.

What's left is environment plumbing, error mapping, and the wiring of `LlmClient` through `ServerHandle`. That fits in 6 tasks.

---

## File structure produced by this plan

```
packages/server/
├── Cargo.toml                     [modified — add 4 deps]
├── README.md                      [modified — AWS creds + LLM_DISABLED notes]
├── examples/
│   └── llm_smoke.rs               [new — small CLI for manual smoke]
├── src/
│   ├── extraction.rs              [modified — async + Result; sim stub removed]
│   ├── lib.rs                     [modified — pub mod llm]
│   ├── llm.rs                     [new — LlmClient + ExtractedMetadata + extract]
│   ├── main.rs                    [modified — boot LlmClient; exit 3 on init failure]
│   └── ws.rs                      [modified — ServerHandle + spawn_extraction]
└── tests/
    ├── common/mod.rs              [modified — disable LLM in test infra]
    ├── extraction.rs              [modified — delete 2 tests; keep extraction_no_description]
    └── llm_integration.rs         [new — env-gated, real LLM call]

Justfile                           [modified — llm-smoke + llm-integration recipes]
docs/specs/server.md               [modified — supersession note on §8.4]
docs/ARCHITECTURE.md               [modified — §0 status — step 16 shipped]
```

---

## Task 1: Add Cargo dependencies

**Files:**

- Modify: `packages/server/Cargo.toml`

- [ ] **Step 1: Look up current rig versions on crates.io**

Before editing, confirm the current published versions of `rig-core`, `rig-bedrock`, `schemars`, and `thiserror`. The plan references approximate versions (`0.36`, `0.4`, `0.8`, `2`) but the implementer should pin to whatever's current at the time of work.

```bash
cargo search rig-core --limit 1
cargo search rig-bedrock --limit 1
cargo search schemars --limit 1
cargo search thiserror --limit 1
```

- [ ] **Step 2: Add `[dependencies]` entries**

In `packages/server/Cargo.toml`, add to the existing `[dependencies]` table (alphabetical ordering):

```toml
rig-bedrock = "0.4"            # adjust to current
rig-core = { version = "0.36", features = ["derive"] }   # adjust to current
schemars = "0.8"               # adjust to current
thiserror = "2"
```

If `rig-core` doesn't have a `derive` feature on the current version (the feature names may have evolved), check the rig-core docs and use whatever activates `JsonSchema`-derived schemas for the Extractor. The intent is "structured-output extraction is wired up."

- [ ] **Step 3: Verify build**

Run: `cargo build -p meeting-companion-server`
Expected: builds. The first build pulls in rig + AWS SDK transitive deps and may take a couple of minutes on a cold cache.

If the build fails because a specific minor version isn't available, adjust the version. Document any adjustment in the commit message.

- [ ] **Step 4: Verify existing tests still pass**

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 tests still pass (no behavior change yet — only deps added).

- [ ] **Step 5: Commit**

```bash
git add packages/server/Cargo.toml Cargo.lock
git commit -m "chore(server): add rig + rig-bedrock + schemars + thiserror deps"
```

---

## Task 2: `llm.rs` — `ExtractedMetadata`, `LlmClient`, `from_env`, `extract`

**Files:**

- Create: `packages/server/src/llm.rs`
- Modify: `packages/server/src/lib.rs` (add `pub mod llm;`)

This task creates the entire LLM client module in one go. It's small enough not to need decomposition into "skeleton then implementation" the way the Bedrock-direct plan did — rig collapses most of the work.

- [ ] **Step 1: Read the current rig-core + rig-bedrock docs**

Before writing code, read at least:

- `https://docs.rs/rig-core/latest/rig/extractor/` (or the most current Extractor module path)
- `https://docs.rs/rig-bedrock/latest/rig_bedrock/` (provider client construction)

Confirm the actual API surface:

- How to construct a `bedrock::Client` (constructor name, async vs sync, region argument shape).
- How to call `.extractor::<T>()` on the client.
- The Extractor's `extract(&str)` method signature and error type.
- The exact path of `ExtractionError` (or whatever rig calls it) for our `From` impl.

If the API has shifted from what the spec sketches in §3.3-3.5, follow the docs. Don't fight the framework.

- [ ] **Step 2: Add `pub mod llm;` to `packages/server/src/lib.rs`**

Append, alphabetically between `pub mod extraction;` and `pub mod main;` (or wherever fits):

```rust
pub mod llm;
```

- [ ] **Step 3: Implement `packages/server/src/llm.rs`**

Use the spec's §3.2-3.5 code as a starting point. Adapt names per rig's actual API. The structure:

```rust
//! LLM-based metadata extraction via rig + rig-bedrock.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use thiserror::Error;
use tracing::info;

// rig + rig-bedrock imports — adjust paths per current crate APIs.
use rig::extractor::Extractor;
use rig::providers::bedrock;

const DEFAULT_REGION: &str = "us-west-2";
const DEFAULT_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-7-20251015-v1:0";
const SYSTEM_PROMPT: &str = "You are a meeting metadata extractor. \
Given a short spoken description of a meeting (transcribed by an STT system, \
may contain disfluencies and filler words), extract concise structured \
metadata. If a field cannot be confidently extracted from the description, \
return an empty string for that field — do not guess.";
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExtractedMetadata {
    /// Concise meeting title in 8 words or fewer. Empty string if not extractable.
    pub title: String,

    /// Project name if mentioned. Empty string if not extractable.
    pub project: String,
}

#[derive(Debug, Error)]
pub enum LlmInitError {
    #[error("LLM provider init failed: {0}")]
    Provider(String),
}

#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("LLM call exceeded timeout of {0:?}")]
    Timeout(Duration),

    #[error("Extraction failed: {0}")]
    Extract(String),
}

#[derive(Clone)]
pub struct LlmClient {
    extractor: Arc<Extractor<bedrock::completion::CompletionModel, ExtractedMetadata>>,
}

impl LlmClient {
    pub async fn from_env() -> Result<Self, LlmInitError> {
        let region = std::env::var("MEETING_COMPANION_LLM_REGION")
            .unwrap_or_else(|_| DEFAULT_REGION.to_string());
        let model_id = std::env::var("MEETING_COMPANION_LLM_MODEL_ID")
            .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string());

        // rig-bedrock client construction — exact shape depends on the crate's API.
        // Likely something like:
        let bedrock_client = bedrock::Client::new(&region)
            .await
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

    pub async fn extract(
        &self,
        description: &str,
    ) -> Result<HashMap<String, String>, ExtractionError> {
        let prompt = format!("Meeting description:\n{description}");

        let typed = tokio::time::timeout(EXTRACTION_TIMEOUT, self.extractor.extract(&prompt))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_map_drops_empty_title_only() {
        let m = ExtractedMetadata {
            title: "Q1 review".to_string(),
            project: String::new(),
        };
        let map = into_map(m);
        assert_eq!(map.get("title"), Some(&"Q1 review".to_string()));
        assert!(!map.contains_key("project"));
    }

    #[test]
    fn into_map_drops_both_when_empty() {
        let m = ExtractedMetadata {
            title: String::new(),
            project: String::new(),
        };
        let map = into_map(m);
        assert!(map.is_empty());
    }

    #[test]
    fn into_map_keeps_both_when_present() {
        let m = ExtractedMetadata {
            title: "T".to_string(),
            project: "P".to_string(),
        };
        let map = into_map(m);
        assert_eq!(map.get("title"), Some(&"T".to_string()));
        assert_eq!(map.get("project"), Some(&"P".to_string()));
    }

    #[test]
    fn system_prompt_mentions_extraction() {
        assert!(SYSTEM_PROMPT.to_lowercase().contains("extract"));
    }

    #[test]
    fn default_model_id_is_cross_region_profile() {
        assert!(DEFAULT_MODEL_ID.starts_with("us."));
        assert!(DEFAULT_MODEL_ID.contains("claude"));
    }
}
```

The `from_env` doesn't unit-test cleanly without making real provider calls — that's covered by the env-gated integration test in Task 4.

- [ ] **Step 4: Run tests**

Run: `cargo test -p meeting-companion-server llm::`
Expected: 5 unit tests pass.

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 prior + 5 new = **84 tests pass**.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p meeting-companion-server -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add packages/server/src/llm.rs packages/server/src/lib.rs
git commit -m "feat(server): rig-based LlmClient with Bedrock + Sonnet 4.7"
```

---

## Task 3: End-to-end transition (squashed)

**Files:**

- Modify: `packages/server/src/extraction.rs`
- Modify: `packages/server/src/ws.rs`
- Modify: `packages/server/src/main.rs`
- Modify: `packages/server/tests/common/mod.rs`
- Modify: `packages/server/tests/extraction.rs`

This task replaces the simulated stub end-to-end. It touches multiple files; the build is broken in intermediate states. Make all changes, then build, then commit as one feature commit.

- [ ] **Step 1: Update `packages/server/src/extraction.rs`**

Replace contents with:

```rust
//! Metadata extraction from meeting descriptions.
//! Phase 2 step 16: thin wrapper that delegates to LlmClient.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;

use crate::llm::{ExtractionError, LlmClient};

pub async fn extract_metadata(
    client: &LlmClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError> {
    client.extract(description).await
}

/// Manual values win on conflict (architecture-stated rule, server.md §4.5).
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
            ("project".to_string(), "extracted".to_string()),
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

The `extract_takes_first_8_words` test is deleted (the function it tested is gone). Two new merge edge-case tests are added.

- [ ] **Step 2: Update `packages/server/src/ws.rs` — `ServerHandle` + `run_server` + `spawn_extraction`**

Add `bedrock`/`llm` field to `ServerHandle`:

```rust
use std::sync::Arc;
use crate::llm::LlmClient;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    pub shutdown: CancellationToken,
    pub llm: Arc<LlmClient>,  // NEW
}
```

Update `run_server` and `run_server_with_listener` signatures:

```rust
pub async fn run_server(
    addr: SocketAddr,
    token: String,
    llm: Arc<LlmClient>,  // NEW
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = ?listener.local_addr()?, "listening");
    run_server_with_listener(listener, token, llm, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    llm: Arc<LlmClient>,  // NEW
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    // ... pass `llm` through to ServerHandle constructor ...
}
```

Replace `spawn_extraction` body:

```rust
fn spawn_extraction(
    handle: ServerHandle,
    description: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Dev escape hatch.
        if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
            tracing::debug!("LLM extraction disabled by env var; skipping");
            return;
        }

        let extracted = tokio::select! {
            result = handle.llm.extract(&description) => match result {
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
                        error: Some(short_error(&e)),
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

        // Re-acquire lock; abandon if meeting was stopped between extraction return + lock.
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

fn short_error(e: &crate::llm::ExtractionError) -> String {
    use crate::llm::ExtractionError::*;
    match e {
        Timeout(_) => "Metadata extraction timed out".to_string(),
        Extract(_) => "Metadata extraction failed".to_string(),
    }
}
```

Remove the `EXTRACTION_DELAY` constant and any `tokio::time::sleep` of it from this file — Bedrock has its own latency, no simulated delay needed.

- [ ] **Step 3: Update `packages/server/src/main.rs`**

In `main()`, after token validation, before `run_server`:

```rust
use std::sync::Arc;
use meeting_companion_server::llm::LlmClient;

// ...existing code...

let llm = match LlmClient::from_env().await {
    Ok(c) => Arc::new(c),
    Err(e) => {
        tracing::error!(error = %e, "LLM client init failed");
        std::process::exit(3);
    }
};

let (shutdown_tx, shutdown_rx) = oneshot::channel();
// ...signal handler unchanged...

meeting_companion_server::run_server(addr, token, llm, shutdown_rx).await
```

- [ ] **Step 4: Update `packages/server/tests/common/mod.rs`**

Set the disable flag at the top of `spawn_test_server_with_token` and construct an `LlmClient`:

```rust
pub async fn spawn_test_server_with_token(token: &str) -> TestServer {
    // Disable LLM extraction in tests by default.
    std::env::set_var("MEETING_COMPANION_LLM_DISABLED", "1");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();
    let token = token.to_string();

    let llm = Arc::new(
        meeting_companion_server::llm::LlmClient::from_env()
            .await
            .expect("LLM client init in tests"),
    );

    tokio::spawn(async move {
        let _ = meeting_companion_server::ws::run_server_with_listener(
            listener,
            token,
            llm,
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

The `LlmClient::from_env` call still happens (the type signature requires one), but `MEETING_COMPANION_LLM_DISABLED=1` ensures the actual extract path never fires from `spawn_extraction`.

If `LlmClient::from_env` itself fails because the AWS credential chain finds nothing on a CI machine, set `AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-west-2` in the CI environment. Document this in the server README under "Test".

- [ ] **Step 5: Update `packages/server/tests/extraction.rs`**

Delete `extraction_merge_manual_wins` and `extraction_cancelled_on_stop` integration tests — they tested the simulated stub's deterministic output and timing, which don't apply to the rig-backed path. Keep `extraction_no_description` (asserts no second `metadata_changed` when description is empty — still valid under the disable flag).

The resulting `tests/extraction.rs` should have exactly one test function: `extraction_no_description`.

- [ ] **Step 6: Verify build + tests**

Run: `cargo build -p meeting-companion-server`
Expected: builds cleanly.

Run: `cargo test -p meeting-companion-server -- --test-threads=1`
Expected: 79 (Phase 0) - 1 (extract_takes_first_8_words deleted in Step 1) + 2 (merge edge cases added in Step 1) + 5 (Task 2 llm tests) - 2 (extraction_merge_manual_wins, extraction_cancelled_on_stop deleted) = **83 tests pass**.

Run: `cargo clippy -p meeting-companion-server -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add packages/server/src/extraction.rs packages/server/src/ws.rs packages/server/src/main.rs packages/server/tests/common/mod.rs packages/server/tests/extraction.rs
git commit -m "feat(server): replace simulated extraction with rig + Sonnet 4.7"
```

---

## Task 4: LLM integration test (env-gated)

**Files:**

- Create: `packages/server/tests/llm_integration.rs`

- [ ] **Step 1: Create the integration test**

```rust
//! Integration test for the rig-backed LLM client. Requires real AWS
//! credentials and Sonnet 4.7 enabled in Bedrock.
//!
//! Skipped by default. Run with:
//!   RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration

#[tokio::test]
async fn extracts_title_from_real_description() {
    if std::env::var("RUN_LLM_INTEGRATION").is_err() {
        return;
    }
    std::env::remove_var("MEETING_COMPANION_LLM_DISABLED");

    let client = meeting_companion_server::llm::LlmClient::from_env()
        .await
        .expect("LLM client init");

    let result = client
        .extract("Q1 budget review for the helix product launch and rollout plan")
        .await
        .expect("extraction succeeded");

    let title = result.get("title").expect("title key present");
    assert!(!title.is_empty(), "title is empty");
    let word_count = title.split_whitespace().count();
    assert!(
        word_count <= 8,
        "title '{}' has {} words; expected ≤ 8",
        title,
        word_count
    );

    // project is best-effort. Either present and non-empty, or absent
    // (filtered out by into_map when the model returned an empty string).
    if let Some(project) = result.get("project") {
        assert!(!project.is_empty(), "project key present but empty");
    }
}
```

- [ ] **Step 2: Verify it skips by default**

Run: `cargo test -p meeting-companion-server --test llm_integration`
Expected: 1 test passes (returns early because `RUN_LLM_INTEGRATION` unset).

- [ ] **Step 3: Optionally test against real LLM**

If AWS credentials + Sonnet 4.7 are configured:

```bash
RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration -- --nocapture
```

- [ ] **Step 4: Commit**

```bash
git add packages/server/tests/llm_integration.rs
git commit -m "test(server): env-gated LLM integration test (rig)"
```

---

## Task 5: `llm-smoke` example + Justfile recipes

**Files:**

- Create: `packages/server/examples/llm_smoke.rs`
- Modify: `Justfile`

- [ ] **Step 1: Create the example**

`packages/server/examples/llm_smoke.rs`:

```rust
//! Manual smoke for the rig LLM client.
//! Usage:
//!   cargo run -p meeting-companion-server --example llm_smoke -- "your description"

use meeting_companion_server::llm::LlmClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let description = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Q1 budget review for helix product launch".to_string());

    tracing_subscriber::fmt::init();

    println!("Initializing LLM client...");
    let client = LlmClient::from_env().await?;

    println!("Description: {description}");
    println!("Extracting...");

    let start = std::time::Instant::now();
    let result = client.extract(&description).await?;
    let elapsed = start.elapsed();

    println!("\nResult ({:?}):", elapsed);
    for (k, v) in &result {
        println!("  {k} = {v}");
    }

    Ok(())
}
```

- [ ] **Step 2: Update Justfile**

In the root `Justfile`, replace any `bedrock-*` recipes (left over from the reverted Bedrock-direct work; if none, just add) with:

```just
# Smoke-test the LLM extraction with a sample description.
llm-smoke description="Q1 budget review for helix product launch":
    cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Run the env-gated LLM integration test (requires real AWS creds + Sonnet 4.7 in Bedrock).
llm-integration:
    RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration -- --nocapture
```

- [ ] **Step 3: Verify example builds**

Run: `cargo build -p meeting-companion-server --examples`
Expected: builds.

Optionally:

```bash
just llm-smoke "team standup, project starlight"
```

- [ ] **Step 4: Commit**

```bash
git add packages/server/examples/llm_smoke.rs Justfile
git commit -m "test(server): llm-smoke example + Justfile recipes"
```

---

## Task 6: Doc updates

**Files:**

- Modify: `packages/server/README.md`
- Modify: `docs/specs/server.md` (supersession callout on §8.4)
- Modify: `docs/ARCHITECTURE.md` (mark step 16 shipped)

- [ ] **Step 1: Update `packages/server/README.md`**

Add a new section after "Test":

````markdown
## LLM-based metadata extraction

Phase 2 step 16 wires real LLM-based metadata extraction via [rig](https://github.com/0xPlaygrounds/rig) + AWS Bedrock (Anthropic Claude Sonnet 4.7). The server requires AWS credentials at boot to construct the LLM client.

### Configuration

| Env var                              | Required | Default                                            |
| ------------------------------------ | -------- | -------------------------------------------------- |
| AWS credentials (any standard chain) | yes      | —                                                  |
| `MEETING_COMPANION_LLM_REGION`       | no       | `us-west-2`                                        |
| `MEETING_COMPANION_LLM_MODEL_ID`     | no       | `us.anthropic.claude-sonnet-4-7-20251015-v1:0`     |
| `MEETING_COMPANION_LLM_DISABLED`     | no       | unset (extraction enabled)                         |

The cross-region inference profile (model id starting with `us.`) must be enabled in the AWS Bedrock console — one-time setup per account.

`MEETING_COMPANION_LLM_DISABLED=1` skips extraction entirely. Default in the test suite. Useful for offline dev.

### Smoke

```bash
just llm-smoke "your meeting description"
```

### Integration test

```bash
just llm-integration
```

(Requires `RUN_LLM_INTEGRATION=1` + working AWS credentials + Sonnet 4.7 enabled.)

### Why rig

rig was chosen over a direct AWS SDK integration for: 20+ provider support (we ship Bedrock; switching to Anthropic-direct or OpenAI is a constructor change), agent abstractions for future Phase 2 step 18 work, retry/backoff embedded in rig's transport, and cortex-mem for the memory layer (Phase 2 step 18 wires this to mnemo).
````

- [ ] **Step 2: Add supersession callout to `docs/specs/server.md`**

In `docs/specs/server.md` §8.4 "Simulated LLM extraction", add at the top of the section:

```markdown
> **Phase 2 update:** This section describes the Phase 0 simulated stub.
> Phase 2 step 16 replaces it with real extraction via rig + Sonnet 4.7 —
> see [`docs/specs/phase-2-llm-extraction.md`](phase-2-llm-extraction.md).
> The wire contract (events, ordering, merge semantics) is unchanged.
```

- [ ] **Step 3: Update `docs/ARCHITECTURE.md` §0 Status block**

Change the Phase 2 line from "pending" to "partially shipped":

```markdown
- **Phase 2 (real audio + extraction pipeline) — partially shipped.** Step 16 (LLM metadata extraction via rig + Sonnet 4.7) is complete; see [`docs/specs/phase-2-llm-extraction.md`](specs/phase-2-llm-extraction.md). Remaining Phase 2 work: step 15 (real audio + STT/summarizer), step 17 (dynamic mode catalog), step 18 (memory-system enrichment via mnemo).
```

- [ ] **Step 4: Verify**

Run: `pnpm format` (formats the markdown).

Run: `cargo test -p meeting-companion-server -- --test-threads=1` — confirm 83 default tests pass.

- [ ] **Step 5: Commit**

```bash
git add packages/server/README.md docs/specs/server.md docs/ARCHITECTURE.md
git commit -m "docs: Phase 2 step 16 — rig LLM extraction documented"
```

---

## Self-review

| Spec section                                                | Implemented in                              |
|-------------------------------------------------------------|---------------------------------------------|
| §1 Purpose & scope                                          | Tasks 2-3                                   |
| §2.1 Function signature change (async + Result)             | Task 3                                      |
| §2.2 ExtractionError enum                                   | Task 2                                      |
| §2.3 Caller integration (spawn_extraction)                  | Task 3                                      |
| §2.4 Wire contract unchanged                                | Implicit — Task 3 doesn't change events      |
| §3.1 Module layout `llm.rs`                                 | Task 2                                      |
| §3.2 ExtractedMetadata struct                               | Task 2                                      |
| §3.3 LlmClient struct (rig Extractor wrapper)               | Task 2                                      |
| §3.4 from_env (region + model_id + provider)                | Task 2                                      |
| §3.5 extract (8s timeout + into_map filter)                 | Task 2                                      |
| §3.6 Retries handled by rig                                 | Implicit (rig framework)                    |
| §4.1 Env vars                                               | Task 2 (from_env reads them)                |
| §4.2 Server boot sequence (exit 3 on init failure)          | Task 3                                      |
| §4.3 Disable flag escape hatch                              | Task 3                                      |
| §5.1 Unit tests in llm.rs (5 tests)                         | Task 2                                      |
| §5.2 extraction.rs becomes thin wrapper                     | Task 3                                      |
| §5.3 Integration test (env-gated)                           | Task 4                                      |
| §5.4 Manual smoke (`just llm-smoke`)                        | Task 5                                      |
| §6 Errors & failure modes                                   | Tasks 2 (error type) + 3 (handler)          |
| §7 Cargo deps                                               | Task 1                                      |
| §8 Out of scope                                             | (acknowledged)                              |
| §9 Open questions                                           | None.                                       |

**Placeholder scan:** No `TODO`, `TBD`, `fill in details` strings remain in any task body.

**Type consistency:** `LlmClient`, `LlmInitError`, `ExtractionError`, `ExtractedMetadata`, `extract_metadata`, `merge_manual_wins`, `into_map` are defined exactly once and referenced by name everywhere else.

---

## Test count delta

- Phase 0 baseline: 79 server tests.
- **Removed (Task 3):**
  - `extract_takes_first_8_words` (extraction.rs unit) — −1
  - `extraction_merge_manual_wins` (tests/extraction.rs) — −1
  - `extraction_cancelled_on_stop` (tests/extraction.rs) — −1
- **Added (Tasks 2, 3, 4):**
  - `llm::tests::into_map_drops_empty_title_only` — +1
  - `llm::tests::into_map_drops_both_when_empty` — +1
  - `llm::tests::into_map_keeps_both_when_present` — +1
  - `llm::tests::system_prompt_mentions_extraction` — +1
  - `llm::tests::default_model_id_is_cross_region_profile` — +1
  - `extraction::tests::merge_manual_wins_with_empty_extracted` — +1
  - `extraction::tests::merge_manual_wins_with_empty_manual` — +1
  - `llm_integration::extracts_title_from_real_description` — +1 (env-gated; passes as no-op when `RUN_LLM_INTEGRATION` is unset)
- **Net: −3 + 8 = +5** → **84 default-running tests** at completion.

---

After Task 6, Phase 2 step 16 is complete via rig. The server uses real LLM-powered extraction, the wire contract with the PWA is unchanged, AWS credentials are required at boot (or `MEETING_COMPANION_LLM_DISABLED=1` for offline dev), and the abstraction stays at rig's `Extractor` level so swapping providers later is a constructor change. Steps 15, 17, 18 remain as separate spec/plan/execute cycles.
