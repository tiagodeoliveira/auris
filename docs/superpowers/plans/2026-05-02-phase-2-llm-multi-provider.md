# Phase 2 Step 16 v3 — Multi-Provider LLM Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add OpenAI as a runtime-selectable second LLM provider alongside Bedrock. Default is unchanged (Bedrock + Sonnet 4.7); users opt into OpenAI via `MEETING_COMPANION_LLM_PROVIDER=openai` plus `OPENAI_API_KEY`. PWA wire contract is unchanged.

**Architecture:** Refactor `LlmClient` from a Bedrock-only struct to a thin wrapper around a `LlmExtractor` enum with one variant per provider. `from_env` parses `MEETING_COMPANION_LLM_PROVIDER` (defaults to `bedrock`), constructs the appropriate rig provider client, builds the typed Extractor, and stores it. `extract` matches on the enum and delegates. Adding a third provider in the future is "+1 enum variant + 1 from_env arm + 1 extract arm" (~10 lines).

**Tech Stack:** Same as v2, plus rig's OpenAI provider — either via `rig-core`'s `openai` feature flag, or via a hypothetical `rig-openai` companion crate. Verified at implementation time.

**Reference:** [`docs/specs/phase-2-llm-extraction.md`](../../specs/phase-2-llm-extraction.md) v3 is the spec this plan implements. Sections cited inline.

---

## Why this plan is short (4 tasks)

The v2 work already did the heavy lifting — `LlmClient`, `from_env`, `extract`, error type, prompt, schema, test infrastructure with the disable flag, smoke example, doc structure. v3 just changes the dispatch shape and adds OpenAI alongside.

---

## File structure produced by this plan

```
packages/server/
├── Cargo.toml                     [modified — enable rig openai feature OR add rig-openai dep]
├── README.md                      [modified — multi-provider config table]
├── examples/
│   └── llm_smoke.rs               [modified — print which provider answered]
├── src/
│   ├── llm.rs                     [modified — enum dispatch + parse_provider + 5 new tests]
│   └── (nothing else changes)
└── tests/
    └── llm_integration.rs         [modified — log resolved provider + brief comment about provider switching]

Justfile                           [modified — add llm-smoke-bedrock + llm-smoke-openai recipes]
```

The `extraction.rs`, `ws.rs`, `main.rs`, and `tests/common/mod.rs` files do NOT change in v3 — they all interact with `LlmClient` via the public API, which is signature-stable across v2 and v3.

---

## Task 1: Enable / add OpenAI provider in Cargo.toml

**Files:**

- Modify: `packages/server/Cargo.toml`

- [ ] **Step 1: Verify how rig exposes OpenAI**

```bash
cargo doc --no-deps -p rig-core --open
```

Or browse https://docs.rs/rig-core/latest/rig/providers/ — check whether `openai` is:

- (a) A submodule of `rig-core` always available (no extra dep / feature needed),
- (b) Behind a `rig-core` feature flag (e.g. `rig-core = { features = ["openai"] }`),
- (c) A separate companion crate (`rig-openai = "..."`).

The v2 build pulled in `rig-core 0.36.0` with `features = ["derive"]`. As of that version's docs, OpenAI is most likely option (a) or (b).

- [ ] **Step 2: Adjust `Cargo.toml` accordingly**

If (a): no change needed — OpenAI is already accessible via `rig::providers::openai`. Skip to Step 3.

If (b): add the feature, e.g.:

```toml
rig-core = { version = "0.36", features = ["derive", "openai"] }
```

If (c): add the new dep:

```toml
rig-openai = "<current>"
```

- [ ] **Step 3: Verify build**

```bash
cargo build -p meeting-companion-server
```

Expected: clean build. The OpenAI client transitively pulls `reqwest` etc. (already present).

- [ ] **Step 4: Verify existing tests still pass**

```bash
cargo test -p meeting-companion-server -- --test-threads=1
```

Expected: 84 tests pass (no behavior change yet).

- [ ] **Step 5: Verify clippy clean**

```bash
cargo clippy -p meeting-companion-server -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add packages/server/Cargo.toml Cargo.lock
git commit -m "chore(server): enable rig OpenAI provider"
```

If you adjusted feature flags or added a companion crate, document the actual choice in the commit body.

---

## Task 2: Refactor `LlmClient` to enum dispatch + provider-selecting `from_env` (squashed)

**Files:**

- Modify: `packages/server/src/llm.rs`

This is a single-commit refactor. It changes the internal shape of `LlmClient` from a single Extractor field to an enum-dispatching wrapper, and rewrites `from_env` to choose a provider based on env var. The public API (`from_env`, `extract`, `Clone`) is preserved.

- [ ] **Step 1: Read the current `llm.rs`**

Re-read `packages/server/src/llm.rs` to know the v2 shape — especially the rig import paths the v2 implementer settled on (the v2 task used `use rig_bedrock::client::Client as BedrockClient` and `use rig_bedrock::completion::CompletionModel`, NOT `rig::providers::bedrock`).

- [ ] **Step 2: Look up the OpenAI provider's actual API**

Confirm:

- Constructor: `rig::providers::openai::Client::from_env()` (sync, reads `OPENAI_API_KEY`) — verify by checking `rig_core`'s docs.
- Model type: `rig::providers::openai::completion::CompletionModel` or similar — confirm the path.
- Extractor builder: `client.extractor::<ExtractedMetadata>(model_id).preamble(...).build()` — same shape as Bedrock.

If the actual API differs from these guesses, adapt the implementation accordingly. Don't fight the framework.

- [ ] **Step 3: Rewrite `llm.rs`**

Replace the v2 module with the v3 version per spec §3.3-3.5. Key changes:

1. Add a `Provider` enum (`pub enum Provider { Bedrock, OpenAI }`).
2. Add an `LlmExtractor` enum with two variants (`Bedrock(Arc<...>)`, `OpenAI(Arc<...>)`) — `pub(crate)` visibility.
3. Add a `parse_provider(&str) -> Result<Provider, LlmInitError>` pure function.
4. Add a new error variant `LlmInitError::UnknownProvider(String)` and `LlmInitError::MissingProviderCredentials(String)`.
5. Add new constants: `DEFAULT_OPENAI_MODEL_ID = "gpt-4.1-mini"` (or whatever's current — verify against OpenAI's available models. If `gpt-4.1-mini` doesn't exist on the user's account, fall back to `gpt-4o-mini`).
6. Rename `DEFAULT_REGION` → `DEFAULT_BEDROCK_REGION` and `DEFAULT_MODEL_ID` → `DEFAULT_BEDROCK_MODEL_ID`. The `default_model_id_is_cross_region_profile` test name updates to `default_bedrock_model_id_is_cross_region_profile`.
7. Refactor `LlmClient` to hold `extractor: LlmExtractor` and `provider: Provider`. Add a `pub fn provider(&self) -> Provider` accessor.
8. Refactor `from_env` per spec §3.4 — read `MEETING_COMPANION_LLM_PROVIDER` (default `bedrock`), match on the parsed provider, construct the appropriate Extractor.
9. Refactor `extract` per spec §3.5 — match on `self.extractor`, dispatch to the right `e.extract(&prompt)`.
10. Add 5 new unit tests:
    - `parse_provider_accepts_bedrock`
    - `parse_provider_accepts_openai`
    - `parse_provider_is_case_insensitive`
    - `parse_provider_rejects_unknown`
    - `default_openai_model_id_is_set`
11. Rename the existing `default_model_id_is_cross_region_profile` test to `default_bedrock_model_id_is_cross_region_profile`.

The full module is roughly 200 lines after the refactor.

- [ ] **Step 4: Verify build**

```bash
cargo build -p meeting-companion-server
```

Expected: clean build. The `LlmClient` public API is unchanged, so `ws.rs` / `main.rs` / `tests/common/mod.rs` keep compiling.

- [ ] **Step 5: Run tests**

```bash
cargo test -p meeting-companion-server -- --test-threads=1
```

Expected:
- 5 v2 unit tests in `llm.rs` minus 1 (renamed) plus 5 new v3 tests = 9 unit tests in `llm.rs`. Wait — the rename keeps the test, so it's 5 (v2, including renamed) + 5 (v3 new) = 10 unit tests in `llm.rs`.
- Total: 84 (v2 baseline) − 0 (no v2 tests removed) + 5 (v3 new) = **89 tests**.

- [ ] **Step 6: Run clippy**

```bash
cargo clippy -p meeting-companion-server --tests -- -D warnings
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add packages/server/src/llm.rs
git commit -m "feat(server): multi-provider LLM dispatch (Bedrock + OpenAI)"
```

---

## Task 3: Update tests, smoke example, and Justfile recipes

**Files:**

- Modify: `packages/server/examples/llm_smoke.rs`
- Modify: `packages/server/tests/llm_integration.rs`
- Modify: `Justfile`

- [ ] **Step 1: Update `examples/llm_smoke.rs`**

Add a line printing the resolved provider before showing results:

```rust
println!("Initializing LLM client...");
let client = LlmClient::from_env().await?;
println!("Provider: {:?}", client.provider());
// ...rest unchanged...
```

This makes it obvious which provider answered when the user runs `just llm-smoke` after switching env vars.

- [ ] **Step 2: Update `tests/llm_integration.rs`**

Add a `tracing::info!` line logging the resolved provider after construction (it'll surface with `--nocapture`). Keep the rest of the test as-is — it still asserts a non-empty title and ≤8 words regardless of which provider answered.

```rust
let client = meeting_companion_server::llm::LlmClient::from_env()
    .await
    .expect("LLM client init");

tracing::info!(provider = ?client.provider(), "running integration test against provider");

let result = client.extract(/* ... */).await.expect(/* ... */);
```

(If `tracing_subscriber::fmt::init()` isn't being called in the test, use `eprintln!` instead so the line shows up.)

- [ ] **Step 3: Add `llm-smoke-bedrock` and `llm-smoke-openai` recipes to `Justfile`**

Append to the `# --- LLM ---` section:

```just
# Smoke-test against Bedrock specifically.
llm-smoke-bedrock description="Q1 budget review for helix product launch":
    MEETING_COMPANION_LLM_PROVIDER=bedrock cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Smoke-test against OpenAI specifically.
llm-smoke-openai description="Q1 budget review for helix product launch":
    MEETING_COMPANION_LLM_PROVIDER=openai cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"
```

The original `llm-smoke` recipe stays as-is (uses whatever the user's shell env says).

- [ ] **Step 4: Verify**

```bash
cargo build -p meeting-companion-server --examples
cargo test -p meeting-companion-server -- --test-threads=1   # still 89 tests
just --list                                                  # confirms the new recipes
```

Optionally run `just llm-smoke-bedrock` (requires AWS creds) and `just llm-smoke-openai` (requires `OPENAI_API_KEY`) to verify both paths hit live providers.

- [ ] **Step 5: Commit**

```bash
git add packages/server/examples/llm_smoke.rs packages/server/tests/llm_integration.rs Justfile
git commit -m "test(server): provider-aware smoke + integration test"
```

---

## Task 4: Doc updates

**Files:**

- Modify: `packages/server/README.md`
- Modify: `docs/ARCHITECTURE.md` §0 status block (note v3)

- [ ] **Step 1: Update `packages/server/README.md`**

Replace the existing "LLM-based metadata extraction" section's body (between the heading and the next section). Use the v3 env var table from spec §4.1 verbatim. Mention both providers + how to switch.

Before:

```markdown
## LLM-based metadata extraction

Phase 2 step 16 wires real LLM-based metadata extraction via [rig](https://github.com/0xPlaygrounds/rig) + AWS Bedrock (Anthropic Claude Sonnet 4.7). The server requires AWS credentials at boot to construct the LLM client.

### Configuration

| ... single-provider table ... |
```

After:

```markdown
## LLM-based metadata extraction

Phase 2 step 16 wires real LLM-based metadata extraction via [rig](https://github.com/0xPlaygrounds/rig). The server supports two providers as of v3: **AWS Bedrock** (default — Anthropic Claude Sonnet 4.7) and **OpenAI** (gpt-4.1-mini by default). Provider chosen at boot via env var.

### Configuration

| Env var                              | Required when                       | Default                                          |
| ------------------------------------ | ----------------------------------- | ------------------------------------------------ |
| `MEETING_COMPANION_LLM_PROVIDER`     | no                                  | `bedrock`                                        |
| `MEETING_COMPANION_LLM_MODEL_ID`     | no                                  | provider-specific                                |
| `MEETING_COMPANION_LLM_DISABLED`     | no                                  | unset                                            |
| **Bedrock-only**                     |                                     |                                                  |
| AWS credentials (any standard chain) | when `LLM_PROVIDER=bedrock`         | —                                                |
| `MEETING_COMPANION_LLM_REGION`       | no                                  | `us-west-2`                                      |
| **OpenAI-only**                      |                                     |                                                  |
| `OPENAI_API_KEY`                     | when `LLM_PROVIDER=openai`          | —                                                |

### Smoke

\`\`\`bash
just llm-smoke "your meeting description"          # uses currently-configured provider
just llm-smoke-bedrock "your description"          # forces bedrock
just llm-smoke-openai "your description"           # forces openai
\`\`\`

### Comparing providers

To compare extractions side by side, run the same description against both:

\`\`\`bash
just llm-smoke-bedrock "Q1 budget review for helix"
just llm-smoke-openai "Q1 budget review for helix"
\`\`\`
```

(Use literal triple-backtick fences when writing the file. The `\`\`\`bash` above is just for prompt rendering.)

Adjust the existing "Why rig" paragraph to remove "(we ship Bedrock; switching to ... is a constructor change)" — that's now inaccurate. Replace with: "Provider-pluggable via env var; v3 ships Bedrock + OpenAI; adding more rig-supported providers is a one-arm-on-the-enum change."

- [ ] **Step 2: Update `docs/ARCHITECTURE.md` §0 Status block**

Find:

```markdown
- **Phase 2 (real audio + extraction pipeline) — partially shipped.** Step 16 (LLM metadata extraction via rig + Sonnet 4.7) is complete; ...
```

Append " (v3 supports both Bedrock and OpenAI as runtime-selectable providers)":

```markdown
- **Phase 2 (real audio + extraction pipeline) — partially shipped.** Step 16 (LLM metadata extraction via rig + Sonnet 4.7) is complete and supports both AWS Bedrock and OpenAI as runtime-selectable providers; see [`docs/specs/phase-2-llm-extraction.md`](specs/phase-2-llm-extraction.md). Remaining Phase 2 work: step 15 (real audio + STT/summarizer), step 17 (dynamic mode catalog), step 18 (memory-system enrichment via mnemo).
```

- [ ] **Step 3: Verify**

Run `cargo test -p meeting-companion-server -- --test-threads=1` — confirm 89 tests still pass (no code changes; sanity).

- [ ] **Step 4: Commit**

```bash
git add packages/server/README.md docs/ARCHITECTURE.md
git commit -m "docs: Phase 2 step 16 v3 — multi-provider documented"
```

---

## Self-review

| Spec section                                                  | Implemented in                              |
| ------------------------------------------------------------- | ------------------------------------------- |
| §1 Purpose & scope (multi-provider behavior)                  | Tasks 2-4                                   |
| §2.1 Function signature unchanged                             | (no change in Tasks 2-4)                    |
| §2.2 ExtractionError unchanged; LlmInitError gains variants   | Task 2                                      |
| §2.3 Caller integration unchanged                             | (no change)                                 |
| §2.4 Wire contract unchanged                                  | (no change)                                 |
| §3.1 Module layout (LlmExtractor enum + Provider enum)        | Task 2                                      |
| §3.2 ExtractedMetadata unchanged                              | (kept from v2)                              |
| §3.3 LlmClient + LlmExtractor enum dispatch                   | Task 2                                      |
| §3.4 from_env with provider parsing + dispatch                | Task 2                                      |
| §3.5 extract with match arms per provider                     | Task 2                                      |
| §3.6 Retries handled by rig                                   | (kept from v2)                              |
| §4.1 Env vars (multi-provider table)                          | Task 4 (README) + Task 2 (constants)        |
| §4.2 Server boot sequence (3 LlmInitError variants)           | Task 2                                      |
| §4.3 Local dev / provider-comparison instructions             | Task 4                                      |
| §5.1 New unit tests (5 added)                                 | Task 2                                      |
| §5.2 extraction.rs unchanged                                  | (no change)                                 |
| §5.3 Integration test (provider-aware logging)                | Task 3                                      |
| §5.4 Manual smoke (provider-pinned recipes)                   | Task 3                                      |
| §6 Errors (init + extraction)                                 | Task 2                                      |
| §7 Cargo deps (OpenAI feature/crate)                          | Task 1                                      |
| §8 Out of scope (Anthropic-direct, etc.)                      | (acknowledged; not implemented)             |
| §9 Open questions                                             | None.                                       |

**Type consistency:** `Provider`, `LlmExtractor`, `LlmClient`, `LlmInitError`, `ExtractionError`, `ExtractedMetadata`, `parse_provider`, `extract`, `into_map` are defined exactly once and referenced by name everywhere else.

**Placeholder scan:** No `TODO`, `TBD`, `fill in details` strings.

---

## Test count delta

- v2 baseline: 84 tests.
- **Removed:** None.
- **Added (Task 2):**
  - `parse_provider_accepts_bedrock`
  - `parse_provider_accepts_openai`
  - `parse_provider_is_case_insensitive`
  - `parse_provider_rejects_unknown`
  - `default_openai_model_id_is_set`
- **Renamed (no count change):** `default_model_id_is_cross_region_profile` → `default_bedrock_model_id_is_cross_region_profile`.
- **Net: +5** → **89 tests** at completion.

---

After Task 4, the server supports two LLM providers (Bedrock, OpenAI) selectable via `MEETING_COMPANION_LLM_PROVIDER` env var. Adding more providers later is a small refactor pattern (1 enum variant + 1 from_env arm + 1 extract arm). The wire contract, test infra, and PWA all stay unchanged.
