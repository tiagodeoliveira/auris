# Mobile Native App — Implementation Plan

A native phone client (Expo / React Native) that reaches feature parity
with the PWA for compose / live meeting / history / artifacts /
chat / moments, _without_ the EvenHub glasses bridge (browser-only
SDK). Treats the existing PWA as the canonical reference for flows
and visual language; reuse is opportunistic, not load-bearing.

This is forward planning, not yet decided. Companion to
[`PLAN.md`](PLAN.md); will graduate to an ADR once the stack and
wire-format strategy land.

---

## 1. Goal

Ship a native iOS + Android app via a single Expo codebase that
covers every PWA flow except glasses display. The phone takes over
the "control surface + audio source" role today filled by the PWA;
the Mac app and the PWA both keep operating unchanged. Glasses
bridging stays a PWA-only feature for now (see §9).

Primary use case: a user wants to start, observe, and chat with a
meeting from their phone — without opening the Mac, without the
glasses paired. The phone's mic is the audio source; the phone's
screen is the items HUD; the phone's camera adds a _new_ capability
the existing surfaces don't have — moment markers can attach a
photo of the actual scene (whiteboard, slide, person), not just a
desktop screenshot like the Mac does.

## 2. Out of scope

- **Glasses bridging.** `@evenrealities/even_hub_sdk` is browser-only
  and depends on a host-injected `evenAppBridge` global. No
  React-Native equivalent ships today. Phone runs without glasses.
  A future addition could let the phone act as a "remote" while the
  glasses are paired with the PWA on a Mac, but that's a separate
  architectural problem.
- **Background audio capture beyond OS quotas.** Mobile OSes
  aggressively suspend backgrounded apps. The first cut runs only
  while the app is foregrounded; "screen off, capture continues"
  needs platform-specific background-mode entitlements that warrant
  their own design pass.
- **Multi-window / split-view.** The Mac overlay's "compose vs live
  vs starting" mode swap maps cleanly to a navigation stack on
  mobile, no resizable single-window choreography.
- **Web-only auth flows.** Auth0 universal login works natively
  through `expo-auth-session`; we don't need to reuse the PWA's
  redirect dance.

## 3. Workspace organization

New package: `packages/mobile`. Sits alongside `packages/server`,
`packages/pwa`, `packages/mac`. Self-contained Expo project with its
own `app.json`, `package.json`, `eas.json` (for native builds).

The wire types (`contract.ts`-shaped + REST clients) are
**hand-synced** in this package for v1 — same status quo as PWA,
Mac, and server. No interim "shared-ts" package; when extraction
happens, it'll be the full codegen step (§4) covering all four
clients at once, not a TS-only halfway house that has to be
re-migrated later.

## 4. Wire-format strategy

Two options:

1. **Hand-sync until codegen lands.** Each wire change is a 4-place
   edit (Rust + Swift + 2× TS). Cost is linear in wire-change
   frequency. **Chosen for v1.**

2. **Codegen across all four clients (PLAN.md §4.2 plan b).**
   protobuf + `prost` (Rust) + `swift-protobuf` (Mac) + `ts-proto`
   (PWA + mobile). One-time setup; covers every client uniformly
   so there's no second-class citizen. Right answer; lands as a
   discrete project once the mobile wire surface stabilizes and we
   can migrate all four clients in lockstep behind a generated
   schema.

The decision is "stay hand-synced now, do the full codegen later" —
not "do a TS-only intermediate, then migrate twice." The mobile
client is a useful forcing function for the codegen ADR (PLAN.md
§4.2 says "write the ADR when the next non-trivial wire change
comes due") because it makes the 4-place edit cost visible.

## 5. Stack

- **Expo SDK 51+** with the new architecture (Fabric / TurboModules
  enabled). Expo Router for file-based navigation.
- **TypeScript** end-to-end. No vanilla JS.
- **State**: Zustand. Single store, subscribers, mirrors the
  PWA's `createStore` semantics with a battle-tested implementation.
- **Auth**: `expo-auth-session` (Auth0 PKCE flow, native callback
  via app scheme), `expo-secure-store` for refresh-token persistence.
- **REST**: `fetch` with a thin wrapper for `Authorization` header
  injection. No React Query in v1 — meeting/artifact lists are
  fetched on-demand and not heavily re-queried.
- **WebSocket**: built-in `WebSocket` (RN polyfill). Same reconnect
  - heartbeat logic as `pwa/src/ws.ts`, ported.
- **Audio**: `expo-audio` (Expo 51+) for raw PCM streaming. Falls
  back to `react-native-audio-record` if `expo-audio` doesn't
  expose enough — see §6.4.
- **Persistence (non-secret)**: `expo-sqlite` for any local
  caching. `AsyncStorage` for simple key/value (UI prefs, last
  meeting id).
- **Image rendering**: `expo-image` for moment screenshots and
  artifact thumbnails (HTTP cache, lazy load).
- **Theming**: `useColorScheme` for system theme; user override
  parallel to Mac's `OverlayTheme` setting.
- **Build & distribution**: EAS Build for ad-hoc TestFlight + Play
  internal track. No Expo Go (we'll need native modules for audio).

## 6. Architecture per layer

### 6.1 Auth

- Auth0 application registered as **Native** (mobile) — separate
  client_id from the PWA's SPA app. Adds the iOS bundle id /
  Android package as allowed callbacks.
- Login flow: `expo-auth-session` opens the system browser (not an
  embedded webview — Apple rejects embedded for OAuth) → user signs
  in → callback `myapp://auth-callback` → app extracts authorization
  code → exchange for tokens via Auth0 token endpoint.
- Token storage: refresh token in `expo-secure-store`; access token
  in memory only (matches PWA behavior).
- Token refresh: silent refresh on every WS reconnect + every REST
  request, gated by `getAccessToken()` returning a fresh token.
- Sign-out: clear secure store, disconnect WS, navigate to login
  screen. Same shape as `AppModel.disconnect()` on Mac.

### 6.2 State

Single Zustand store mirroring `pwa/src/types.ts::defaultAppState()`.
Subscriber-based; views read slices via `useStore(selector)`.

Key slices (matching PWA):

- `meetingState`: `idle | active | paused`
- `currentMeetingId: string | null`
- `currentMode: string` (transcript / highlights / actions /
  open_questions / summary / chat)
- `availableModes: ModeOption[]`
- `itemsByMode: Record<string, Item[]>`
- `liveTranscriptInterim: string`
- `metadata: Record<string, string>`
- `description: string` (compose draft)
- `attachedArtifactIds: string[]`
- `pendingArtifactAttachments: string[]`
- `wsStatus: 'connecting' | 'connected' | 'disconnected'`
- `auth: identity | null`

The PWA's items-mirror DOM-diffing logic doesn't translate — React
Native handles list reconciliation through `FlatList` + `keyExtractor`,
which is the equivalent semantic guarantee. Same anti-flicker
property (untouched rows preserve identity), expressed differently.

### 6.3 Networking

- **WebSocket client**: port `pwa/src/ws.ts`. Reconnect with
  exponential backoff, ping/pong heartbeat, token refresh on every
  reconnect. Single connection per session.
- **REST client**: port `pwa/src/meetings-api.ts` and
  `artifacts-api.ts`. Hand-synced TS copy living under
  `packages/mobile/src/` — same shape as the PWA copy until codegen
  lands (§4).
- **Connectivity awareness**: `@react-native-community/netinfo` to
  detect offline state and pause reconnect attempts; resume on
  network return. PWA doesn't have this because browsers handle it
  implicitly — mobile networks need explicit handling.

### 6.4 Audio pipeline (deep dive — biggest risk)

The PWA's audio path:

1. `MediaRecorder` captures from the browser's default audio input.
2. Web Audio API resamples to 16kHz mono `Float32Array` frames.
3. Frames pass through a VAD (`pwa/src/stt/vad.ts`) to gate upload —
   silence frames are dropped.
4. Voiced frames stream over `/stt` WebSocket as PCM bytes.

Mobile equivalents:

- **Capture**: `expo-audio` (Expo 51+) exposes a recorder with
  configurable sample rate, channel count, and buffer size. We'd
  request 16kHz mono if the OS allows; otherwise capture at the
  device's native rate (typically 44.1kHz or 48kHz) and resample
  client-side.
- **Resampling**: pure JS resampler (linear interpolation 48k→16k is
  ~50 lines, well-tested implementations exist as MIT-licensed
  one-files). Runs on the JS thread today; if it shows up in
  profiling, move to a JSI / Reanimated worklet. Server-side
  resampling is also viable but doubles bandwidth.
- **VAD**: port `pwa/src/stt/vad.ts` directly — it's pure signal
  processing on PCM samples, no platform deps. Same threshold
  tunables.
- **Streaming upload**: same `/stt` WebSocket protocol the PWA uses.
  Server doesn't care which client is speaking; the WS frame format
  is wire-defined.
- **Permissions**: `expo-audio` requests mic permission via
  `requestPermissionsAsync()`. iOS info.plist + Android manifest
  entries needed for first-launch prompt copy.

**Risk**: if `expo-audio`'s recorder doesn't expose raw PCM frames
(only file-based recording), fall back to `react-native-audio-record`
(community library, native module, well-trodden — used by several
voice-AI apps). Worth a 1-day spike to confirm before committing
the build order.

### 6.5 Theming + design system

Phone client mirrors the Mac overlay's palette (`MCTheme.panel`,
`text`, `border`, blue accents, amber moment indicator, danger red).
PWA's design system in `style.css` is the visual reference; we
re-implement those tokens as TS objects + StyleSheet entries.

Light/dark theme picker as a settings option, parallel to the Mac
overlay's `OverlayTheme`. Backed by `AsyncStorage`.

### 6.6 Moment image capture (camera + app screenshot)

Today the Mac uses ScreenCaptureKit to attach a desktop screenshot
to every moment; the PWA marks moments without an image attachment
(browsers can't grab the user's screen without intrusive permissions
flows). The phone has two affordances neither of those surfaces has:

- **Rear camera** — a moment can attach a _photo of the scene_: a
  whiteboard, a slide deck on a projector, the person speaking, a
  product. The vision-LLM moment summarizer (already shipping —
  see commit `33ee736`) will reason about it the same way it
  reasons about a Mac desktop screenshot.
- **App-screen capture** — `react-native-view-shot` snapshots the
  current app view (items HUD). Less interesting than the camera
  affordance, but useful when the user wants to bookmark "what was
  on my screen at this moment" with no extra interaction.

Proposed UX:

- Tap "Mark moment" → bottom sheet with three options:
  1. **Quick capture** — opens rear camera in a single-tap shutter
     mode, snaps a photo, attaches automatically.
  2. **No image** — fires the moment intent without an attachment;
     same flow as the PWA today.
  3. **Snapshot HUD** — captures the current app screen as the
     moment image (rare; useful for "this transcript line was
     interesting" bookmarking).
- Long-press the moment button = quick-camera shortcut; tap = open
  the sheet. Mirror-friendly with how the Mac quick-marks via
  ⇧⌘M (which the phone has no equivalent for).

Wire-format implications:

- Server already accepts moment image uploads via the existing
  artifact-summarizer path; mobile just POSTs the JPEG/PNG to the
  same endpoint and references the resulting artifact id in the
  `mark_moment` intent. **No server changes required.**
- File size: cap uploaded JPEGs at ~1.5MP, ~85% quality (~150-300KB
  per image). Vision summarizer doesn't need 12MP camera resolution.

Permissions:

- `expo-camera` for capture; mic permission already requested in
  §6.4. iOS info.plist: `NSCameraUsageDescription` ("Attach a photo
  to a meeting moment").
- Long-press shortcut should _not_ prompt every time — first-launch
  permission, then quiet thereafter.

## 7. Surface inventory

Screen-by-screen mapping. Each row is "PWA flow → mobile screen."

| PWA flow                                                                        | Mobile screen                          | Notes                                                                                                      |
| ------------------------------------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Login screen                                                                    | `app/login.tsx`                        | One button → Auth0 universal login.                                                                        |
| Compose meeting                                                                 | `app/(tabs)/index.tsx`                 | Description input + extract-tags button + attach-artifact button + start. Default tab.                     |
| Active meeting overlay                                                          | `app/(modal)/meeting.tsx`              | Modal-style fullscreen route (Expo Router stacks). Mode tabs + items list + live transcript footer.        |
| Mode tabs (transcript / highlights / actions / open_questions / summary / chat) | Segmented control inside `meeting.tsx` | Same labels as PWA `mode-tabs.ts`.                                                                         |
| Items list per mode                                                             | `FlatList` inside `meeting.tsx`        | One row component per mode shape (highlight / action / open_question / transcript / summary / chat)        |
| Item expand / detail                                                            | Inline accordion in the row            | Same chevron pattern as PWA's items-mirror; tap toggles, fires `expand_item` if no detail yet.             |
| Chat input                                                                      | Bottom-anchored TextInput in chat mode | RN `KeyboardAvoidingView` for the keyboard slide.                                                          |
| Mark moment                                                                     | Bookmark button in meeting toolbar     | Tap → bottom sheet (camera / no-image / app-snapshot). Long-press → quick camera shortcut. See §6.6.       |
| Camera capture (moment image)                                                   | Sheet from moment button               | `expo-camera` shutter, 1.5MP cap, attaches JPEG to moment intent.                                          |
| Pause / resume / stop                                                           | Toolbar buttons in `meeting.tsx`       | Same intents over WS as PWA.                                                                               |
| Meeting history list                                                            | `app/(tabs)/history.tsx`               | Master list, grouped by relative bucket (today / yesterday / this week / older).                           |
| Meeting detail                                                                  | `app/meeting/[id].tsx`                 | Title (metadata.title) + timing + description (collapsible) + metadata + moments + LLM usage + transcript. |
| Artifacts library                                                               | `app/(tabs)/artifacts.tsx`             | List + upload (camera / files / paste).                                                                    |
| Artifact upload                                                                 | Modal from artifacts tab               | `expo-image-picker`, `expo-document-picker`.                                                               |
| Artifact attach (compose / live)                                                | Modal from compose or meeting          | Multi-select picker over artifacts library.                                                                |
| Settings                                                                        | `app/(tabs)/settings.tsx`              | Theme picker, sign out, server URL? (probably env-baked, not user-edited).                                 |
| Auth0 callback                                                                  | `app/auth-callback.tsx`                | Handles deep-link return from system browser.                                                              |

The tab bar is: **Compose · History · Artifacts · Settings**. The
active-meeting view is _modal_ (above the tab bar) so it can't be
accidentally backgrounded by tapping a tab.

## 8. Build order

Phased, each phase ends with a usable demo state.

### Phase 0 — Skeleton

1. `packages/mobile` Expo project bootstrapped with TypeScript,
   Expo Router, Zustand, expo-auth-session.
2. Empty tab bar (Compose / History / Artifacts / Settings) with
   placeholder screens.
3. EAS Build configured; first dev build runs on a real device.

### Phase 1 — Auth + transport

4. Auth0 native app registered; PKCE flow end-to-end (sign in,
   refresh, sign out) with refresh-token persistence in
   `expo-secure-store`.
5. Hand-port `contract.ts`, `meetings-api.ts`, `artifacts-api.ts`,
   and `ws.ts` from the PWA into `packages/mobile/src/`. Drift is
   accepted until the codegen ADR lands (§4).
6. Mobile connects, receives snapshot, displays meeting state in
   the Compose tab.

### Phase 2 — Bare meeting flow (no audio yet)

7. Compose tab: description input + start button. Send
   `start_meeting` intent; UI transitions to the active meeting
   modal on `meeting_state_changed → active`.
8. Active meeting modal: mode tabs + per-mode items list. Read-only
   first — items flow in from the server (a Mac or PWA elsewhere
   acts as the audio source).
9. Stop / pause / resume buttons working.

This phase already gives a useful "phone as observer of an existing
meeting" experience.

### Phase 3 — Audio capture

10. Audio spike (1-day): confirm `expo-audio` raw-PCM viability vs
    `react-native-audio-record`. Pick.
11. Mic capture → resample → VAD → stream over `/stt`. Phone now
    drives a meeting end-to-end.
12. Audio source visualization (peak meter) similar to Mac
    `MicActivityIcon`.

### Phase 4 — Item interactions

13. Item detail expand (tap chevron → `expand_item` intent →
    `item_updated` event populates detail). Same cross-surface
    auto-expand semantics as PWA.
14. Chat mode: typing area + Q+A bubbles + send button. Streaming
    visual feedback matches PWA `chat-bubble-pending`.
15. Mark moment (bare): tap button, fire intent, no image. Mirrors
    PWA moment behavior.
16. Camera-attached moments: bottom sheet on moment tap, rear
    camera shutter, JPEG upload + reference in mark_moment intent.
    Long-press shortcut for quick capture. See §6.6.

### Phase 5 — History + artifacts

17. Meeting history list with bucket grouping.
18. Meeting detail screen — full inventory: title, description
    (collapsible), metadata, moments (with image from blob server,
    whether captured by Mac/desktop or by phone/camera), LLM usage,
    transcript.
19. Artifacts library: list, upload (camera / files / paste-text),
    delete, attach to compose meeting.
20. Live attach (artifact picker accessible from active meeting).

### Phase 6 — Polish

21. Light/dark theme picker; OS theme follow option.
22. Empty states + error states (no meetings, no artifacts, offline).
23. Background-aware reconnect (NetInfo + foreground events).
24. Push notifications? Out of scope v1; flagged for Phase 7.

## 9. Glasses follow-up

A separate phase, not v1. Two viable approaches if/when we want
glasses on mobile:

- **Native iOS/Android port of the EvenHub bridge.** Even Realities
  hasn't published native SDKs publicly; would require BLE
  reverse-engineering from the JS bridge source. High effort.
- **Phone as remote control while glasses paired with PWA on Mac.**
  The Mac runs the PWA in a hidden WebView, which keeps the glasses
  bridge active; the phone sends "scroll" / "click" intents over
  WebSocket; the server fans them out to the PWA, which calls into
  the EvenHub SDK. Interesting but requires a "control surface
  identity" concept on the server (who's driving glasses right now?)
  that doesn't exist today.

Both are explicitly _future_ work. Mobile v1 ships without glasses.

## 10. Open questions

- Does `expo-audio` 51+ expose raw PCM frame callbacks, or only
  file-based recording? Must answer in Phase 3 spike.
- Do we want to share Auth0 tenants between PWA + mobile, or
  separate tenants? Same tenant + different applications is the
  default; same tenant means the same identity across surfaces (a
  user's mobile sign-in mirrors their PWA sign-in).
- Universal links vs custom scheme for OAuth callback? Custom
  scheme (`meetingcompanion://auth-callback`) is simpler; universal
  links are cleaner UX (no "open in app" prompt). v1: custom
  scheme.
- Server-side considerations: any user-agent tracking we want for
  per-client metrics? Today the server doesn't distinguish PWA from
  Mac in its event log — adding "client kind" to the WS handshake
  is a small contract addition we should think about before three
  client kinds make this hard to add.

---

## References

- [`docs/PLAN.md`](PLAN.md) — current roadmap; §4.2 wire-format
  codegen is the closest existing entry, this plan refines it.
- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — current two-client
  shape; this plan adds a third.
- [`docs/PROTOCOL.md`](PROTOCOL.md) — wire protocol mobile must
  speak; same as PWA.
- [`docs/adr/0004-websocket-protocol.md`](adr/0004-websocket-protocol.md)
- [`docs/adr/0006-live-audio-stt-pipeline.md`](adr/0006-live-audio-stt-pipeline.md)
  — server-side STT contract; mobile audio path must produce frames
  it accepts.
- [`docs/adr/0009-pwa-ux-design-system.md`](adr/0009-pwa-ux-design-system.md)
  — visual reference; mobile mirrors the same tokens.
