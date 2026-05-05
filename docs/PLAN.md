# Meeting Companion — Forward Plan

A living roadmap for the next major architectural evolution: split the
local server into a native Mac app + cloud-hosted server, with the PWA
remaining the rich control surface. Every component must work
standalone.

This document is forward-looking and revisable. Phases that ship get
distilled into ADRs (which capture the _why_ of decisions made) and
this doc loses those phases as it advances. ADRs are durable;
`PLAN.md` is provisional.

---

## 1. The shape we're building toward

```
                                  ┌────────────────────┐
                                  │ Server (AWS)       │
                                  │                    │
                                  │ • audio in         │
                                  │ • commands in      │
                                  │ • updates out      │
                                  │   (per active mode)│
                                  └──┬──────────────┬──┘
                                     │              │
                  audio + commands   │              │   commands
                  updates out        │              │   updates
                                     │              │
                              ┌──────▼─────┐  ┌─────▼──────┐
                              │ Mac app    │  │ PWA        │
                              │ (Swift,    │  │ (TS, in    │
                              │  menu bar) │  │  EvenHub)  │
                              │            │  │            │
                              │ Audio:     │  │ Control:   │
                              │  always    │  │  always    │
                              │            │  │            │
                              │ Control:   │  │ Audio:     │
                              │  optional  │  │  placeholder│
                              │            │  │  (goggles  │
                              │ View:      │  │   mic via  │
                              │  toggle    │  │   BLE → PWA)│
                              └────────────┘  └────────────┘
```

**Three components, three independent capabilities.** Both clients
are bidirectional: each can send commands and receive updates.
Pairing is additive — never required.

---

## 2. Principles

1. **Each phase is shippable on its own.** Stop at any phase, what
   you have still works.
2. **No throwaway code.** A phase's output feeds the next.
3. **Local-first dev path stays alive forever.** Single-machine
   end-to-end remains supported through every phase.
4. **Single-user-multi-device first.** Multi-tenant is Phase 7+.
5. **mnemo unchanged through this rollout.** Per-user mnemo identity
   is its own future track.
6. **Disabled-by-default for cloud features.** Local dev unaffected.
7. **Standalone-first.** Each component is independently useful.
   Pairing is additive.
8. **Capability over identity.** Every client declares capabilities
   (`audio_capture`, `screen_capture`, `control_surface`); roles in a
   meeting are filled by capability-bearers, not by device type.
9. **Container-anywhere, deploy-portable.** The server is a single
   Docker image. Local dev and production use the same image;
   configuration is env-only. Any host that runs Docker is a valid
   target.
10. **SQLite for state, Litestream for durability.** No external DB
    daemon; the database is a file. Production gets continuous
    streaming backup to S3-compatible storage. Migration to
    Postgres is a 2-3 day project later if scale demands.

---

## 3. Infrastructure target

### Stack — container-anywhere, SQLite-backed, Rust server

```
┌─────────────────────────────────────────────────────────────────┐
│ Single host (laptop, VPS, anywhere docker runs)                 │
│                                                                 │
│  ┌──────────────────────┐   ┌─────────────┐   ┌──────────────┐ │
│  │ Server container     │   │ SQLite      │   │ Blob storage │ │
│  │ (Rust binary)        │   │             │   │              │ │
│  │                      │   │ Single file │   │ FS volume    │ │
│  │ • WSS endpoint       │──▶│ in mounted  │──▶│ (local) OR   │ │
│  │ • Static blob serve  │   │ volume      │   │ S3-compatible│ │
│  │ • Migrations on boot │   │             │   │ (R2 in prod) │ │
│  │ • Caddy front (TLS)  │   │ Litestream  │   │              │ │
│  └──────────────────────┘   │ → S3 (prod) │   └──────────────┘ │
│           ▲                  └─────────────┘                    │
│           │ .env (Soniox, mnemo, Google OAuth, JWT secret)      │
│           │ stdout logging (host pipes to its log viewer)       │
└─────────────────────────────────────────────────────────────────┘
```

**Compute: single container.** Local: `docker compose up`. Production:
the same image on Hetzner / Fly / Railway / Render — whatever's
cheapest when we deploy. Caddy (or the host's TLS) terminates HTTPS.

**State: SQLite as a single file** in a mounted volume. The server
opens it at boot, runs SQLx migrations, and reads/writes during the
process lifetime. Active meeting state still lives in
Mutex-guarded server memory; SQLite is the durable record.

**Identity: Google OAuth.** Same plan as before. JWT issued post-callback.

**Backups: Litestream** (production only). Continuous async streaming
of the SQLite WAL to S3-compatible storage (R2 / B2 / S3). Sub-second
RPO, $0-1/month. Local dev: a `cp` on demand.

### SQLite schema

Stored in `packages/server/migrations/0001_initial_schema.sql`.

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE users (
  id            TEXT PRIMARY KEY,                 -- UUID v4
  email         TEXT NOT NULL UNIQUE,
  name          TEXT NOT NULL,
  google_sub    TEXT NOT NULL UNIQUE,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  last_seen_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE devices (
  id              TEXT PRIMARY KEY,               -- UUID v4
  user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  hostname        TEXT NOT NULL,
  capabilities    TEXT NOT NULL,                  -- JSON array (TEXT[])
  registered_at   TEXT NOT NULL DEFAULT (datetime('now')),
  last_seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
  online          INTEGER NOT NULL DEFAULT 0      -- 0/1 boolean
);
CREATE INDEX devices_by_user ON devices(user_id);

CREATE TABLE meetings (
  id                       TEXT PRIMARY KEY,      -- ULID (sortable by time)
  user_id                  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  title                    TEXT,
  description              TEXT,
  project                  TEXT,
  status                   TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'aborted')),
  started_at               TEXT NOT NULL,         -- ISO 8601
  ended_at                 TEXT,
  duration_seconds         INTEGER,
  audio_source_device_id   TEXT REFERENCES devices(id) ON DELETE SET NULL,
  metadata                 TEXT NOT NULL DEFAULT '{}',         -- JSON map
  transcript_url           TEXT,
  items                    TEXT NOT NULL DEFAULT '{"actions":[],"highlights":[],"open_questions":[]}',
  moments                  TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX meetings_by_user_started_at ON meetings(user_id, started_at DESC);
CREATE INDEX meetings_by_project ON meetings(user_id, project) WHERE project IS NOT NULL;
```

JSON columns stored as TEXT; the **JSON1 extension** (built into SQLite
≥ 3.9) gives `json_extract`, `json_set`, `json_array_append` for navigation
and updates. SQLx supports this natively.

**Pragmas at connection:**

```sql
PRAGMA journal_mode = WAL;        -- concurrent reads + single writer, no read blocking
PRAGMA synchronous = NORMAL;      -- safer than OFF, faster than FULL; safe with WAL
PRAGMA foreign_keys = ON;         -- not on by default in SQLite; we want CASCADE
PRAGMA busy_timeout = 5000;       -- 5s wait if a transient write conflict arises
```

**Write strategy** (same semantics as the DDB plan):

- On `Start`: `INSERT` creates a meetings row with `status='in_progress'`,
  empty `items`, empty `moments`.
- During the meeting: `UPDATE` on the same row each summarizer cycle
  (replace `items.highlights`, append to `items.actions` /
  `items.open_questions` via `json_set` / `json_insert`) and each
  moment (`json_array_append` on `moments`).
- On `Stop`: `UPDATE` sets `status='completed'`, `ended_at`,
  `duration_seconds`, `transcript_url`.
- On crash mid-meeting: row remains with `status='in_progress'`. UI
  surfaces this as "interrupted."

**Access patterns:**

- "List my devices" → `SELECT * FROM devices WHERE user_id = ?`.
- "List my meetings, most recent first" → `SELECT * FROM meetings WHERE user_id = ? ORDER BY started_at DESC LIMIT ?`.
- "Get one meeting" → `SELECT * FROM meetings WHERE id = ? AND user_id = ?`.
- "All actions across last 30 meetings" → SELECT + extract from `items` JSON, client-side filter.

**Transcripts are NOT in SQLite.** Server's in-memory rolling
transcript dumps to the BlobStore on Stop. One-time write per meeting.

### Blob storage — `BlobStore` trait, two implementations

A small Rust trait abstracts where blobs live. Same key shape in both:

```
meetings/<user_id>/<meeting_id>/transcript.md
meetings/<user_id>/<meeting_id>/captures/<capture_id>.jpg
meetings/<user_id>/<meeting_id>/moments/<moment_id>.jpg
meetings/<user_id>/<meeting_id>/audio.opus           (opt-in, future)
```

**`FilesystemBlobStore`** (default for local + small VPS deploys):
Writes under a configured root dir (e.g., `/data/blobs/`). Served by
the Rust server itself via `GET /api/blobs/<path>` with auth-checked
JWT — no presigned URLs needed because the server serves them. Simple,
zero external dependencies.

**`S3BlobStore`** (production with cloud blob hosting):
Speaks the S3 API. Works with **AWS S3**, **Cloudflare R2** (cheap,
no egress), **Backblaze B2**, **MinIO** (self-hosted). Returns
short-lived presigned URLs to clients. Configured by env vars
(`BLOB_STORE=s3`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_REGION`,
`S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`).

**Picking at runtime:** `BLOB_STORE` env var, `fs` (default) or `s3`.
Switch is a deploy-config change, no code change.

**Production blob storage of choice: Cloudflare R2** for the cost
profile (~$0.015/GB/month, free egress). Lifecycle rules on R2 cover
the same retention tiering we'd want on AWS S3.

---

## 4. Phase rollout

| Phase | Goal                                        | Estimated effort |
| ----- | ------------------------------------------- | ---------------- |
| 1     | `AudioSource` trait refactor (server only)  | 1-2 days         |
| 2     | Swift Mac app v1 + standalone modes         | 2-3 weeks        |
| 3     | Container deploy + SQLite + Google OAuth    | 1-1.5 weeks      |
| 4     | Post-meeting browse + Litestream backups    | 1-2 weeks        |
| 5     | Screen capture + Moments artifact           | 1-2 weeks        |
| 6     | Mac overlay UX (live view)                  | 1 week           |
| 7+    | Multi-tenant, multimodal, calendar, glasses | TBD              |

Total scope: ~7-10 weeks of focused work, deliverable in 6 demoable
milestones.

### Phase 1 — `AudioSource` trait refactor

Server-internal. No user-visible change.

**Goal:** decouple the audio pipeline from local-capture
implementation, so audio can come from in-process or a remote client.

**Server work:**

- `trait AudioSource` with two impls:
  - `LocalAudioSource` — current SCKit + mixer (default).
  - `RemoteAudioSource` — exposes `/audio` WebSocket endpoint, accepts
    PCM frames as binary messages.
- Pick at boot via `MEETING_COMPANION_AUDIO_SOURCE` ∈ `local|remote`.
- Downstream pipeline unchanged.

**Mac / PWA work:** none.

**Acceptance:** existing local mode unchanged; `/audio` endpoint
testable with `wscat` sending PCM.

**Risks:** low (pure refactor).

---

### Phase 2 — Swift Mac app v1 + standalone modes

The first user-visible change. Three demos must pass.

**Goal:** native Mac app exists. Both Mac and PWA work standalone.
Server runs locally still (cloud is Phase 3).

**Server work:**

- **Device registry** endpoints: `POST /devices`, `DELETE /devices/{id}`,
  `GET /devices`. Capability advertisement on registration.
- **Device control channel**: `DeviceCommand` enum delivered to clients
  with audio capability — `StartStreaming`, `StopStreaming`, ping.
- **Meeting → device binding**: `state.audio_source_device_id`. Lifecycle
  drives streaming commands to the bound device.
- **Snapshot extension**: `available_devices`, `audio_source_device_id`.
- **PWA-as-audio-source capability**: server accepts PCM from PWA-owned
  devices identically to mac-owned devices. Capability flagged off in
  PWA for now (placeholder for goggles-mic-via-BLE future path).

**Mac app work (new Swift Xcode project, `packages/mac/`):**

- SwiftUI + AppKit, `LSUIElement = true` (true accessory app).
- **Menu bar item** with state-reflecting icon (dim / neutral / rust /
  red-error).
- **Menu dropdown**: status line, "Start meeting…", "Stop meeting",
  "Meetings…", "Settings…", "Permissions…", "Quit".
- **Permissions onboarding** window: walks user through Microphone +
  Screen Recording grants on first launch.
- **Audio capture** via SCKit native Swift APIs (no Rust binding pain).
- **Audio mixer**: matches Rust impl behavior (~50 fps system audio + mic).
- **WS client** to `ws://localhost:7331/audio` with shared dev token.
  Streams binary PCM frames.
- **Compose window** (NSWindow, summoned on Start Meeting): description
  textarea, "Extract Tags" button, chip preview, Start / Cancel.
- **Meetings window** (NSWindow, summoned from menu): master-detail SwiftUI
  list. Loads from `GET /api/meetings`. Tabs in detail: Transcript,
  Highlights, Actions, Open Questions, Moments.
- **Settings window**: Account, General (start at login),
  Permissions tabs.

**PWA work:**

- **Audio source picker** (in settings or pre-meeting compose): list
  registered devices, radio-select one. PWA itself listed as a
  capability-disabled option (placeholder).
- **Pre-meeting state**: "Bind a capture device" hint when nothing is
  bound (Start Meeting disabled).
- **Snapshot reducer**: track `availableDevices`, `audioSourceDeviceId`.

**Three acceptance demos:**

1. **Mac standalone**: Mac running alone, click "Start Meeting" in menu
   bar, compose window appears, type description, click Start, audio
   captures, transcript flows to (currently no UI surface — just verify
   server-side processing).
2. **PWA-led with Mac as source**: both running, PWA picks the Mac as
   audio source, starts meeting from PWA, audio flows from Mac, items
   appear in PWA's items mirror.
3. **Mac standalone with browse**: open Mac's "Meetings…" window, see
   the meeting from demo 1 in the list, drill into detail, see transcript.

**Estimate:** 2-3 weeks (Mac app is the bulk).

**Risks:**

- SCKit Swift permission UX — must degrade gracefully.
- Audio mixer parity with Rust impl (audio quality regression invisible
  to compile-time tests).
- Frame loss at the new WS hop — backpressure / bounded queue with
  logged drops.
- Wire format with three consumers (Rust, TS, Swift) — defer codegen
  decision to Phase 3 (track this in a follow-up ADR).

---

### Phase 3 — Container deploy + SQLite + Google OAuth

Server gets dockerized. Adds SQLite + initial schema (users) + Google
OAuth. Same image runs locally and on whatever production host we
eventually pick. Local dev preserved (and is now the same artifact as
prod).

**Goal:** production-shaped deployment of the server, runnable on a
laptop or a $5 VPS, without committing to any specific cloud.

**Server work:**

- **Dockerfile** — multi-stage build, statically-linked Rust binary,
  ~30 MB final image. Includes Litestream binary alongside the server
  binary (or a sidecar container).
- **`docker-compose.yml`** at the repo root — server container + named
  volume for the SQLite file + named volume for blobs. Health check.
- **SQLx + SQLite** — add deps, set up `migrations/` directory with the
  initial schema (`0001_initial_schema.sql` containing the `users`
  table only — devices and meetings land in Phase 4). `sqlx::migrate!`
  applies on boot.
- **`BlobStore` trait** with `FilesystemBlobStore` impl — writes under
  `/data/blobs`, served via `GET /api/blobs/<path>` with auth-checked JWT.
- **Google OAuth** — `/oauth/google/callback`, server exchanges code,
  issues JWT (short-lived, ~15 min) + refresh token. User row
  upserted.
- **JWT middleware** — every authenticated endpoint validates JWT;
  attaches `user_id` to request context.
- **Per-user state container** — meeting state keyed by `user_id` in
  memory.
- **CORS** — configurable allowed origins (PWA in prod will be on a
  different origin).
- **Health endpoint** — `/healthz` for orchestration.

**Mac work:**

- **OAuth flow** — open browser → Google OAuth → callback to web page →
  "Open Desktop App" button → custom URL scheme `meeting-companion://`
  handoff with code → app exchanges code for JWT, stores in Keychain.
- **Server URL configurable** in Settings (default `localhost:7331` for
  dev; production URL when we pick one).
- **WS uses JWT**; auto-refresh on 401.
- **Auto-reconnect** with exponential backoff.

**PWA work:**

- **Google OAuth flow** — browser tab → Google → callback to PWA URL →
  PWA's auth handler exchanges code for JWT, persists via
  `bridge.setLocalStorage` + `localStorage` fallback.
- **Server URL configurable** (default localhost; production override
  field).
- **"Sign in with Google"** replaces shared-token UI.

**Acceptance:**

- `docker compose up` from a clean checkout produces a working server
  with an empty SQLite file.
- PWA on phone signs in via Google → identifies as you, persists JWT.
- Mac app signs in same way → registers under your account (devices
  table written in Phase 4; placeholder works in Phase 3 from
  in-memory).
- Local end-to-end smoke: full meeting on the same laptop with the
  containerized server.

**Estimate:** 1-1.5 weeks (Dockerfile + SQLx setup + OAuth + JWT
plumbing).

**Risks:**

- Google OAuth redirect URLs need correct registration in the Google
  Cloud Console (separate URLs for dev / prod).
- Custom URL scheme handoff on Mac (Safari handoff edge cases).
- SQLx compile-time query verification needs a dev DB at compile time;
  we use `sqlx prepare` to capture offline metadata for CI builds.

---

### Phase 4 — Post-meeting browse + Litestream backups

**Goal:** every meeting becomes a browsable artifact. Server gains
durable backups via Litestream replication.

**Server work:**

- **Schema migration** `0002_devices_and_meetings.sql` — adds the
  `devices` and `meetings` tables (per the schema in §3).
- **Repositories** in `packages/server/src/db/` — `users.rs`,
  `devices.rs`, `meetings.rs`. Each is a thin SQLx wrapper.
- **Persistence on `Start`**: `INSERT INTO meetings ...` with
  `status='in_progress'`, empty `items`, empty `moments`.
- **Persistence during the meeting**: `UPDATE meetings SET items = ?, moments = ?`
  every summarizer cycle (full JSON replacement) and per moment
  (`UPDATE moments = json_array_append(moments, ?)`).
- **Persistence on `Stop`**: `UPDATE meetings SET status='completed',
ended_at=?, duration_seconds=?, transcript_url=?`. Transcript
  serialized to markdown and written via the BlobStore.
- **Persistence on crash**: nothing — row stays `status='in_progress'`,
  surfaced in UI as "interrupted."
- **Read APIs:**
  - `GET /api/meetings` — paginated, sorted DESC by `started_at`,
    filterable by project + date.
  - `GET /api/meetings/{id}` — full meeting (items + moments embedded).
  - `GET /api/meetings/{id}/transcript` — redirects to BlobStore URL
    (or inline for FS-backed dev).
  - `DELETE /api/meetings/{id}` — user-initiated deletion; cascade
    to BlobStore prefix.
- **Litestream sidecar** — when `LITESTREAM_S3_*` env vars are set,
  Litestream replicates the SQLite WAL to S3-compatible storage.
  Local dev: not enabled.

**Mac work:**

- **Meetings window** (already scaffolded in Phase 2) now connects to
  the real `/api/meetings` endpoints.
- Detail view loads transcript from BlobStore; renders markdown.

**PWA work:**

- **Meetings view** (new top-level surface): list view, detail view
  with tabs.
- Routing: `/meetings`, `/meetings/{id}`. Active-meeting view stays
  at `/`.
- Export transcript as markdown.
- Delete with confirm.

**Acceptance:** run a meeting, click Stop. Open Meetings on either
Mac or PWA — meeting appears with title, duration, project tag.
Drill in, read transcript, see per-mode items. Forcefully kill the
server mid-meeting → restart → meeting shows `status=aborted` (or
`in_progress` until manual cleanup). Bring up Litestream against a
test S3 bucket → verify the WAL is replicating.

**Estimate:** 1-2 weeks.

**Risks:**

- SQLite-specific quirks: foreign key enforcement requires
  `PRAGMA foreign_keys = ON` per connection (SQLx supports this via
  pool init).
- `json_set` / `json_array_append` semantics for the items / moments
  updates — straightforward but worth a unit test.
- Storage cost projection: ~$5/month per 1000 meetings of typical size.

---

### Phase 5 — Screen capture pipeline + Moments artifact

**Goal:** "Mark moment" becomes a saved artifact with a screenshot.

**Server work:**

- `DeviceCommand::CaptureNow { moment_id }` — server-to-mac control
  message.
- Asset upload endpoint: `POST /api/captures` (multipart: image +
  metadata). Stores in S3 by meeting_id + moment_id (or timestamp for
  continuous captures).
- `Intent::MarkMoment` upgrade:
  - Persist `Moment` row in DDB.
  - Gather transcript context (item IDs ±30s).
  - If bound device has `screen_capture`, fire `CaptureNow`.
  - On capture upload, attach URL to moment.
  - Emit `Event::MomentSaved { moment }`.
- Continuous screen-capture acceptance (~1 fps from mac when capability
  active, opt-in per meeting).

**Mac work:**

- Screen capture in SCKit: `SCStreamOutputType::Screen` added to existing
  stream.
- Frame diff'ing: hash each frame, send only on change.
- JPEG encoding client-side, q=70.
- `CaptureNow` handler: fresh frame on demand, tagged with moment_id.
- Capability flag: `screen_capture: true` when permission granted.
- Pause/resume controls in menu bar — separate audio and screen.

**PWA work:**

- 📌 Mark Moment button in active-meeting CTA region (with optional
  note prompt, skippable).
- Moments tab in meeting detail view: card list with screenshot
  thumbnails + transcript context preview.
- Toast "📌 Moment saved" on `Event::MomentSaved`.

**Acceptance:** click Mark Moment during active meeting, see toast.
Open meeting detail later, see moment card with screenshot.

**Estimate:** 1-2 weeks.

**Risks:**

- ScreenCaptureKit screen permission UX.
- Continuous capture storage growth (mitigated by diff-skip + JPEG q=70
  - per-meeting size cap).

---

### Phase 6 — Mac overlay UX (live view)

**Goal:** native floating panel during meetings, with action buttons +
live state.

**Server work:**

- `connected_clients` snapshot field (which control surfaces are
  online).
- No major changes — Mac subscribes to event stream as a peer.

**Mac work:**

- Floating panel: `NSPanel`, `.floating` level, `canBecomeKey = false`,
  `hidesOnDeactivate = false`. Above all apps; doesn't steal focus.
- Position memory per-display.
- State subscription: WS to event stream. Swift state reducer mirrors
  PWA's.
- **Action buttons:** Mark Moment, Pause / Resume, Stop (arm-then-confirm).
- **Live state surface:** current mode label, last 1-2 items, optional
  collapsible pill mode.
- **Show / hide toggle:** Settings → "Show live overlay during meetings"
  (default off). Optional auto-hide-when-PWA-connected.

**PWA work:** none.

**Acceptance:** start meeting → overlay appears on Mac → click Mark
Moment in overlay → same effect as PWA. Drag overlay; restart Mac;
position persists.

**Estimate:** 1 week.

**Risks:**

- SwiftUI panel layering, focus, multi-display fiddly bits.

---

### Phase 7+ (deferred)

Sized as a separate planning round once Phase 6 is live and we have
real usage data. Topics:

- **Multi-tenant productionization.** Real auth provider (Clerk /
  Auth0), per-user isolation hardening, billing, quota.
- **PWA audio-source path activated** (goggles mic via BLE → PWA →
  server). Requires BLE audio + EvenHub WebView mic permission
  validation on real hardware.
- **Multimodal extraction.** Feed N most-recent screen captures to a
  vision-capable LLM; new "Visuals" mode.
- **Per-user mnemo identity** (mnemo-side change).
- **Calendar integration.** Auto-detect meeting starts.
- **Glasses gestures.** Hardware-dependent (ADR-0001 follow-up); fires
  existing intents.
- **Observability.** Tracing, metrics, alerting, error dashboards.

---

## 5. Cross-cutting decisions

These come due during the rollout.

**Wire format with three consumers** (Phase 2 surfaces this). Three
hand-maintained contract files become harder to keep in sync. At
Phase 2 end, decide between:

- (a) Continue hand-maintained, accept drift risk.
- (b) Adopt protobuf + codegen for Rust (`prost`), Swift
  (`swift-protobuf`), TS (`ts-proto` or similar).
- (c) TS-source-of-truth + `quicktype` to Rust + Swift.

I lean (b) — one-time cost, pays back forever.

**Identity model**. Single-user Google OAuth in Phase 3 is right.
Watch for places where `user_id` is hardcoded — keep parameterized so
Phase 7 multi-tenant doesn't re-architect.

**Storage cost ceiling**. Personal-use baseline (container-anywhere
deploy):

- VPS (e.g., Hetzner CX22, 4GB / 2 vCPU): ~$5-6/month
- SQLite: $0 (just a file in a volume)
- Cloudflare R2 (Litestream backups + blob storage): $0-1/month at
  free-tier volumes
- Domain + Let's Encrypt TLS via Caddy: $0-1/month (domain only)
- **Total: ~$5-8/month** before STT/LLM/mnemo external costs.

Local-only dev: $0.

**Privacy / consent**. By Phase 5 we capture audio + screen. The Mac
app needs:

- Persistent "RECORDING" indicator (separate audio + screen icons).
- One-click pause for either or both.
- Per-meeting opt-out of screen capture.
- Clear data retention policy in Settings.

**Local-only mode forever**. Run the project without cloud,
without OAuth, without mnemo. CI must support this path.

---

## 6. Open follow-ups

Not blocking Phase 1, but flagged for later resolution:

- **PWA audio source activation conditions:** when goggles mic
  becomes feasible. Tests needed: BLE audio for long sessions,
  EvenHub WebView mic permissions.
- **Standalone with no PWA AND no Mac:** degenerate case (e.g., future
  glasses-only). Defer until glasses hardware lands.
- **Per-user mnemo identity:** depends on a mnemo-side change. Forward
  compatibility today (`attributes.meeting_id`) keeps the door open.
- **Wire format codegen:** decision at end of Phase 2.
- **Production host:** picked when Phase 3 ships. Hetzner (cheapest +
  full control), Fly.io (zero-ops + container-native), Railway
  (simplest UX) are all viable. Same Docker image runs on any.
- **SQLite → Postgres migration:** if/when concurrent meetings or
  multi-instance scaleout matters. SQLx makes this a 2-3 day project
  later. Not on the radar at our scale.
