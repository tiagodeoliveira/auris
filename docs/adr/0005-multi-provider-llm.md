# ADR-0005: Multi-provider LLM via `rig`

**Status:** Superseded in part (2026-05-24) by
[Split LLM pool spec](../superpowers/specs/2026-05-24-split-llm-pool-design.md).
The `rig` abstraction and provider set stand; the single-`LlmClient`
+ single-`AURIS_LLM_PROVIDER` shape is replaced by two pools
(chat + background) with their own provider/model env vars.
**Date:** 2026-05-02

## Context

The server uses an LLM for two jobs: extracting structured metadata from a
short meeting description (project, title, owner, …) and running per-mode
summarizers (highlights, actions, open questions) on a rolling transcript.
Both want JSON-shaped output with a typed schema, both run server-side, and
both must work from a single binary that the developer can ship without
hard-wiring credentials at compile time.

Three concrete pressures:

- **Provider availability varies by environment.** Bedrock works on AWS;
  OpenAI works anywhere with an API key; Anthropic-direct is the cheapest
  path during development for projects without an AWS account. The choice
  cannot be a build-time flag.
- **Schema-typed extraction is non-trivial to bolt onto raw HTTP clients.**
  Each provider has a different "structured output" mechanism (Bedrock
  tool calls, OpenAI JSON-mode, Anthropic system prompt + JSON parsing).
  Hand-rolling all three is doable but distracts from the meeting domain.
- **The summarizers and the extractor want the same backend.** They differ
  only in the system prompt and the output schema. A common abstraction
  saves duplicating provider-glue code per consumer.

## Decision

- **Use [`rig`](https://github.com/0xPlaygrounds/rig) as the LLM
  abstraction layer.** It exposes a unified `Extractor<Model, Schema>`
  type that handles structured output across all three providers we care
  about (Bedrock via `rig-bedrock`, OpenAI via `rig-core`, Anthropic via
  `rig-core`). Schemas are derived via `schemars::JsonSchema`; the
  provider's native structured-output mechanism is selected by `rig`.
- **Single `LlmClient` enum** in `llm.rs` wraps the three provider
  variants. Constructed once at server boot via `LlmClient::from_env()`,
  shared as `Arc<LlmClient>` across all consumers (extraction lambda,
  summarizers).
- **Provider chosen by `AURIS_LLM_PROVIDER` env var**:
  `bedrock` (default) | `openai` | `anthropic`. Model ID overridable via
  `AURIS_LLM_MODEL_ID`.
- **`extract_with_prompt::<Schema>(prompt, input)`** is the single
  consumer-facing API. Takes the system prompt as a runtime string
  (different for metadata vs. each summarizer), the user input, and a
  type parameter for the expected output schema.
- **Disable hatch:** `AURIS_LLM_DISABLED=1` makes
  `extract` return an empty result; useful for offline development and
  for skipping LLM calls in CI.

## Consequences

**Positive:**

- Adding a new summarizer is `let prompt = "..."; client.extract_with_prompt::<MySchema>(prompt, input).await` — no provider-aware code.
- Switching providers is one env var, no rebuild. Useful when an AWS
  region is throttled or an Anthropic key is rotating.
- The summarizers, the metadata extractor, and any future LLM-driven mode
  share one set of credentials and one initialization path.

**Negative:**

- `rig` is an evolving crate; we pin minor versions and accept that some
  upgrades will require small adjustments at our boundary.
- All three provider clients are linked into the binary even when only
  one is used, costing ~20 MB of binary size and ~5 s of build time.
  Acceptable for a personal project.
- Bedrock-specific knobs (region, model ID) are duplicated against
  the generic ones; a future contributor must read `from_env()` to know
  which env vars apply when.

**Accepted risks:**

- If `rig` ever drops support for a provider we depend on, we'd need to
  fork or migrate. We've kept our usage to the narrow `Extractor` API,
  which makes a hand-rolled replacement tractable.

## Alternatives considered

### (a, chosen) `rig` with three providers behind one enum

See above.

### (b) Hand-rolled HTTP client per provider

Three implementations of "POST chat/messages with structured output."
Rejected: structured output is the hardest part to get right per
provider; the cost of getting it wrong is malformed JSON in production.
Letting `rig` own that code path is a clear win.

### (c) Single-provider lock-in to Bedrock

Simpler dependency tree, one set of credentials. Rejected: locks
development-mode use to AWS. The Anthropic-direct path was specifically
added for cheap local iteration.

### (d) Build-time feature flag per provider

`cargo build --features=openai` etc. Rejected: switching providers
requires a rebuild. Live operations may want to swap based on quota
status, which a build-time flag forecloses.
