# Cross-Surface Coordination

How Auris stays consistent across the three primary surfaces — Mac, mobile,
PWA — and the single server they all talk to. When two surfaces disagree
about something, this document is the tiebreaker.

This is a living spec: when a behavior changes in code, change it here too.
Each rule cites the load-bearing implementation files so the link between
intent and code stays clear.

---

## Vocabulary

- **User** — an Auth0 identity. Has exactly **one** running meeting at a
  time, across all surfaces, simultaneously.
- **Surface** — a client app: Mac (`packages/mac`), mobile (`packages/mobile`),
  PWA (`packages/pwa`). A user may have 0..N surfaces open at the same time.
- **Session** — an authenticated WebSocket connection from a surface to the
  server. A user's surfaces map 1:1 to sessions while connected.
- **Device** — a server-registered piece of hardware that can produce audio
  (and, in the future, screen capture). Devices carry `capabilities`. Today
  only the Mac app registers itself as a device, with `["audio_capture"]`.
- **Audio source** — where the active meeting's audio comes from. Either a
  server-registered Device (currently always a Mac) or the local microphone
  of the surface that started the meeting (mobile only).
- **Meeting** — a recording session. Has one active state at a time per user;
  on `stop_meeting`, it transitions to a past meeting (read-only).

---

## Rule 1 — Meeting state is shared across all of a user's surfaces

When a meeting is active for a user, **every** session of that user observes
the same meeting state in real time. Transcript items, mode tabs, chat
history, metadata (tags), and moments all propagate via server-broadcast
WebSocket events.

### How it works (server)

The server keeps a per-user "active meeting" record and a per-user fan-out
list of WS sessions. When any intent mutates meeting state, the server
applies it once, then broadcasts the resulting event to **all** of the user's
sessions — including the originating one (so optimistic UI can be reconciled).

### Surface responsibilities

| Event                                                | Mac                                             | Mobile                                 | PWA                           |
| ---------------------------------------------------- | ----------------------------------------------- | -------------------------------------- | ----------------------------- |
| `meeting_state_changed { active }` started elsewhere | Show overlay if `overlayAutoShow=true` (Rule 5) | Auto-push `/meeting` modal             | Switch idle → active layout   |
| `meeting_state_changed { idle }` from elsewhere      | Hide overlay                                    | Pop `/meeting`, return to tabs         | Switch back to compose layout |
| `items_update` (transcript / highlights / etc.)      | Append to active mode list                      | Append to active mode list, animate in | Same                          |
| `chat { question, answer }`                          | Append bubble pair                              | Append bubble pair                     | Append bubble pair            |
| `moment_added`                                       | Slide-in toast + add to moments list            | Add to moments list                    | Add to moments list           |
| `metadata_changed`                                   | Update tags strip                               | Update MetadataEditor chip row         | Update KV editor              |
| `devices_changed`                                    | (n/a — Mac IS the device)                       | Update AudioSourcePicker               | Update audio source select    |

### Status

| Capability                                            | Status        | Notes                                                                                              |
| ----------------------------------------------------- | ------------- | -------------------------------------------------------------------------------------------------- |
| Server broadcast on state change                      | ✓ implemented | Verify reaches ALL sessions, not just the originator                                               |
| Mobile auto-navigate on active                        | ✓ implemented | `app/(tabs)/index.tsx` useEffect                                                                   |
| Mac overlay auto-show on remote start                 | ✓ implemented | See Rule 5 (subject to `overlayAutoShow`)                                                          |
| Chat history in `MeetingDetail.items_by_mode["chat"]` | ✓ server-side | Server populates the field; mobile still needs to add the Chat tab to render it (PWA already does) |

---

## Rule 2 — A Mac app is a universal audio source

Any surface (Mac itself, mobile, PWA) can select any connected Mac app as
the audio source for a new meeting. If the user has multiple Macs registered
to their account simultaneously, **all** Macs appear in the picker and the
user chooses one.

### How it works

- The Mac app sends `{ type: "register_device", hostname, capabilities: ["audio_capture"] }`
  on every WS (re)connect.
- The server stores `Device { id, hostname, capabilities, online: bool }`.
- On any session connect, the server includes the user's full `availableDevices`
  list in the initial `snapshot` event, then broadcasts `devices_changed` on
  any subsequent connect/disconnect.
- When a surface starts a meeting with `audio_source_device_id` set to a
  Mac's device id, the server signals that Mac (via its own WS session) to
  begin streaming audio to `<server>/audio?token=<jwt>`.

### Surface responsibilities

- **All surfaces**: render the picker filtered by `capabilities.includes("audio_capture")`.
  Online devices selectable; offline devices shown as disabled (`hostname (offline)`).
- **Mac**: register on connect. Re-register on every reconnect. Drop the
  registration record on graceful disconnect (server can also detect via
  WS close).

### Wire contract

```ts
// Intent
{ type: "register_device", hostname: string, capabilities: Capability[] }

// Event (broadcast to all of user's sessions)
{ type: "devices_changed", devices: Device[] }

// In start_meeting
{ type: "start_meeting", description?: string, audio_source_device_id?: string, ... }
```

### Status

| Capability                  | Status                                                                 |
| --------------------------- | ---------------------------------------------------------------------- |
| Mac registers on connect    | ✓ implemented                                                          |
| Multi-Mac listing           | ✓ implemented                                                          |
| Audio routing to chosen Mac | ✓ implemented                                                          |
| Picker on each surface      | ✓ implemented (mobile, PWA); n/a for Mac (Mac picks itself by default) |

---

## Rule 3 — Mobile mic is local-only

The mobile app's microphone is **not** a server-registered device. It cannot
be selected from the Mac app, PWA, or another mobile device. Mobile's audio
source picker shows: (a) "This phone (microphone)" as a synthetic local
option, plus (b) any connected Macs (Rule 2).

### Rationale

- Mobile clients are ephemeral (apps backgrounded, screens locked) — they
  can't reliably accept streaming-audio commands from elsewhere.
- iOS's background-audio-capture story is fragile and platform-restricted.
- A user holding their phone and starting a meeting is implicitly saying
  "capture me right now"; no remote-trigger UX is needed.

### How it works

- Mobile's `AudioSourcePicker` injects a sentinel entry with `LOCAL_MIC_ID =
"__local_mic__"`.
- When the user picks the local mic, compose translates the sentinel to
  "omit `audio_source_device_id`" in the `start_meeting` intent.
- The mobile client uses `@siteed/expo-audio-studio` to stream PCM16 LE @
  16 kHz mono to `<server>/audio?token=<jwt>` — same endpoint as the Mac
  client, same wire format.

### Constraint

The mobile mic **cannot** stream system audio from other apps (Zoom,
FaceTime, phone calls). This is an iOS sandbox limitation, not a feature
gap. See _Platform constraints_ below.

### Status

| Capability                          | Status                                                            |
| ----------------------------------- | ----------------------------------------------------------------- |
| Local mic sentinel in picker        | ✓ implemented                                                     |
| Mobile mic NOT registered as Device | ✓ implemented (by omission)                                       |
| PCM streaming wired                 | ⏳ implemented but **not verified end-to-end** (see _Open items_) |

---

## Rule 4 — PWA has no audio source yet

The PWA's audio source picker exists but currently shows only registered
Macs. Browser microphone capture is not implemented (browser mic via
`getUserMedia` would work technically, but the server-side routing for a
session-scoped capture stream isn't built).

Until that ships, the PWA's no-device state shows a placeholder hint:
`── open the Mac app to start.` (or equivalent em-dash voice).

### Surface responsibilities

- **PWA**: render the picker filtered by `capabilities.includes("audio_capture")`.
  If empty, show the placeholder. Do not offer browser-mic as an option.

### Status

| Capability                  | Status             |
| --------------------------- | ------------------ |
| No-device placeholder       | ✓ implemented      |
| Browser-mic as audio source | ⏳ not implemented |

---

## Rule 5 — Mac overlay show/hide with memory

The Mac app's overlay (the wide-shallow floating window) shows automatically
when a meeting is active, **unless** the user has dismissed it. Once
dismissed, it stays dismissed even when meetings start on other surfaces —
the user has expressed preference. The overlay re-shows automatically only
when:

1. The user explicitly opens it from the menu bar, OR
2. The user starts a meeting **on the Mac itself**.

Conversely, if a meeting starts on mobile or PWA while the user is on the
Mac with `overlayAutoShow=true`, the overlay should appear automatically.

### State machine

```
Settings flag (persisted in UserDefaults):
  overlayAutoShow: Bool  (default true)

Events:
  meeting_state_changed { active }   →
    if startedOnThisMac OR overlayAutoShow:
      showOverlay()
  meeting_state_changed { idle }     →
    hideOverlay()
  user dismisses overlay manually    →
    overlayAutoShow = false
    hideOverlay()
  user opens overlay from menu bar   →
    showOverlay()
    (does NOT reset overlayAutoShow — manual open is one-time)
  user reopens "Show overlay on remote meetings" in Settings →
    overlayAutoShow = true
```

### Status

| Capability                                 | Status                                        |
| ------------------------------------------ | --------------------------------------------- |
| Overlay shows when local meeting starts    | ✓ implemented                                 |
| Overlay shows when remote meeting starts   | ✓ implemented (subject to `overlayAutoShow`)  |
| `overlayAutoShow` memory of manual dismiss | ✓ implemented                                 |
| Settings toggle to re-enable auto-show     | ✓ implemented (Account tab → Overlay section) |

---

## Single-active-meeting enforcement

A user has at most one active meeting at any time, regardless of how many
surfaces they have open. The UX is **never rejection** — the Start button
is disabled on every surface whenever a meeting is active anywhere, so a
duplicate `start_meeting` can't reach the server through normal use. When
a user opens any surface while a meeting is already running, that surface
auto-routes to the active-meeting view (overlay on Mac subject to Rule 5;
`/meeting` modal on mobile; active layout on PWA) where they can chat,
read the transcript, mark moments, and so on.

### Behavior at each touchpoint

When `meeting_state_changed { active }` reaches an idle surface, that
surface enters the live meeting view automatically:

- **Mac** — overlay appears (subject to `overlayAutoShow`, Rule 5).
- **Mobile** — `app/(tabs)/index.tsx` useEffect pushes `/meeting`.
- **PWA** — `compose-region.ts` / `compose-start.ts` hide the idle compose
  surface; the active-meeting layout takes over.

Start buttons are bound to `meetingState`:

- Active or paused → disabled / hidden
- Idle → enabled

### Server contract (defense-in-depth)

If a duplicate `start_meeting` somehow arrives (clock skew, race after a
client refresh, etc.), the server must not create a second meeting record.
The correct behavior:

```
On start_meeting intent:
  if user has active meeting → no-op + re-broadcast meeting_state_changed
                               { active } to the requesting session; log a
                               warning
  else                       → create meeting, broadcast meeting_state_changed
```

No `error` event is needed. The client's existing
`meeting_state_changed` handler will route the surface into the live
meeting view — which is where the user wanted to be anyway.

### Status

| Capability                                                   | Status                                               |
| ------------------------------------------------------------ | ---------------------------------------------------- |
| Mac Start disabled when active                               | ✓ implemented                                        |
| Mobile Start disabled when active                            | ✓ implemented                                        |
| PWA Start disabled when active                               | ✓ implemented                                        |
| Mac auto-show overlay on remote active                       | ✓ implemented (subject to `overlayAutoShow`, Rule 5) |
| Mobile auto-push /meeting on active                          | ✓ implemented                                        |
| PWA auto-switch layout on active                             | ✓ implemented                                        |
| Server treats duplicate start_meeting as no-op + rebroadcast | ✓ implemented                                        |

---

## Platform constraints

These cannot be lifted with engineering effort — they reflect actual
sandbox boundaries set by Apple / Google. Listed here so future product
discussions don't re-litigate them.

### iOS

- Cannot programmatically capture audio from other apps. Zoom, FaceTime,
  WhatsApp, and phone-call audio are all unreachable. ReplayKit's
  `RPSampleBufferTypeAudioApp` is filtered to the broadcast app's own
  audio + system-level UI sounds.
- Cannot programmatically capture screen content for moment screenshots.
- Background microphone is allowed only with `UIBackgroundModes: ["audio"]`
  (already set) and a visible recording indicator (already provided by the
  system).

### Android

- `MediaProjection` + `AudioPlaybackCapture` can technically capture some
  other-app audio, but **excludes** `USAGE_VOICE_COMMUNICATION` — which
  covers every video-call app. Effectively no useful capture for
  meeting-companion use cases.
- Call recording is policy-banned on Play Store since 2022.

### Browser (PWA)

- Cannot capture audio from other tabs reliably (the user would have to
  explicitly share the tab via `getDisplayMedia`).
- Cannot register as a persistent Device (no stable identity across
  sessions; ephemeral by design).

---

## Wire contract additions still needed

### `GET /meetings/:id/artifacts`

List of artifacts currently attached to a meeting. The PWA already attaches
via `POST /meetings/:id/artifacts {artifact_id}` and detaches via
`DELETE /meetings/:id/artifacts/:aid`, but there's no list endpoint, so the
mobile detail view renders a placeholder. Server contract:

```
GET /meetings/:id/artifacts
  → 200 { artifacts: Artifact[] }   // empty array if none
  → 404 if meeting unknown / not owned by user
```

Once available, mobile renders these as a section on the past meeting
detail screen (currently a placeholder card).

### `MeetingDetail.items_by_mode["chat"]`

Chat history on past meetings. The PWA renders this on the meetings-modal
detail view; mobile doesn't. The wire field may already exist (verify in
`packages/contract`); if so, mobile just needs to add the rendering.

### `error { code, ...payload }` event

For single-active-meeting rejection (above) and any other intent that fails
server-side validation. Specifically:

```ts
{ type: "error", code: "active_meeting_exists", active_meeting_id: string }
```

Surfaces should pattern-match on `code` and route to the appropriate UI
treatment (toast vs. modal vs. silent).

---

## Open items (deferred work, by surface)

### Mobile

- 🔍 **Mic capture end-to-end** — Phase D wired the streaming module + WS
  endpoint, but the mic indicator stays flat in practice. Needs a focused
  debug session (~30 min): verify `start()` fires, the WS connects, peak
  feedback propagates.
- ✓ **Chat tab on past meeting detail** — `ChatPanel` extracted from the
  live meeting screen, rendered read-only between Summary and Moments.
- ✓ **Attached artifacts list** — `ArtifactsApi.listForMeeting(id)` calls
  the new server endpoint; renders tappable rows linking to
  `/artifact/[id]`, with branded loading + empty + error states.

### Mac

- ✓ **`overlayAutoShow` setting + memory** — Rule 5, fully wired with a
  Settings toggle in the Account tab → Overlay section.
- ✓ **Start disabled when meeting active on any surface** — `canStartMeeting`
  now requires `currentMeetingId == nil` (was previously only checking
  local audio state, which missed PWA/mobile-initiated meetings).

### PWA

- ✓ **No-audio-source placeholder copy** — refined to spec voice.
- ✓ **Start disabled when meeting active on any surface** — verified
  correct (compose-region visibility gated on `meetingState !== "idle"`).

### Server

- ✓ **`GET /meetings/:id/artifacts`** — list endpoint live, returns
  `{ artifacts: ArtifactDto[] }`, 404 on unknown / not-owned meeting.
- ✓ **Duplicate `start_meeting` is a no-op + state rebroadcast** —
  `handle_start_meeting` early-returns when not Idle; the originating
  session also receives a `MeetingStateChanged { Active }` re-echo so
  its UI auto-routes into the live meeting view.
- ✓ **Broadcast scope** — single `tokio::sync::broadcast` channel
  filtered by `user_id` on each WS receiver, so every event reaches all
  of a user's sessions including the originator.
- ✓ **`MeetingDetail.items_by_mode["chat"]`** — chat mode is declared
  `Append` strategy and chat items persist to the database with
  `mode="chat"`; `list_items_for_meeting_grouped` returns them in the
  detail response. Lock-in test added.

### Cross-surface

- 🚫 **iOS moment screenshots** — platform-blocked. Documented above.
- 🚫 **iOS / Android system-audio capture** — platform-blocked.
- 🚫 **Mobile mic as remote source** — by design (Rule 3).

---

## Symbol legend

- ✓ implemented and in production
- ⏳ not yet implemented; tracked
- 🔍 likely correct but unverified — needs a confirmation pass
- 🚫 blocked by platform constraint — not implementable

---

## Maintenance

When you add a feature that affects two or more surfaces, update this
document **before** writing code. When you ship a row above, change its
status flag. When you discover a new constraint, add it to _Platform
constraints_. If this document and the code disagree, fix the code OR fix
the document — never leave them in conflict.
