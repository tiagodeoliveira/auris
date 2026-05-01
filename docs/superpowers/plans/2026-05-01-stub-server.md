# Stub Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Phase 0 stub server defined in `docs/specs/server.md` — a single Rust binary that owns meeting state, serves a WebSocket endpoint, broadcasts events to connected clients, and emits mock content. Sets up the monorepo so the PWA can drop in alongside.

**Architecture:** Bottom-up: monorepo skeleton → shared TS contract → Rust crate skeleton → contract types in Rust → pure state struct → pure state-machine → WS plumbing → background tasks (mock, extraction, heartbeat) → shutdown. Every commit leaves a green `cargo test` and `pnpm install`.

**Tech Stack:** Rust 2021 (tokio, tokio-tungstenite, serde, clap, tracing, uuid, subtle). Node 20+, pnpm 9+, TypeScript 5+.

---

## File structure produced by this plan

```
meeting_companion/
├── .gitignore
├── package.json                        # root, pnpm workspace anchor
├── pnpm-workspace.yaml
├── Cargo.toml                          # workspace; lists apps/server as member
├── Justfile
├── README.md                           # top-level
├── docs/                               # already exists
│   ├── meeting-companion-architecture.md
│   ├── specs/
│   │   └── server.md
│   └── superpowers/plans/
│       └── 2026-05-01-stub-server.md
├── apps/
│   └── server/
│       ├── Cargo.toml
│       ├── README.md
│       └── src/
│           ├── main.rs                 # CLI, boot, signal handler
│           ├── lib.rs                  # module exports + run_server
│           ├── contract.rs             # Intent, Event, shared types, PROTOCOL_VERSION
│           ├── state.rs                # ServerState struct, default modes, apply_intent
│           ├── mock.rs                 # mock content generator (pure)
│           ├── extraction.rs           # simulated LLM extraction (pure merge logic)
│           └── ws.rs                   # accept loop, auth, per-connection task
│       └── tests/
│           ├── common/mod.rs           # spawn_test_server, connect, helpers
│           ├── handshake.rs            # auth tests
│           ├── snapshot.rs             # snapshot on connect tests
│           ├── state_machine.rs        # intent → event integration tests
│           ├── mock_content.rs         # mock generator integration tests
│           ├── extraction.rs           # simulated extraction integration tests
│           ├── heartbeat.rs            # heartbeat integration tests
│           └── shutdown.rs             # graceful shutdown integration test
└── packages/
    └── contract/
        ├── package.json
        ├── tsconfig.json
        ├── README.md
        └── src/
            └── index.ts                # all §2.6 types + PROTOCOL_VERSION constant
```

---

## Task 1: Monorepo skeleton

**Files:**
- Create: `.gitignore`, `pnpm-workspace.yaml`, `package.json`, `Cargo.toml`, `Justfile`, `README.md`

- [ ] **Step 1: Create `.gitignore`**

```gitignore
# Rust
/target
**/target

# Node
node_modules
dist
.pnpm-store
*.tsbuildinfo

# Editor
.vscode
.idea
*.swp
.DS_Store

# Env
.env
.env.local
```

- [ ] **Step 2: Create `pnpm-workspace.yaml`**

```yaml
packages:
  - "apps/*"
  - "packages/*"
```

- [ ] **Step 3: Create root `package.json`**

```json
{
  "name": "meeting-companion",
  "version": "0.0.0",
  "private": true,
  "packageManager": "pnpm@9.0.0",
  "engines": {
    "node": ">=20"
  }
}
```

- [ ] **Step 4: Create root `Cargo.toml` (workspace)**

```toml
[workspace]
members = []
resolver = "2"

[workspace.package]
edition = "2021"
license = "Proprietary"
publish = false
```

(Members list is empty here; Task 3 adds `apps/server`.)

- [ ] **Step 5: Create `Justfile` (placeholder)**

```just
default:
    @just --list
```

- [ ] **Step 6: Create `README.md`**

```markdown
# Meeting Companion

Personal project. See `meeting-companion-architecture.md` for the system
spec and `docs/specs/` for component specs.

## Structure

- `apps/server/` — Rust WebSocket server (state owner)
- `apps/pwa/` — TypeScript PWA (forthcoming)
- `packages/contract/` — shared TS contract types

## Running

See `apps/server/README.md` once the server crate is in place.
```

- [ ] **Step 7: Verify**

Run: `pnpm install`
Expected: exits 0; "Done in <Xs>" with zero packages installed.

Run: `cargo check`
Expected: exits 0; output includes "warning: virtual workspace defaulting to `resolver = \"2\"`" or similar; no errors.

- [ ] **Step 8: Commit**

```bash
git add .gitignore pnpm-workspace.yaml package.json Cargo.toml Justfile README.md
git commit -m "chore: initialize monorepo skeleton"
```

---

## Task 2: Contract package (TypeScript)

**Files:**
- Create: `packages/contract/package.json`, `packages/contract/tsconfig.json`, `packages/contract/src/index.ts`, `packages/contract/README.md`

- [ ] **Step 1: Create `packages/contract/package.json`**

```json
{
  "name": "@meeting-companion/contract",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "typecheck": "tsc --noEmit"
  },
  "devDependencies": {
    "typescript": "^5.4.0"
  }
}
```

- [ ] **Step 2: Create `packages/contract/tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "declaration": true,
    "outDir": "./dist",
    "rootDir": "./src",
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true
  },
  "include": ["src/**/*"]
}
```

- [ ] **Step 3: Create `packages/contract/src/index.ts`**

```ts
export const PROTOCOL_VERSION = 1 as const;

export type MeetingState = "idle" | "active" | "paused";
export type UpdateStrategy = "replace" | "append";

export interface ModeOption {
  id: string;
  label: string;
  update_strategy: UpdateStrategy;
}

export interface Item {
  id: string;
  text: string;
  detail?: string;
  t: number;
  meta?: Record<string, unknown>;
}

export interface Status {
  listening: boolean;
  paused: boolean;
  error?: string;
}

export type Intent =
  | { type: "start_meeting"; description?: string; metadata?: Record<string, string> }
  | { type: "stop_meeting" }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_mode"; mode: string }
  | { type: "set_metadata"; key: string; value: string | null }
  | { type: "mark_moment"; t: number; note?: string }
  | { type: "expand_item"; item_id: string };

export type Event =
  | {
      type: "snapshot";
      protocol_version: number;
      meeting_state: MeetingState;
      available_modes: ModeOption[];
      mode: string;
      display_tag?: string;
      metadata: Record<string, string>;
      items: Item[];
      status: Status;
    }
  | { type: "meeting_state_changed"; meeting_state: MeetingState }
  | { type: "available_modes_changed"; available_modes: ModeOption[] }
  | { type: "mode_changed"; mode: string; display_tag?: string; items: Item[] }
  | { type: "display_tag_changed"; tag?: string }
  | { type: "metadata_changed"; metadata: Record<string, string> }
  | { type: "items_update"; items: Item[] }
  | { type: "status"; status: Status }
  | { type: "error"; code: string; message: string; intent_ref?: string };

export type ErrorCode =
  | "bad_json"
  | "unknown_intent"
  | "bad_payload"
  | "unknown_mode"
  | "unknown_item";
```

- [ ] **Step 4: Create `packages/contract/README.md`**

```markdown
# @meeting-companion/contract

Shared TypeScript types for the WebSocket contract between the server and
the PWA. Defines `Intent`, `Event`, `Item`, `ModeOption`, `Status`, and
`PROTOCOL_VERSION`. Mirrored in Rust at `apps/server/src/contract.rs`.

See `docs/specs/server.md` §2 for the full protocol.
```

- [ ] **Step 5: Install + build**

Run: `pnpm install`
Expected: installs `typescript` for the contract package.

Run: `pnpm -F @meeting-companion/contract build`
Expected: exits 0; emits `packages/contract/dist/index.js` and `dist/index.d.ts`.

- [ ] **Step 6: Commit**

```bash
git add packages/ pnpm-lock.yaml
git commit -m "feat(contract): add WS protocol types"
```

---

## Task 3: Server crate scaffold

**Files:**
- Create: `apps/server/Cargo.toml`, `apps/server/src/main.rs`, `apps/server/src/lib.rs`, `apps/server/README.md`
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Create `apps/server/Cargo.toml`**

```toml
[package]
name = "meeting-companion-server"
version = "0.0.0"
edition.workspace = true
license.workspace = true
publish = false

[lib]
path = "src/lib.rs"

[[bin]]
name = "meeting-companion-server"
path = "src/main.rs"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive", "env"] }
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
subtle = "2"
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"
tokio-util = { version = "0.7", features = ["rt"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
uuid = { version = "1", features = ["v4", "serde"] }

[dev-dependencies]
tokio = { version = "1", features = ["full", "test-util"] }
tokio-tungstenite = "0.24"
```

- [ ] **Step 2: Add server to root `Cargo.toml`**

Modify root `Cargo.toml`:
```toml
[workspace]
members = ["apps/server"]
resolver = "2"

[workspace.package]
edition = "2021"
license = "Proprietary"
publish = false
```

- [ ] **Step 3: Create `apps/server/src/lib.rs`**

```rust
//! Meeting Companion server library.
//!
//! See `docs/specs/server.md` for the component specification.
```

(Empty for now; later tasks add module declarations.)

- [ ] **Step 4: Create `apps/server/src/main.rs`**

```rust
use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "meeting-companion-server")]
#[command(about = "Meeting Companion stub WebSocket server")]
struct Args {
    /// TCP port to bind
    #[arg(long, default_value_t = 7331)]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let token = std::env::var("MEETING_COMPANION_TOKEN")
        .context("MEETING_COMPANION_TOKEN env var must be set and non-empty")?;
    if token.is_empty() {
        anyhow::bail!("MEETING_COMPANION_TOKEN must be non-empty");
    }

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    info!(?addr, version = env!("CARGO_PKG_VERSION"), "boot");

    info!("server scaffold complete; WS handling lands in Task 10");
    Ok(())
}
```

- [ ] **Step 5: Create `apps/server/README.md`**

```markdown
# meeting-companion-server

Phase 0 stub server. See `docs/specs/server.md` for the spec.

## Run

```bash
MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331
```

## Test

```bash
cargo test -p meeting-companion-server
```
```

- [ ] **Step 6: Verify**

Run: `cargo build -p meeting-companion-server`
Expected: succeeds.

Run: `MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --help`
Expected: prints clap-generated help text including `--port` and `--bind`.

Run: `cargo run -p meeting-companion-server` (no env var)
Expected: exits non-zero with error message about `MEETING_COMPANION_TOKEN`.

Run: `MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331`
Expected: prints "boot" log line and exits cleanly (no WS yet).

- [ ] **Step 7: Commit**

```bash
git add apps/server Cargo.toml Cargo.lock
git commit -m "chore(server): scaffold Rust crate"
```

---

## Task 4: Contract types in Rust

**Files:**
- Create: `apps/server/src/contract.rs`
- Modify: `apps/server/src/lib.rs`

- [ ] **Step 1: Add module declaration to `lib.rs`**

```rust
//! Meeting Companion server library.
//!
//! See `docs/specs/server.md` for the component specification.

pub mod contract;
```

- [ ] **Step 2: Create `apps/server/src/contract.rs` with types and tests**

```rust
//! WebSocket message contract. Mirrors `packages/contract/src/index.ts`.
//! See `docs/specs/server.md` §2.6.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingState {
    Idle,
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStrategy {
    Replace,
    Append,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOption {
    pub id: String,
    pub label: String,
    pub update_strategy: UpdateStrategy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
    pub t: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub listening: bool,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    StartMeeting {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        metadata: Option<HashMap<String, String>>,
    },
    StopMeeting,
    Pause,
    Resume,
    SetMode { mode: String },
    SetMetadata { key: String, value: Option<String> },
    MarkMoment {
        t: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
    },
    ExpandItem { item_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        protocol_version: u32,
        meeting_state: MeetingState,
        available_modes: Vec<ModeOption>,
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        display_tag: Option<String>,
        metadata: HashMap<String, String>,
        items: Vec<Item>,
        status: Status,
    },
    MeetingStateChanged { meeting_state: MeetingState },
    AvailableModesChanged { available_modes: Vec<ModeOption> },
    ModeChanged {
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        display_tag: Option<String>,
        items: Vec<Item>,
    },
    DisplayTagChanged {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tag: Option<String>,
    },
    MetadataChanged { metadata: HashMap<String, String> },
    ItemsUpdate { items: Vec<Item> },
    Status { status: Status },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        intent_ref: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn intent_start_meeting_full() {
        let i = Intent::StartMeeting {
            description: Some("Q1 review".into()),
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_start_meeting_minimal() {
        let i = Intent::StartMeeting { description: None, metadata: None };
        let json = serde_json::to_string(&i).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("metadata"));
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_stop_pause_resume() {
        for i in [Intent::StopMeeting, Intent::Pause, Intent::Resume] {
            assert_eq!(round_trip(&i), i);
        }
    }

    #[test]
    fn intent_set_mode() {
        let i = Intent::SetMode { mode: "highlights".into() };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_set_metadata_set_and_delete() {
        let set = Intent::SetMetadata { key: "project".into(), value: Some("helix".into()) };
        let del = Intent::SetMetadata { key: "project".into(), value: None };
        assert_eq!(round_trip(&set), set);
        assert_eq!(round_trip(&del), del);
        // value: null must round-trip as Some(None) → None — the field is present.
        let json = serde_json::to_string(&del).unwrap();
        assert!(json.contains("\"value\":null"));
    }

    #[test]
    fn intent_mark_moment() {
        let i = Intent::MarkMoment { t: 1234, note: Some("nice".into()) };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_expand_item() {
        let i = Intent::ExpandItem { item_id: "abc".into() };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn event_snapshot_round_trip() {
        let e = Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: MeetingState::Idle,
            available_modes: vec![ModeOption {
                id: "highlights".into(),
                label: "Highlights".into(),
                update_strategy: UpdateStrategy::Replace,
            }],
            mode: "highlights".into(),
            display_tag: None,
            metadata: HashMap::new(),
            items: vec![],
            status: Status { listening: false, paused: false, error: None },
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_meeting_state_changed() {
        let e = Event::MeetingStateChanged { meeting_state: MeetingState::Active };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_mode_changed_with_items() {
        let e = Event::ModeChanged {
            mode: "transcript".into(),
            display_tag: None,
            items: vec![Item {
                id: "i1".into(),
                text: "hello".into(),
                detail: None,
                t: 100,
                meta: None,
            }],
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_metadata_changed() {
        let e = Event::MetadataChanged {
            metadata: HashMap::from([("foo".into(), "bar".into())]),
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_items_update() {
        let e = Event::ItemsUpdate { items: vec![] };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_status() {
        let e = Event::Status {
            status: Status { listening: true, paused: false, error: None },
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_error_with_intent_ref() {
        let e = Event::Error {
            code: "unknown_mode".into(),
            message: "no such mode".into(),
            intent_ref: Some("bogus".into()),
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn intent_type_discriminator_snake_case() {
        let i = Intent::StartMeeting { description: None, metadata: None };
        let json = serde_json::to_string(&i).unwrap();
        assert!(json.contains("\"type\":\"start_meeting\""));
    }

    #[test]
    fn event_type_discriminator_snake_case() {
        let e = Event::MeetingStateChanged { meeting_state: MeetingState::Idle };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"meeting_state_changed\""));
        assert!(json.contains("\"meeting_state\":\"idle\""));
    }

    #[test]
    fn unknown_intent_type_fails_decode() {
        let json = r#"{"type":"fly_to_moon"}"#;
        let r: Result<Intent, _> = serde_json::from_str(json);
        assert!(r.is_err());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p meeting-companion-server contract::`
Expected: all 16 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/server/src/contract.rs apps/server/src/lib.rs
git commit -m "feat(server): contract types with serde round-trip tests"
```

---

## Task 5: ServerState struct + invariants + snapshot

**Files:**
- Create: `apps/server/src/state.rs`
- Modify: `apps/server/src/lib.rs`

- [ ] **Step 1: Add module to `lib.rs`**

```rust
pub mod contract;
pub mod state;
```

- [ ] **Step 2: Write failing tests in `state.rs`**

Create `apps/server/src/state.rs`:

```rust
//! ServerState — owns all meeting state. See `docs/specs/server.md` §3.

use crate::contract::{Event, Item, MeetingState, ModeOption, Status, UpdateStrategy, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::time::Instant;

pub fn default_modes() -> Vec<ModeOption> {
    vec![
        ModeOption {
            id: "highlights".into(),
            label: "Highlights".into(),
            update_strategy: UpdateStrategy::Replace,
        },
        ModeOption {
            id: "transcript".into(),
            label: "Transcript".into(),
            update_strategy: UpdateStrategy::Append,
        },
        ModeOption {
            id: "actions".into(),
            label: "Actions".into(),
            update_strategy: UpdateStrategy::Append,
        },
    ]
}

pub const DEFAULT_MODE_ID: &str = "highlights";

pub struct ServerState {
    pub(crate) meeting_state: MeetingState,
    pub(crate) available_modes: Vec<ModeOption>,
    pub(crate) current_mode: String,
    pub(crate) items_per_mode: HashMap<String, Vec<Item>>,
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) meeting_started_at: Option<Instant>,
}

impl ServerState {
    pub fn new() -> Self {
        let modes = default_modes();
        let items_per_mode: HashMap<String, Vec<Item>> =
            modes.iter().map(|m| (m.id.clone(), Vec::new())).collect();
        let s = Self {
            meeting_state: MeetingState::Idle,
            available_modes: modes,
            current_mode: DEFAULT_MODE_ID.to_string(),
            items_per_mode,
            metadata: HashMap::new(),
            meeting_started_at: None,
        };
        s.assert_invariants();
        s
    }

    pub fn snapshot(&self) -> Event {
        Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: self.meeting_state,
            available_modes: self.available_modes.clone(),
            mode: self.current_mode.clone(),
            display_tag: None,
            metadata: self.metadata.clone(),
            items: self
                .items_per_mode
                .get(&self.current_mode)
                .cloned()
                .unwrap_or_default(),
            status: Status {
                listening: matches!(self.meeting_state, MeetingState::Active),
                paused: matches!(self.meeting_state, MeetingState::Paused),
                error: None,
            },
        }
    }

    pub(crate) fn assert_invariants(&self) {
        debug_assert!(
            self.available_modes.iter().any(|m| m.id == self.current_mode),
            "current_mode not in available_modes"
        );
        debug_assert_eq!(
            self.items_per_mode.len(),
            self.available_modes.len(),
            "items_per_mode must have an entry per mode"
        );
        for m in &self.available_modes {
            debug_assert!(
                self.items_per_mode.contains_key(&m.id),
                "items_per_mode missing entry for mode {}",
                m.id
            );
        }
        match self.meeting_state {
            MeetingState::Idle => {
                debug_assert!(self.metadata.is_empty(), "metadata must be empty when idle");
                debug_assert!(
                    self.items_per_mode.values().all(|v| v.is_empty()),
                    "items must be empty when idle"
                );
                debug_assert!(self.meeting_started_at.is_none(), "meeting_started_at must be None when idle");
            }
            MeetingState::Active | MeetingState::Paused => {
                debug_assert!(self.meeting_started_at.is_some(), "meeting_started_at must be Some when not idle");
            }
        }
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_idle_state() {
        let s = ServerState::new();
        assert!(matches!(s.meeting_state, MeetingState::Idle));
    }

    #[test]
    fn new_has_three_default_modes() {
        let s = ServerState::new();
        assert_eq!(s.available_modes.len(), 3);
        assert_eq!(s.available_modes[0].id, "highlights");
        assert_eq!(s.available_modes[1].id, "transcript");
        assert_eq!(s.available_modes[2].id, "actions");
    }

    #[test]
    fn new_has_empty_items_per_mode() {
        let s = ServerState::new();
        for mode in &s.available_modes {
            assert_eq!(s.items_per_mode[&mode.id].len(), 0);
        }
    }

    #[test]
    fn new_default_current_mode_is_highlights() {
        let s = ServerState::new();
        assert_eq!(s.current_mode, "highlights");
    }

    #[test]
    fn snapshot_initial_state() {
        let s = ServerState::new();
        match s.snapshot() {
            Event::Snapshot {
                protocol_version,
                meeting_state,
                available_modes,
                mode,
                display_tag,
                metadata,
                items,
                status,
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(matches!(meeting_state, MeetingState::Idle));
                assert_eq!(available_modes.len(), 3);
                assert_eq!(mode, "highlights");
                assert!(display_tag.is_none());
                assert!(metadata.is_empty());
                assert!(items.is_empty());
                assert!(!status.listening);
                assert!(!status.paused);
                assert!(status.error.is_none());
            }
            e => panic!("expected snapshot, got {:?}", e),
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p meeting-companion-server state::`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/server/src/state.rs apps/server/src/lib.rs
git commit -m "feat(server): ServerState struct, invariants, snapshot"
```

---

## Task 6: State machine — start / stop / pause / resume

**Files:**
- Modify: `apps/server/src/state.rs`

This task implements the lifecycle rows of §4.1 of the spec.

- [ ] **Step 1: Define `IntentOutcome` and add `apply_intent` skeleton**

Add to `state.rs`:

```rust
use crate::contract::Intent;

/// Result of applying an intent. `events` are broadcast in order.
/// `error` is sent only to the originating client (None unless protocol error).
#[derive(Debug, Default)]
pub struct IntentOutcome {
    pub events: Vec<Event>,
    pub error: Option<Event>,
    pub start_extraction_for: Option<String>,   // populated on start_meeting w/ description
    pub started_meeting: bool,                  // signal to spawn mock generator
    pub stopped_meeting: bool,                  // signal to cancel mock generator
    pub paused_meeting: bool,                   // signal to cancel mock generator
    pub resumed_meeting: bool,                  // signal to respawn mock generator
}

impl ServerState {
    pub fn apply_intent(&mut self, intent: Intent) -> IntentOutcome {
        let mut outcome = IntentOutcome::default();
        match intent {
            Intent::StartMeeting { description, metadata } => {
                self.handle_start_meeting(description, metadata, &mut outcome);
            }
            Intent::StopMeeting => {
                self.handle_stop_meeting(&mut outcome);
            }
            Intent::Pause => {
                self.handle_pause(&mut outcome);
            }
            Intent::Resume => {
                self.handle_resume(&mut outcome);
            }
            // Other intents land in Tasks 7 and 8.
            _ => {
                tracing::warn!("intent not yet implemented");
            }
        }
        self.assert_invariants();
        outcome
    }

    fn handle_start_meeting(
        &mut self,
        description: Option<String>,
        metadata: Option<HashMap<String, String>>,
        outcome: &mut IntentOutcome,
    ) {
        if !matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!(state = ?self.meeting_state, "start_meeting in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        self.meeting_started_at = Some(Instant::now());
        self.metadata = metadata.unwrap_or_default();
        self.current_mode = DEFAULT_MODE_ID.to_string();

        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Active });
        outcome.events.push(Event::MetadataChanged { metadata: self.metadata.clone() });
        outcome.events.push(Event::ModeChanged {
            mode: self.current_mode.clone(),
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
        outcome.started_meeting = true;
        if let Some(d) = description.filter(|s| !s.is_empty()) {
            outcome.start_extraction_for = Some(d);
        }
    }

    fn handle_stop_meeting(&mut self, outcome: &mut IntentOutcome) {
        if matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!("stop_meeting in idle state");
            return;
        }
        self.meeting_state = MeetingState::Idle;
        self.metadata.clear();
        for v in self.items_per_mode.values_mut() {
            v.clear();
        }
        self.meeting_started_at = None;
        self.current_mode = DEFAULT_MODE_ID.to_string();

        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Idle });
        outcome.stopped_meeting = true;
    }

    fn handle_pause(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "pause in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Paused;
        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Paused });
        outcome.paused_meeting = true;
    }

    fn handle_resume(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Paused) {
            tracing::warn!(state = ?self.meeting_state, "resume in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Active });
        outcome.resumed_meeting = true;
    }
}
```

- [ ] **Step 2: Add lifecycle tests**

Append to `state.rs` `mod tests`:

```rust
    #[test]
    fn start_meeting_from_idle() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
        });
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        assert_eq!(out.events.len(), 3);
        assert!(matches!(out.events[0], Event::MeetingStateChanged { meeting_state: MeetingState::Active }));
        assert!(matches!(out.events[1], Event::MetadataChanged { .. }));
        assert!(matches!(out.events[2], Event::ModeChanged { .. }));
        assert!(out.started_meeting);
        assert!(out.start_extraction_for.is_none());
    }

    #[test]
    fn start_meeting_with_description_signals_extraction() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: Some("Q1 budget review".into()),
            metadata: None,
        });
        assert_eq!(out.start_extraction_for.as_deref(), Some("Q1 budget review"));
    }

    #[test]
    fn start_meeting_when_active_is_noop() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out = s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(matches!(s.meeting_state, MeetingState::Active));
    }

    #[test]
    fn stop_meeting_from_active() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: Some(HashMap::from([("k".into(), "v".into())])) });
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(matches!(s.meeting_state, MeetingState::Idle));
        assert!(s.metadata.is_empty());
        assert!(s.items_per_mode.values().all(|v| v.is_empty()));
        assert_eq!(s.current_mode, "highlights");
        assert_eq!(out.events.len(), 1);
        assert!(out.stopped_meeting);
    }

    #[test]
    fn stop_meeting_when_idle_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(out.events.is_empty());
        assert!(!out.stopped_meeting);
    }

    #[test]
    fn pause_from_active() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out = s.apply_intent(Intent::Pause);
        assert!(matches!(s.meeting_state, MeetingState::Paused));
        assert_eq!(out.events.len(), 1);
        assert!(out.paused_meeting);
    }

    #[test]
    fn pause_when_idle_or_paused_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::Pause);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        s.apply_intent(Intent::Pause);
        let out2 = s.apply_intent(Intent::Pause);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn resume_from_paused() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        s.apply_intent(Intent::Pause);
        let out = s.apply_intent(Intent::Resume);
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert!(out.resumed_meeting);
    }

    #[test]
    fn resume_when_idle_or_active_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::Resume);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out2 = s.apply_intent(Intent::Resume);
        assert!(out2.events.is_empty());
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p meeting-companion-server state::`
Expected: previous 5 tests + 9 new lifecycle tests = 14 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/server/src/state.rs
git commit -m "feat(server): state machine — start/stop/pause/resume"
```

---

## Task 7: State machine — set_mode + set_metadata

**Files:**
- Modify: `apps/server/src/state.rs`

- [ ] **Step 1: Extend `apply_intent` match arms**

In `state.rs`, replace the placeholder `_ => { tracing::warn!(...) }` arm with handlers for `SetMode` and `SetMetadata`. The match becomes:

```rust
match intent {
    Intent::StartMeeting { description, metadata } => {
        self.handle_start_meeting(description, metadata, &mut outcome);
    }
    Intent::StopMeeting => self.handle_stop_meeting(&mut outcome),
    Intent::Pause => self.handle_pause(&mut outcome),
    Intent::Resume => self.handle_resume(&mut outcome),
    Intent::SetMode { mode } => self.handle_set_mode(mode, &mut outcome),
    Intent::SetMetadata { key, value } => self.handle_set_metadata(key, value, &mut outcome),
    _ => { tracing::warn!("intent not yet implemented"); }
}
```

- [ ] **Step 2: Add handler implementations**

```rust
impl ServerState {
    fn handle_set_mode(&mut self, mode: String, outcome: &mut IntentOutcome) {
        if !self.available_modes.iter().any(|m| m.id == mode) {
            outcome.error = Some(Event::Error {
                code: "unknown_mode".into(),
                message: format!("mode '{}' not in catalog", mode),
                intent_ref: Some(mode),
            });
            return;
        }
        self.current_mode = mode.clone();
        outcome.events.push(Event::ModeChanged {
            mode,
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
    }

    fn handle_set_metadata(&mut self, key: String, value: Option<String>, outcome: &mut IntentOutcome) {
        match value {
            Some(v) => { self.metadata.insert(key, v); }
            None => { self.metadata.remove(&key); }
        }
        outcome.events.push(Event::MetadataChanged { metadata: self.metadata.clone() });
    }
}
```

- [ ] **Step 3: Add tests**

```rust
    #[test]
    fn set_mode_valid() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode { mode: "transcript".into() });
        assert_eq!(s.current_mode, "transcript");
        assert!(out.error.is_none());
        match &out.events[..] {
            [Event::ModeChanged { mode, items, display_tag }] => {
                assert_eq!(mode, "transcript");
                assert!(items.is_empty());
                assert!(display_tag.is_none());
            }
            other => panic!("unexpected events: {:?}", other),
        }
    }

    #[test]
    fn set_mode_unknown_emits_error() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode { mode: "bogus".into() });
        assert_eq!(s.current_mode, "highlights");
        assert!(out.events.is_empty());
        match out.error {
            Some(Event::Error { code, intent_ref, .. }) => {
                assert_eq!(code, "unknown_mode");
                assert_eq!(intent_ref.as_deref(), Some("bogus"));
            }
            _ => panic!("expected unknown_mode error"),
        }
    }

    #[test]
    fn set_mode_in_idle_is_allowed() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode { mode: "actions".into() });
        assert_eq!(s.current_mode, "actions");
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn set_metadata_insert() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMetadata {
            key: "project".into(),
            value: Some("helix".into()),
        });
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        match &out.events[..] {
            [Event::MetadataChanged { metadata }] => {
                assert_eq!(metadata.len(), 1);
                assert_eq!(metadata["project"], "helix");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn set_metadata_delete() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::SetMetadata { key: "k".into(), value: Some("v".into()) });
        let out = s.apply_intent(Intent::SetMetadata { key: "k".into(), value: None });
        assert!(s.metadata.is_empty());
        match &out.events[..] {
            [Event::MetadataChanged { metadata }] => assert!(metadata.is_empty()),
            _ => panic!(),
        }
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p meeting-companion-server state::`
Expected: previous 14 + 5 new = 19 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/server/src/state.rs
git commit -m "feat(server): state machine — set_mode + set_metadata"
```

---

## Task 8: State machine — mark_moment + expand_item + detail synthesis

**Files:**
- Modify: `apps/server/src/state.rs`

- [ ] **Step 1: Add detail synthesis helper**

```rust
fn synthesize_detail(text: &str) -> String {
    format!(
        "Detail for '{}': lorem ipsum dolor sit amet, consectetur adipiscing elit. \
         Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
         Ut enim ad minim veniam.",
        text
    )
}
```

- [ ] **Step 2: Extend `apply_intent`**

```rust
match intent {
    // ...existing arms...
    Intent::MarkMoment { t, note } => self.handle_mark_moment(t, note, &mut outcome),
    Intent::ExpandItem { item_id } => self.handle_expand_item(item_id, &mut outcome),
}
```

(Remove the wildcard `_ => ...` arm now that all intents are handled.)

- [ ] **Step 3: Add handlers**

```rust
impl ServerState {
    fn handle_mark_moment(&mut self, t: u64, note: Option<String>, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "mark_moment in invalid state");
            return;
        }
        tracing::info!(t, ?note, "mark_moment");
        outcome.events.push(Event::Status {
            status: Status {
                listening: true,
                paused: false,
                error: None,
            },
        });
    }

    fn handle_expand_item(&mut self, item_id: String, outcome: &mut IntentOutcome) {
        let mode_id = self.current_mode.clone();
        let strategy = self
            .available_modes
            .iter()
            .find(|m| m.id == mode_id)
            .map(|m| m.update_strategy)
            .expect("invariant: current_mode in available_modes");

        let items = self.items_per_mode.get_mut(&mode_id).expect("invariant: items_per_mode entry exists");
        let Some(idx) = items.iter().position(|i| i.id == item_id) else {
            outcome.error = Some(Event::Error {
                code: "unknown_item".into(),
                message: format!("item '{}' not found in current mode", item_id),
                intent_ref: Some(item_id),
            });
            return;
        };

        let detail = synthesize_detail(&items[idx].text);
        items[idx].detail = Some(detail);

        let payload = match strategy {
            UpdateStrategy::Replace => items.clone(),
            UpdateStrategy::Append => vec![items[idx].clone()],
        };
        outcome.events.push(Event::ItemsUpdate { items: payload });
    }
}
```

- [ ] **Step 4: Add helper for tests to inject items**

In the `tests` module (above the test fns):

```rust
    fn push_item(s: &mut ServerState, mode: &str, id: &str, text: &str) {
        s.items_per_mode.get_mut(mode).unwrap().push(Item {
            id: id.into(),
            text: text.into(),
            detail: None,
            t: 0,
            meta: None,
        });
    }
```

- [ ] **Step 5: Add tests**

```rust
    #[test]
    fn mark_moment_active_emits_status() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out = s.apply_intent(Intent::MarkMoment { t: 1234, note: None });
        match &out.events[..] {
            [Event::Status { status }] => {
                assert!(status.listening);
                assert!(!status.paused);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn mark_moment_idle_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::MarkMoment { t: 0, note: None });
        assert!(out.events.is_empty());
    }

    #[test]
    fn expand_item_append_strategy_returns_single_item() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        s.apply_intent(Intent::SetMode { mode: "transcript".into() });
        push_item(&mut s, "transcript", "i1", "first");
        push_item(&mut s, "transcript", "i2", "second");

        let out = s.apply_intent(Intent::ExpandItem { item_id: "i2".into() });
        match &out.events[..] {
            [Event::ItemsUpdate { items }] => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id, "i2");
                assert!(items[0].detail.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_replace_strategy_returns_full_list() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        push_item(&mut s, "highlights", "h1", "first");
        push_item(&mut s, "highlights", "h2", "second");

        let out = s.apply_intent(Intent::ExpandItem { item_id: "h1".into() });
        match &out.events[..] {
            [Event::ItemsUpdate { items }] => {
                assert_eq!(items.len(), 2);
                assert!(items[0].detail.is_some());
                assert!(items[1].detail.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_unknown_emits_error() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out = s.apply_intent(Intent::ExpandItem { item_id: "nope".into() });
        assert!(out.events.is_empty());
        match out.error {
            Some(Event::Error { code, intent_ref, .. }) => {
                assert_eq!(code, "unknown_item");
                assert_eq!(intent_ref.as_deref(), Some("nope"));
            }
            _ => panic!(),
        }
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p meeting-companion-server state::`
Expected: previous 19 + 5 new = 24 tests pass.

- [ ] **Step 7: Commit**

```bash
git add apps/server/src/state.rs
git commit -m "feat(server): state machine — mark_moment + expand_item + detail"
```

---

## Task 9: Mock content generator (pure logic)

**Files:**
- Create: `apps/server/src/mock.rs`
- Modify: `apps/server/src/lib.rs`, `apps/server/src/state.rs`

- [ ] **Step 1: Add module to `lib.rs`**

```rust
pub mod contract;
pub mod mock;
pub mod state;
```

- [ ] **Step 2: Add helper on `ServerState`**

In `state.rs`, add:

```rust
impl ServerState {
    /// Append a new item to the current mode's list and return the broadcast payload
    /// shaped per the current mode's update strategy.
    /// Caps the replace-strategy list at 10 (FIFO).
    pub fn push_mock_item(&mut self, item: Item) -> Vec<Item> {
        let mode_id = self.current_mode.clone();
        let strategy = self
            .available_modes
            .iter()
            .find(|m| m.id == mode_id)
            .map(|m| m.update_strategy)
            .expect("invariant");
        let items = self.items_per_mode.get_mut(&mode_id).expect("invariant");
        items.push(item.clone());

        let payload = match strategy {
            UpdateStrategy::Replace => {
                while items.len() > 10 {
                    items.remove(0);
                }
                items.clone()
            }
            UpdateStrategy::Append => vec![item],
        };
        self.assert_invariants();
        payload
    }

    pub fn current_mode_id(&self) -> &str {
        &self.current_mode
    }

    pub fn meeting_started_at(&self) -> Option<Instant> {
        self.meeting_started_at
    }
}
```

- [ ] **Step 3: Create `apps/server/src/mock.rs`**

```rust
//! Mock content generator. Produces fake items for Phase 0 so the PWA
//! has something to render. See `docs/specs/server.md` §8.6.

use crate::contract::Item;
use std::time::Instant;
use uuid::Uuid;

pub const HIGHLIGHTS: &[&str] = &[
    "Tiago raised concern about Q1 budget overrun",
    "Decision: ship feature X by end of sprint",
    "Open question: who owns the migration",
    "Action item: schedule follow-up with vendor",
    "Aline highlighted the dependency on the auth team",
    "Push the launch date by two weeks",
    "Concern: test coverage gap in the new module",
    "Confirmed: customer is OK with the proposed timeline",
];

pub const TRANSCRIPT: &[&str] = &[
    "Speaker A: I think we should delay the launch by two weeks.",
    "Speaker B: Acknowledged. Let me check with engineering.",
    "Speaker A: The dependency on the auth team is the blocker.",
    "Speaker C: I can take the auth conversation offline.",
    "Speaker A: Great. What about the migration plan?",
    "Speaker B: Draft is ready, sending tonight.",
    "Speaker C: Are we testing against staging first?",
    "Speaker A: Yes, full staging soak before prod.",
    "Speaker B: Agreed. We'll set up the soak window.",
    "Speaker A: Anything else? OK, ending here.",
];

pub const ACTIONS: &[&str] = &[
    "Tiago: Draft proposal by Friday",
    "Aline: Confirm vendor availability",
    "Speaker C: Sync with auth team on dependency",
    "Speaker B: Send migration draft tonight",
    "Speaker A: Schedule staging soak window",
    "Tiago: Update launch date in roadmap",
];

pub fn template_for(mode_id: &str) -> &'static [&'static str] {
    match mode_id {
        "highlights" => HIGHLIGHTS,
        "transcript" => TRANSCRIPT,
        "actions" => ACTIONS,
        _ => HIGHLIGHTS,
    }
}

pub fn make_item(mode_id: &str, tick_index: usize, started_at: Instant) -> Item {
    let templates = template_for(mode_id);
    let text = templates[tick_index % templates.len()].to_string();
    let t_ms = started_at.elapsed().as_millis() as u64;
    Item {
        id: Uuid::new_v4().to_string(),
        text,
        detail: None,
        t: t_ms,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_item_uses_correct_template() {
        let started = Instant::now();
        let i0 = make_item("highlights", 0, started);
        assert_eq!(i0.text, HIGHLIGHTS[0]);
        let i1 = make_item("transcript", 1, started);
        assert_eq!(i1.text, TRANSCRIPT[1]);
    }

    #[test]
    fn make_item_wraps_around() {
        let started = Instant::now();
        let i = make_item("highlights", HIGHLIGHTS.len(), started);
        assert_eq!(i.text, HIGHLIGHTS[0]);
    }

    #[test]
    fn make_item_unique_ids() {
        let started = Instant::now();
        let a = make_item("actions", 0, started);
        let b = make_item("actions", 0, started);
        assert_ne!(a.id, b.id);
    }
}
```

- [ ] **Step 4: Add tests for `push_mock_item` cap behavior**

In `state.rs` `mod tests`:

```rust
    #[test]
    fn push_mock_item_replace_caps_at_10() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        // current_mode = highlights = replace strategy
        for i in 0..15 {
            let item = Item {
                id: format!("h{}", i),
                text: format!("item {}", i),
                detail: None,
                t: i as u64,
                meta: None,
            };
            let payload = s.push_mock_item(item);
            assert!(payload.len() <= 10);
        }
        let final_items = &s.items_per_mode["highlights"];
        assert_eq!(final_items.len(), 10);
        assert_eq!(final_items[0].id, "h5");   // FIFO drop kept items 5..15
        assert_eq!(final_items[9].id, "h14");
    }

    #[test]
    fn push_mock_item_append_returns_single_item() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        s.apply_intent(Intent::SetMode { mode: "transcript".into() });
        let item = Item {
            id: "t1".into(),
            text: "hi".into(),
            detail: None,
            t: 0,
            meta: None,
        };
        let payload = s.push_mock_item(item.clone());
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0].id, "t1");
        // But items_per_mode keeps growing
        for i in 0..5 {
            s.push_mock_item(Item {
                id: format!("t{}", i + 2),
                text: format!("hi{}", i + 2),
                detail: None,
                t: i as u64,
                meta: None,
            });
        }
        assert_eq!(s.items_per_mode["transcript"].len(), 6);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p meeting-companion-server`
Expected: 24 prior + 3 mock + 2 push_mock_item = 29 tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/server/src/mock.rs apps/server/src/state.rs apps/server/src/lib.rs
git commit -m "feat(server): mock content generator (pure)"
```

---

## Task 10: WS server — accept loop + auth + run_server entry

**Files:**
- Create: `apps/server/src/ws.rs`, `apps/server/tests/common/mod.rs`, `apps/server/tests/handshake.rs`
- Modify: `apps/server/src/lib.rs`, `apps/server/src/main.rs`

- [ ] **Step 1: Add module to `lib.rs` and re-export `run_server`**

```rust
pub mod contract;
pub mod mock;
pub mod state;
pub mod ws;

pub use ws::run_server;
```

- [ ] **Step 2: Create `apps/server/src/ws.rs` (skeleton with auth only)**

```rust
//! WebSocket server. See `docs/specs/server.md` §2.1, §6.3, §7.

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::contract::Event;
use crate::state::ServerState;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
}

pub async fn run_server(addr: SocketAddr, token: String, shutdown_rx: oneshot::Receiver<()>) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = ?actual, "listening");
    run_server_with_listener(listener, token, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let (events_tx, _) = broadcast::channel::<Event>(64);
    let handle = ServerHandle {
        state: Arc::new(Mutex::new(ServerState::new())),
        events_tx,
        token: Arc::new(token),
    };

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let h = handle.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, peer, h).await {
                                warn!(?peer, error = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => warn!(error = %e, "accept error"),
                }
            }
            _ = &mut shutdown_rx => {
                info!("shutdown received");
                break;
            }
        }
    }
    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handle: ServerHandle,
) -> Result<()> {
    let token_cell = Arc::new(std::sync::Mutex::new(None::<String>));
    let cell_clone = Arc::clone(&token_cell);

    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, response: Response| {
        let raw_path = req.uri().to_string();
        let token = parse_token_from_uri(&raw_path);
        *cell_clone.lock().unwrap() = token;
        Ok(response)
    })
    .await?;

    let provided = token_cell.lock().unwrap().clone();
    let valid = match provided.as_deref() {
        Some(t) => constant_time_eq(t.as_bytes(), handle.token.as_bytes()),
        None => false,
    };

    if !valid {
        warn!(?peer, reason = if provided.is_some() { "mismatch" } else { "missing" }, "auth failure");
        let mut ws = ws;
        let _ = ws
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "invalid token".into(),
            })))
            .await;
        return Ok(());
    }

    info!(?peer, "connection accepted");
    // Per-connection loop (snapshot + dispatch + broadcast forward) lands in Task 11.
    // For now: send a placeholder snapshot and exit.
    let snapshot = {
        let s = handle.state.lock().await;
        s.snapshot()
    };
    let mut ws = ws;
    ws.send(Message::Text(serde_json::to_string(&snapshot)?)).await?;
    ws.close(None).await.ok();
    info!(?peer, "connection closed");
    Ok(())
}

fn parse_token_from_uri(raw: &str) -> Option<String> {
    let uri: Uri = raw.parse().ok()?;
    let q = uri.query()?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next()?;
        if k == "token" {
            return Some(v.to_string());
        }
    }
    None
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}
```

- [ ] **Step 3: Wire main.rs to call `run_server`**

Replace `apps/server/src/main.rs` body:

```rust
use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use tokio::sync::oneshot;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "meeting-companion-server")]
struct Args {
    #[arg(long, default_value_t = 7331)]
    port: u16,
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let token = std::env::var("MEETING_COMPANION_TOKEN")
        .context("MEETING_COMPANION_TOKEN env var must be set")?;
    if token.is_empty() {
        anyhow::bail!("MEETING_COMPANION_TOKEN must be non-empty");
    }
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    info!(?addr, version = env!("CARGO_PKG_VERSION"), "boot");

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(());
    });

    meeting_companion_server::run_server(addr, token, shutdown_rx).await
}
```

- [ ] **Step 4: Create `apps/server/tests/common/mod.rs`**

```rust
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;

pub struct TestServer {
    pub addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn spawn_test_server() -> TestServer {
    spawn_test_server_with_token("test-token").await
}

pub async fn spawn_test_server_with_token(token: &str) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();
    let token = token.to_string();
    tokio::spawn(async move {
        let _ = meeting_companion_server::ws::run_server_with_listener(listener, token, rx).await;
    });
    TestServer { addr, shutdown: Some(tx) }
}

pub fn ws_url(addr: SocketAddr, token: &str) -> Request {
    let url = format!("ws://{}/?token={}", addr, token);
    url.into_client_request().expect("client request")
}
```

- [ ] **Step 5: Create `apps/server/tests/handshake.rs`**

```rust
mod common;

use common::*;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error;

#[tokio::test]
async fn handshake_token_match() {
    let server = spawn_test_server().await;
    let (mut ws, _) = connect_async(ws_url(server.addr, "test-token")).await.expect("connect");
    let msg = tokio::time::timeout(std::time::Duration::from_secs(1), futures_util::StreamExt::next(&mut ws))
        .await
        .expect("timeout")
        .expect("frame")
        .expect("msg");
    assert!(msg.is_text(), "expected text frame, got {:?}", msg);
    let json: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(json["type"], "snapshot");
}

#[tokio::test]
async fn handshake_token_mismatch() {
    let server = spawn_test_server().await;
    let res = connect_async(ws_url(server.addr, "wrong-token")).await;
    let mut ws = match res {
        Ok((ws, _)) => ws,
        Err(_) => return, // some clients see the close as a connect error; that's also OK.
    };
    // Read frames until we get a close.
    use futures_util::StreamExt;
    loop {
        match ws.next().await {
            Some(Ok(msg)) if msg.is_close() => return,
            Some(Ok(_)) => continue,
            Some(Err(Error::ConnectionClosed)) | None => return,
            Some(Err(e)) => panic!("unexpected error: {}", e),
        }
    }
}

#[tokio::test]
async fn handshake_token_missing() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let server = spawn_test_server().await;
    let url = format!("ws://{}/", server.addr);
    let req = url.into_client_request().unwrap();
    let res = connect_async(req).await;
    let mut ws = match res {
        Ok((ws, _)) => ws,
        Err(_) => return,
    };
    use futures_util::StreamExt;
    loop {
        match ws.next().await {
            Some(Ok(msg)) if msg.is_close() => return,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return,
        }
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p meeting-companion-server --test handshake`
Expected: 3 tests pass.

Run: `cargo test -p meeting-companion-server`
Expected: 29 unit tests + 3 integration tests = 32 tests pass.

- [ ] **Step 7: Commit**

```bash
git add apps/server/src/ws.rs apps/server/src/main.rs apps/server/src/lib.rs apps/server/tests/
git commit -m "feat(server): WS accept + auth + connection scaffold"
```

---

## Task 11: Per-connection task — snapshot + intent dispatch + broadcast

**Files:**
- Modify: `apps/server/src/ws.rs`
- Create: `apps/server/tests/snapshot.rs`, `apps/server/tests/state_machine.rs`

This is the integration seam. After this task, the wire contract from §2 is fully exercised end-to-end (mock generator + extraction + heartbeat still come in later tasks).

- [ ] **Step 1: Replace `handle_connection` body with full per-connection loop**

In `ws.rs`, replace the placeholder body:

```rust
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handle: ServerHandle,
) -> Result<()> {
    // ... auth block stays the same ...

    info!(?peer, "connection accepted");

    let mut events_rx = handle.events_tx.subscribe();
    let snapshot = {
        let s = handle.state.lock().await;
        s.snapshot()
    };

    let (mut sink, mut stream) = ws.split();
    sink.send(Message::Text(serde_json::to_string(&snapshot)?)).await?;

    loop {
        tokio::select! {
            evt = events_rx.recv() => {
                match evt {
                    Ok(event) => {
                        let json = serde_json::to_string(&event)?;
                        if sink.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(?peer, lagged = n, "client lagging — disconnecting");
                        let _ = sink.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Error,
                            reason: "client lagging".into(),
                        }))).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        dispatch_intent(&t, &handle, &mut sink, &peer).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}    // ignore binary, ping, pong
                    Some(Err(e)) => {
                        warn!(?peer, error = %e, "ws read error");
                        break;
                    }
                }
            }
        }
    }

    info!(?peer, "connection closed");
    Ok(())
}
```

- [ ] **Step 2: Add `dispatch_intent` (state errors only; protocol errors land in Task 12)**

```rust
use crate::contract::Intent;

async fn dispatch_intent(
    text: &str,
    handle: &ServerHandle,
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
    peer: &SocketAddr,
) -> Result<()> {
    let intent: Intent = match serde_json::from_str(text) {
        Ok(i) => i,
        Err(_) => {
            // Protocol-error handling lands in Task 12.
            warn!(?peer, "bad inbound JSON; will be handled in Task 12");
            return Ok(());
        }
    };

    let outcome = {
        let mut s = handle.state.lock().await;
        s.apply_intent(intent)
    };

    if let Some(err_event) = outcome.error {
        let json = serde_json::to_string(&err_event)?;
        sink.send(Message::Text(json)).await.ok();
    }
    for event in outcome.events {
        let _ = handle.events_tx.send(event);
    }
    // Background task signals (started_meeting/stopped_meeting/etc.) handled in later tasks.
    Ok(())
}
```

- [ ] **Step 3: Update test helper to provide event capture**

Modify `apps/server/tests/common/mod.rs`. Add helpers:

```rust
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let req = ws_url(addr, token);
    let (ws, _) = tokio_tungstenite::connect_async(req).await.expect("connect");
    ws
}

pub async fn next_event(ws: &mut Ws, timeout: Duration) -> Value {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .expect("timeout waiting for event")
        .expect("stream ended")
        .expect("ws error");
    let text = msg.to_text().expect("text frame").to_string();
    serde_json::from_str(&text).expect("json")
}

pub async fn send_intent(ws: &mut Ws, intent: Value) {
    ws.send(Message::Text(intent.to_string())).await.expect("send");
}
```

- [ ] **Step 4: Create `apps/server/tests/snapshot.rs`**

```rust
mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn snapshot_initial_state() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    let snap = next_event(&mut ws, Duration::from_secs(1)).await;
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["protocol_version"], 1);
    assert_eq!(snap["meeting_state"], "idle");
    assert_eq!(snap["available_modes"].as_array().unwrap().len(), 3);
    assert_eq!(snap["mode"], "highlights");
    assert!(snap["metadata"].as_object().unwrap().is_empty());
    assert!(snap["items"].as_array().unwrap().is_empty());
    assert_eq!(snap["status"]["listening"], false);
    assert_eq!(snap["status"]["paused"], false);
}

#[tokio::test]
async fn reconnect_snapshot_active() {
    let server = spawn_test_server().await;
    let mut ws1 = connect(server.addr, "test-token").await;
    let _ = next_event(&mut ws1, Duration::from_secs(1)).await; // snapshot
    send_intent(&mut ws1, json!({"type":"start_meeting"})).await;
    // Drain 3 events from the start-meeting sequence.
    for _ in 0..3 { let _ = next_event(&mut ws1, Duration::from_secs(1)).await; }
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut ws2 = connect(server.addr, "test-token").await;
    let snap = next_event(&mut ws2, Duration::from_secs(1)).await;
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["meeting_state"], "active");
}
```

- [ ] **Step 5: Create `apps/server/tests/state_machine.rs`**

```rust
mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(2);

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, T).await;
}

#[tokio::test]
async fn start_stop_meeting_events() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;

    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    let e1 = next_event(&mut ws, T).await;
    let e2 = next_event(&mut ws, T).await;
    let e3 = next_event(&mut ws, T).await;
    assert_eq!(e1["type"], "meeting_state_changed");
    assert_eq!(e1["meeting_state"], "active");
    assert_eq!(e2["type"], "metadata_changed");
    assert_eq!(e3["type"], "mode_changed");
    assert_eq!(e3["mode"], "highlights");

    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let e4 = next_event(&mut ws, T).await;
    assert_eq!(e4["type"], "meeting_state_changed");
    assert_eq!(e4["meeting_state"], "idle");
}

#[tokio::test]
async fn start_meeting_with_metadata() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;

    send_intent(&mut ws, json!({"type":"start_meeting", "metadata": {"project": "helix"}})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed
    let meta = next_event(&mut ws, T).await;
    assert_eq!(meta["type"], "metadata_changed");
    assert_eq!(meta["metadata"]["project"], "helix");
}

#[tokio::test]
async fn pause_resume_events() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let p = next_event(&mut ws, T).await;
    assert_eq!(p["meeting_state"], "paused");
    send_intent(&mut ws, json!({"type":"resume"})).await;
    let r = next_event(&mut ws, T).await;
    assert_eq!(r["meeting_state"], "active");
}

#[tokio::test]
async fn set_mode_valid() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["type"], "mode_changed");
    assert_eq!(m["mode"], "transcript");
}

#[tokio::test]
async fn set_mode_unknown_returns_error_to_originator_only() {
    let server = spawn_test_server().await;
    let mut a = connect(server.addr, "test-token").await;
    let mut b = connect(server.addr, "test-token").await;
    drain_snapshot(&mut a).await;
    drain_snapshot(&mut b).await;

    send_intent(&mut a, json!({"type":"set_mode", "mode": "bogus"})).await;
    let err = next_event(&mut a, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "unknown_mode");
    assert_eq!(err["intent_ref"], "bogus");

    // B should see nothing within 500ms.
    let res = tokio::time::timeout(Duration::from_millis(500), futures_util::StreamExt::next(&mut b)).await;
    assert!(res.is_err(), "B should not have received an event");
}

#[tokio::test]
async fn set_mode_in_idle_allowed() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["type"], "mode_changed");
    assert_eq!(m["mode"], "transcript");
}

#[tokio::test]
async fn set_metadata_basic() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_metadata", "key": "foo", "value": "bar"})).await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["metadata"]["foo"], "bar");
}

#[tokio::test]
async fn set_metadata_delete() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_metadata", "key": "foo", "value": "bar"})).await;
    let _ = next_event(&mut ws, T).await;
    send_intent(&mut ws, json!({"type":"set_metadata", "key": "foo", "value": null})).await;
    let m = next_event(&mut ws, T).await;
    assert!(m["metadata"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn two_clients_broadcast() {
    let server = spawn_test_server().await;
    let mut a = connect(server.addr, "test-token").await;
    let mut b = connect(server.addr, "test-token").await;
    drain_snapshot(&mut a).await;
    drain_snapshot(&mut b).await;
    send_intent(&mut a, json!({"type":"start_meeting"})).await;
    let bn = next_event(&mut b, T).await;
    assert_eq!(bn["type"], "meeting_state_changed");
    assert_eq!(bn["meeting_state"], "active");
}

#[tokio::test]
async fn expand_item_unknown() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"expand_item", "item_id": "nope"})).await;
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "unknown_item");
}

#[tokio::test]
async fn mark_moment_active() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"mark_moment", "t": 1234})).await;
    let s = next_event(&mut ws, T).await;
    assert_eq!(s["type"], "status");
    assert_eq!(s["status"]["listening"], true);
}

#[tokio::test]
async fn mark_moment_idle_no_event() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"mark_moment", "t": 0})).await;
    let res = tokio::time::timeout(Duration::from_millis(500), futures_util::StreamExt::next(&mut ws)).await;
    assert!(res.is_err());
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p meeting-companion-server`
Expected: all unit + integration tests pass (29 unit + 3 handshake + 2 snapshot + 12 state_machine = 46).

- [ ] **Step 7: Commit**

```bash
git add apps/server/src/ws.rs apps/server/tests/
git commit -m "feat(server): per-connection task — snapshot + intent dispatch"
```

---

## Task 12: Protocol error events (bad JSON, unknown intent, bad payload)

**Files:**
- Modify: `apps/server/src/ws.rs`
- Modify: `apps/server/tests/state_machine.rs` (add tests)

- [ ] **Step 1: Replace placeholder error handling in `dispatch_intent`**

In `ws.rs`, replace the `dispatch_intent` body's bad-JSON section:

```rust
async fn dispatch_intent(
    text: &str,
    handle: &ServerHandle,
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
    peer: &SocketAddr,
) -> Result<()> {
    // 1. Parse as raw JSON object first to distinguish bad_json vs unknown_intent vs bad_payload.
    let raw: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) if v.is_object() => v,
        _ => {
            send_protocol_error(sink, "bad_json", "frame is not a valid JSON object", None).await?;
            return Ok(());
        }
    };

    let ty = raw.get("type").and_then(|v| v.as_str());
    let known_intents = [
        "start_meeting", "stop_meeting", "pause", "resume",
        "set_mode", "set_metadata", "mark_moment", "expand_item",
    ];
    let Some(ty) = ty else {
        send_protocol_error(sink, "unknown_intent", "missing 'type' field", None).await?;
        return Ok(());
    };
    if !known_intents.contains(&ty) {
        send_protocol_error(sink, "unknown_intent", &format!("unknown intent type '{}'", ty), Some(ty)).await?;
        return Ok(());
    }

    // 2. Parse as Intent strictly. Failure here = bad_payload.
    let intent: Intent = match serde_json::from_value(raw) {
        Ok(i) => i,
        Err(e) => {
            send_protocol_error(sink, "bad_payload", &format!("{}", e), Some(ty)).await?;
            return Ok(());
        }
    };

    let outcome = {
        let mut s = handle.state.lock().await;
        s.apply_intent(intent)
    };

    if let Some(err_event) = outcome.error {
        let json = serde_json::to_string(&err_event)?;
        sink.send(Message::Text(json)).await.ok();
    }
    for event in outcome.events {
        let _ = handle.events_tx.send(event);
    }
    let _ = peer;   // currently unused beyond the auth phase
    Ok(())
}

async fn send_protocol_error(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
    code: &str,
    message: &str,
    intent_ref: Option<&str>,
) -> Result<()> {
    let evt = Event::Error {
        code: code.into(),
        message: message.into(),
        intent_ref: intent_ref.map(|s| s.into()),
    };
    let json = serde_json::to_string(&evt)?;
    sink.send(Message::Text(json)).await.ok();
    Ok(())
}
```

- [ ] **Step 2: Add tests to `state_machine.rs`**

```rust
#[tokio::test]
async fn bad_json() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    use tokio_tungstenite::tungstenite::Message;
    use futures_util::SinkExt;
    ws.send(Message::Text("not json at all".into())).await.unwrap();
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "bad_json");
}

#[tokio::test]
async fn unknown_intent() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"fly_to_moon"})).await;
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["code"], "unknown_intent");
}

#[tokio::test]
async fn bad_payload() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_mode"})).await;   // missing 'mode'
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["code"], "bad_payload");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p meeting-companion-server`
Expected: 46 prior + 3 new = 49 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/server/src/ws.rs apps/server/tests/state_machine.rs
git commit -m "feat(server): protocol error events"
```

---

## Task 13: Mock generator background task

**Files:**
- Modify: `apps/server/src/ws.rs`, `apps/server/src/state.rs` (minor)
- Create: `apps/server/tests/mock_content.rs`

- [ ] **Step 1: Add `tokio_util::sync::CancellationToken` infrastructure to `ServerHandle`**

In `ws.rs`:

```rust
use tokio_util::sync::CancellationToken;
use std::sync::Mutex as StdMutex;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
}
```

Update `run_server_with_listener` to initialize `meeting_cancel: Arc::new(StdMutex::new(None))`.

- [ ] **Step 2: Add `spawn_mock_generator` function**

In `ws.rs`:

```rust
use crate::contract::Event;
use crate::mock::make_item;
use std::time::Duration;

const MOCK_INTERVAL: Duration = Duration::from_secs(3);

pub fn spawn_mock_generator(handle: ServerHandle, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MOCK_INTERVAL);
        interval.tick().await;   // discard the immediate tick
        let mut idx: usize = 0;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let event = {
                        let mut s = handle.state.lock().await;
                        let started_at = match s.meeting_started_at() {
                            Some(t) => t,
                            None => break,
                        };
                        let mode_id = s.current_mode_id().to_string();
                        let item = make_item(&mode_id, idx, started_at);
                        let payload = s.push_mock_item(item);
                        Event::ItemsUpdate { items: payload }
                    };
                    let _ = handle.events_tx.send(event);
                    idx += 1;
                }
                _ = cancel.cancelled() => break,
            }
        }
    });
}
```

- [ ] **Step 3: Wire `dispatch_intent` to react to lifecycle outcome flags**

After applying the intent and broadcasting events:

```rust
// After: for event in outcome.events { ... }

if outcome.started_meeting || outcome.resumed_meeting {
    let token = CancellationToken::new();
    {
        let mut slot = handle.meeting_cancel.lock().unwrap();
        if let Some(prev) = slot.take() {
            prev.cancel();
        }
        *slot = Some(token.clone());
    }
    spawn_mock_generator(handle.clone(), token);
}
if outcome.stopped_meeting || outcome.paused_meeting {
    let prev = handle.meeting_cancel.lock().unwrap().take();
    if let Some(t) = prev {
        t.cancel();
    }
}
```

- [ ] **Step 4: Create `apps/server/tests/mock_content.rs`**

```rust
mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(5);

async fn drain_snapshot(ws: &mut Ws) { let _ = next_event(ws, T).await; }

async fn drain_n(ws: &mut Ws, n: usize) {
    for _ in 0..n { let _ = next_event(ws, T).await; }
}

#[tokio::test]
async fn mock_items_replace() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
    let items = evt["items"].as_array().unwrap();
    assert!(!items.is_empty());
}

#[tokio::test]
async fn mock_items_append() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let _ = next_event(&mut ws, T).await; // mode_changed
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
    let items = evt["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn mock_stops_on_pause() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _first = next_event(&mut ws, Duration::from_secs(5)).await; // wait for at least one items_update
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let _ = next_event(&mut ws, T).await;   // meeting_state_changed
    // Now wait 5s and confirm no items_update arrives.
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "items_update" {
                return evt;
            }
        }
    }).await;
    assert!(res.is_err(), "items_update should not arrive while paused");
}

#[tokio::test]
async fn mock_stops_on_stop() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _ = next_event(&mut ws, Duration::from_secs(5)).await;
    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed { idle }
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "items_update" {
                return evt;
            }
        }
    }).await;
    assert!(res.is_err(), "items_update should not arrive after stop");
}

#[tokio::test]
async fn mock_resumes_on_resume() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _ = next_event(&mut ws, Duration::from_secs(5)).await;
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let _ = next_event(&mut ws, T).await;
    send_intent(&mut ws, json!({"type":"resume"})).await;
    let _ = next_event(&mut ws, T).await;
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p meeting-companion-server --test mock_content`
Expected: 5 tests pass (each takes 5-10s — test runtime ~30s).

Run: `cargo test -p meeting-companion-server`
Expected: 49 prior + 5 new = 54 tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/server/src/ws.rs apps/server/tests/mock_content.rs
git commit -m "feat(server): mock generator background task"
```

---

## Task 14: Simulated extraction task

**Files:**
- Create: `apps/server/src/extraction.rs`
- Modify: `apps/server/src/lib.rs`, `apps/server/src/ws.rs`
- Create: `apps/server/tests/extraction.rs`

- [ ] **Step 1: Add module to `lib.rs`**

```rust
pub mod contract;
pub mod extraction;
pub mod mock;
pub mod state;
pub mod ws;
```

- [ ] **Step 2: Create `apps/server/src/extraction.rs`**

```rust
//! Simulated LLM metadata extraction. See `docs/specs/server.md` §8.4.

use std::collections::HashMap;

pub fn extract_metadata(description: &str) -> HashMap<String, String> {
    let title = description.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
    HashMap::from([
        ("title".to_string(), title),
        ("project".to_string(), "sim-extracted".to_string()),
    ])
}

/// Manual values win on conflict (architecture-stated rule).
pub fn merge_manual_wins(extracted: HashMap<String, String>, manual: &HashMap<String, String>) -> HashMap<String, String> {
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
    fn extract_takes_first_8_words() {
        let d = "Q1 budget review for the helix product launch and beyond";
        let m = extract_metadata(d);
        assert_eq!(m["title"], "Q1 budget review for the helix product launch");
        assert_eq!(m["project"], "sim-extracted");
    }

    #[test]
    fn merge_manual_wins_on_conflict() {
        let extracted = HashMap::from([
            ("project".into(), "sim-extracted".into()),
            ("title".into(), "auto title".into()),
        ]);
        let manual = HashMap::from([("project".into(), "helix".into())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged["project"], "helix");
        assert_eq!(merged["title"], "auto title");
    }
}
```

- [ ] **Step 3: Add extraction-task spawning in `ws.rs`**

```rust
use crate::extraction;

const EXTRACTION_DELAY: Duration = Duration::from_millis(1500);

fn spawn_extraction(handle: ServerHandle, description: String, cancel: CancellationToken) {
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(EXTRACTION_DELAY) => {
                let extracted = extraction::extract_metadata(&description);
                let event = {
                    let mut s = handle.state.lock().await;
                    if !matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Active | crate::contract::MeetingState::Paused) {
                        return;
                    }
                    let manual = s.metadata_clone();
                    let merged = extraction::merge_manual_wins(extracted, &manual);
                    s.set_metadata_full(merged.clone());
                    Event::MetadataChanged { metadata: merged }
                };
                let _ = handle.events_tx.send(event);
            }
            _ = cancel.cancelled() => {}
        }
    });
}
```

- [ ] **Step 4: Add helper methods to `ServerState`**

In `state.rs`:

```rust
impl ServerState {
    pub fn snapshot_meeting_state(&self) -> MeetingState {
        self.meeting_state
    }

    pub fn metadata_clone(&self) -> HashMap<String, String> {
        self.metadata.clone()
    }

    pub fn set_metadata_full(&mut self, metadata: HashMap<String, String>) {
        if matches!(self.meeting_state, MeetingState::Idle) {
            // Don't apply extraction results to idle state — meeting was stopped mid-extraction.
            return;
        }
        self.metadata = metadata;
    }
}
```

- [ ] **Step 5: Wire `dispatch_intent` to spawn extraction**

In `ws.rs`, add after the mock-generator spawn block:

```rust
if let Some(description) = outcome.start_extraction_for {
    let token = handle
        .meeting_cancel
        .lock()
        .unwrap()
        .as_ref()
        .map(|t| t.child_token());
    if let Some(t) = token {
        spawn_extraction(handle.clone(), description, t);
    }
}
```

- [ ] **Step 6: Create `apps/server/tests/extraction.rs`**

```rust
mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(3);

async fn drain_snapshot(ws: &mut Ws) { let _ = next_event(ws, T).await; }

#[tokio::test]
async fn extraction_merge_manual_wins() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({
        "type":"start_meeting",
        "description":"Q1 budget review",
        "metadata": {"project": "helix"}
    })).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed
    let m1 = next_event(&mut ws, T).await; // first metadata_changed (manual only)
    assert_eq!(m1["type"], "metadata_changed");
    assert_eq!(m1["metadata"]["project"], "helix");
    assert!(m1["metadata"].get("title").is_none());
    let _ = next_event(&mut ws, T).await; // mode_changed

    // Wait for extraction (1.5s + slop).
    let m2 = next_event(&mut ws, Duration::from_secs(3)).await;
    assert_eq!(m2["type"], "metadata_changed");
    assert_eq!(m2["metadata"]["project"], "helix"); // manual wins
    assert_eq!(m2["metadata"]["title"], "Q1 budget review");
}

#[tokio::test]
async fn extraction_no_description() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    // After 2.5s, no extraction event should arrive.
    let res = tokio::time::timeout(Duration::from_millis(2500), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "metadata_changed" { return evt; }
        }
    }).await;
    assert!(res.is_err(), "extraction should not run without description");
}

#[tokio::test]
async fn extraction_cancelled_on_stop() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({
        "type":"start_meeting",
        "description":"some description here"
    })).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed { idle }
    // After 2.5s, no late metadata_changed should arrive.
    let res = tokio::time::timeout(Duration::from_millis(2500), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "metadata_changed" { return evt; }
        }
    }).await;
    assert!(res.is_err());
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p meeting-companion-server`
Expected: 54 prior + 2 unit + 3 integration = 59 tests pass.

- [ ] **Step 8: Commit**

```bash
git add apps/server/src/extraction.rs apps/server/src/state.rs apps/server/src/ws.rs apps/server/src/lib.rs apps/server/tests/extraction.rs
git commit -m "feat(server): simulated LLM extraction task"
```

---

## Task 15: Heartbeat task

**Files:**
- Modify: `apps/server/src/ws.rs`
- Create: `apps/server/tests/heartbeat.rs`

- [ ] **Step 1: Spawn heartbeat task in `run_server_with_listener`**

In `ws.rs`, in `run_server_with_listener` (after constructing `handle`, before the accept loop):

```rust
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

let hb_handle = handle.clone();
let hb_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
let hb_shutdown_clone = hb_shutdown.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    interval.tick().await; // skip first immediate tick
    loop {
        interval.tick().await;
        if hb_shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) { break; }
        let status = {
            let s = hb_handle.state.lock().await;
            crate::contract::Status {
                listening: matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Active),
                paused: matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Paused),
                error: None,
            }
        };
        let _ = hb_handle.events_tx.send(Event::Status { status });
    }
});
```

After the accept loop completes (`break`), set `hb_shutdown.store(true, ...)`.

- [ ] **Step 2: Create `apps/server/tests/heartbeat.rs`**

For tests, the 10s default would make the test slow. Allow override via env var for tests:

In `ws.rs`:

```rust
fn heartbeat_interval() -> Duration {
    if let Ok(s) = std::env::var("MEETING_COMPANION_HEARTBEAT_MS") {
        if let Ok(ms) = s.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(10)
}
```

Use `heartbeat_interval()` instead of the const. (This is a test-only seam; the spec's behavior is unchanged at default.)

Add to `apps/server/tests/common/mod.rs`:

```rust
pub async fn spawn_test_server_fast_heartbeat() -> TestServer {
    std::env::set_var("MEETING_COMPANION_HEARTBEAT_MS", "300");
    let s = spawn_test_server().await;
    s
}
```

Create `apps/server/tests/heartbeat.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(2);

async fn drain_snapshot(ws: &mut Ws) { let _ = next_event(ws, T).await; }

async fn next_status(ws: &mut Ws) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let evt = next_event(ws, Duration::from_millis(500)).await;
        if evt["type"] == "status" {
            return evt;
        }
        if std::time::Instant::now() > deadline {
            panic!("no status event within deadline");
        }
    }
}

#[tokio::test]
async fn heartbeat_idle() {
    let server = spawn_test_server_fast_heartbeat().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    let s = next_status(&mut ws).await;
    assert_eq!(s["status"]["listening"], false);
    assert_eq!(s["status"]["paused"], false);
}

#[tokio::test]
async fn heartbeat_active() {
    let server = spawn_test_server_fast_heartbeat().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    let s = next_status(&mut ws).await;
    assert_eq!(s["status"]["listening"], true);
    assert_eq!(s["status"]["paused"], false);
}
```

Note: tests that set env vars are not perfectly isolated. If flakiness arises, gate by setting the env var inside `run_server_with_listener`'s configuration parameter instead (refactor to take a `Config` struct).

- [ ] **Step 3: Run tests**

Run: `cargo test -p meeting-companion-server --test heartbeat -- --test-threads=1`
Expected: 2 tests pass.

Run: `cargo test -p meeting-companion-server`
Expected: 59 prior + 2 new = 61 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/server/src/ws.rs apps/server/tests/
git commit -m "feat(server): heartbeat task"
```

---

## Task 16: Graceful shutdown + signal handler

**Files:**
- Modify: `apps/server/src/main.rs`, `apps/server/src/ws.rs`
- Create: `apps/server/tests/shutdown.rs`

- [ ] **Step 1: Update `run_server_with_listener` to broadcast shutdown to all subscribers**

In `ws.rs`, before returning from `run_server_with_listener`, ensure the `events_tx` is dropped so subscribers receive `RecvError::Closed`. Then send a close frame to each tracked connection.

To track connections, change `ServerHandle` to carry an `Arc<StdMutex<Vec<Sender<()>>>>` of per-connection close signals. Each connection registers on accept and removes on disconnect. On shutdown, fire all close signals.

Concretely:

```rust
#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    pub shutdown: CancellationToken,
}
```

The `CancellationToken` replaces the per-connection signals — each connection observes it and exits.

In `handle_connection`'s `select!`:

```rust
loop {
    tokio::select! {
        _ = handle.shutdown.cancelled() => {
            let _ = sink.send(Message::Close(Some(CloseFrame {
                code: CloseCode::Away,
                reason: "going away".into(),
            }))).await;
            break;
        }
        evt = events_rx.recv() => { /* ...as before... */ }
        msg = stream.next() => { /* ...as before... */ }
    }
}
```

In `run_server_with_listener`:

```rust
pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let shutdown = CancellationToken::new();
    // ... build handle including shutdown.clone() ...

    // accept loop ... when shutdown_rx triggers, call shutdown.cancel() and break.

    loop {
        tokio::select! {
            accept = listener.accept() => { /* spawn */ }
            _ = &mut shutdown_rx => {
                shutdown.cancel();
                break;
            }
        }
    }

    // give connections 2s to drain.
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}
```

- [ ] **Step 2: Wire SIGTERM in `main.rs`**

```rust
let (shutdown_tx, shutdown_rx) = oneshot::channel();
tokio::spawn(async move {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }
    let _ = shutdown_tx.send(());
});
```

- [ ] **Step 3: Create `apps/server/tests/shutdown.rs`**

```rust
mod common;

use common::*;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn graceful_shutdown_sends_close() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    let _ = next_event(&mut ws, Duration::from_secs(1)).await;

    drop(server); // triggers shutdown_tx via TestServer::Drop

    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got_close = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(msg))) if matches!(msg, Message::Close(_)) => { got_close = true; break; }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(got_close, "expected close frame on graceful shutdown");
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p meeting-companion-server`
Expected: 61 prior + 1 new = 62 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/server/src/ws.rs apps/server/src/main.rs apps/server/tests/shutdown.rs
git commit -m "feat(server): graceful shutdown"
```

---

## Task 17: Justfile smoke + README polish

**Files:**
- Modify: `Justfile`, `apps/server/README.md`, `README.md` (top level)

- [ ] **Step 1: Update `Justfile`**

```just
default:
    @just --list

# Run the server with a development token
server-run:
    MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331

# Run all tests
test:
    cargo test -p meeting-companion-server
    pnpm -F @meeting-companion/contract typecheck

# Manual smoke: start server + websocat in two terminals
smoke-instructions:
    @echo "Terminal 1: just server-run"
    @echo "Terminal 2:"
    @echo "  websocat 'ws://localhost:7331/?token=dev'"
    @echo "  Then paste intents like:"
    @echo "  {\"type\":\"start_meeting\"}"
    @echo "  {\"type\":\"set_mode\",\"mode\":\"transcript\"}"
    @echo "  {\"type\":\"stop_meeting\"}"
```

- [ ] **Step 2: Update `apps/server/README.md`**

```markdown
# meeting-companion-server

Phase 0 stub server. See `docs/specs/server.md` for the spec.

## Run

```bash
MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331
```

Or via `just`:

```bash
just server-run
```

## Test

```bash
cargo test -p meeting-companion-server
```

## Manual smoke

In one terminal:
```bash
just server-run
```

In another:
```bash
websocat 'ws://localhost:7331/?token=dev'
```

Paste intents to interact:
```json
{"type":"start_meeting","description":"Q1 budget review","metadata":{"project":"helix"}}
{"type":"set_mode","mode":"transcript"}
{"type":"mark_moment","t":12345}
{"type":"stop_meeting"}
```

## Configuration

| Env / Flag                  | Default     | Description                  |
|-----------------------------|-------------|------------------------------|
| `MEETING_COMPANION_TOKEN`   | (required)  | Shared secret for WS auth.   |
| `RUST_LOG`                  | `info`      | tracing-subscriber filter.   |
| `--port`                    | `7331`      | TCP port.                    |
| `--bind`                    | `0.0.0.0`   | Bind address.                |

See `docs/specs/server.md` for the full protocol.
```

- [ ] **Step 3: Update top-level `README.md`**

```markdown
# Meeting Companion

Personal project. See `meeting-companion-architecture.md` for the system
spec and `docs/specs/` for component specs.

## Structure

- `apps/server/` — Rust WebSocket server (state owner). See [its README](apps/server/README.md).
- `apps/pwa/` — TypeScript PWA (forthcoming).
- `packages/contract/` — shared TS contract types.

## Quick start

```bash
just server-run     # starts the stub server on :7331 with token "dev"
just test           # runs all tests
```

## Plans & specs

- `docs/specs/` — component specs (server, pwa).
- `docs/superpowers/plans/` — implementation plans derived from the specs.
```

- [ ] **Step 4: Verify**

Run: `just --list`
Expected: shows `default`, `server-run`, `test`, `smoke-instructions`.

Run: `just test`
Expected: all 62 tests pass + contract package typechecks.

- [ ] **Step 5: Commit**

```bash
git add Justfile README.md apps/server/README.md
git commit -m "docs: smoke instructions + READMEs"
```

---

## Self-review

(Performed against `docs/specs/server.md`. Each row points to where the spec requirement is fulfilled.)

| Spec section                                                 | Implemented in            |
|--------------------------------------------------------------|---------------------------|
| §2.1 endpoint URL, bind, port, frame size                    | Tasks 3 (CLI), 10 (accept), main.rs |
| §2.2 auth via env token, constant-time, close 1008           | Task 10                   |
| §2.3 message envelope `serde(tag = "type")`                  | Task 4                    |
| §2.4 inbound intents (every row of the table)                | Tasks 6, 7, 8             |
| §2.5 outbound events                                         | Tasks 5, 6, 7, 8, 11, 13, 14, 15 |
| §2.6 concrete schemas                                        | Tasks 2 (TS), 4 (Rust)    |
| §2.7 protocol versioning (PROTOCOL_VERSION = 1)              | Task 4                    |
| §3 ServerState struct + invariants                           | Task 5                    |
| §4.1 state machine (every cell)                              | Tasks 6, 7, 8             |
| §4.2 start_meeting startup atomic order                      | Task 6                    |
| §4.3 stop_meeting teardown                                   | Task 6                    |
| §4.4 set_mode + unknown mode error                           | Task 7                    |
| §4.5 set_metadata insert/delete                              | Task 7                    |
| §4.6 mark_moment status ack                                  | Task 8                    |
| §4.7 expand_item + strategy-aware payload + unknown error    | Task 8                    |
| §4.8 reconnect snapshot                                      | Task 11 (reconnect test)  |
| §4.9 heartbeat 10s + status shape                            | Task 15                   |
| §5 configuration (env, CLI, defaults)                        | Tasks 3, 10               |
| §6.1 protocol error codes (bad_json, unknown_intent, bad_payload, unknown_mode, unknown_item) | Tasks 7, 8, 12 |
| §6.2 state errors silent ignore + WARN                       | Tasks 6, 7, 8             |
| §6.3 auth failure close code 1008                            | Task 10                   |
| §6.4 lagged consumer disconnect 1011                         | Task 11                   |
| §6.5 startup failures                                        | Tasks 3, 10               |
| §6.6 graceful shutdown 1001                                  | Task 16                   |
| §7 concurrency model + lock-then-act-then-emit               | Tasks 11, 13, 14, 15      |
| §8.1 mock generator 3s                                       | Task 13                   |
| §8.2 replace cap 10                                          | Task 9 (push_mock_item)   |
| §8.3 default mode catalog                                    | Task 5                    |
| §8.4 simulated extraction 1.5s + manual-wins merge           | Task 14                   |
| §8.5 display_tag omitted in stub                             | Tasks 5, 7                |
| §8.6 item content templates + detail synthesis               | Tasks 8, 9                |
| §9 logging via tracing                                       | All tasks                 |
| §10.1 unit tests (contract round-trip, state machine)        | Tasks 4, 5, 6, 7, 8, 9, 14 |
| §10.2 integration tests (every row of the matrix)            | Tasks 10, 11, 12, 13, 14, 15, 16 |
| §10.3 test conventions (spawn helper, port 0)                | Task 10 (common/mod.rs)   |
| §10.4 manual smoke (just smoke-instructions)                 | Task 17                   |
| §11 out of scope (no real audio, no real LLM, etc.)          | (acknowledged; no code)   |
| §12 open questions                                           | None.                     |

**Placeholder scan:** No `TODO`, `TBD`, `fill in details`, or `add appropriate error handling` strings remain in any task body.

**Type consistency:** `Item`, `Event`, `Intent`, `MeetingState`, `Status`, `ModeOption`, `ServerState`, `IntentOutcome`, `ServerHandle` are defined exactly once and referenced by name everywhere else. Method names (`apply_intent`, `snapshot`, `push_mock_item`, `current_mode_id`, `meeting_started_at`, `metadata_clone`, `set_metadata_full`, `snapshot_meeting_state`) are consistent across tasks that use them.

---

## Test count summary

- Unit tests: 16 contract + 5 + 9 + 5 + 5 + 5 (state) + 3 mock + 2 extraction = **50** unit tests.
- Integration tests: 3 handshake + 2 snapshot + 12 state_machine + 3 protocol_error + 5 mock_content + 3 extraction + 2 heartbeat + 1 shutdown = **31** integration tests.
- **Total: 81** tests at completion.

Each task ends with a green `cargo test` and a single commit. After Task 17, the stub server is ready as the contract reference for the PWA implementation plan that comes next.
