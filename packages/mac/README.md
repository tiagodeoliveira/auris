# Meeting Companion — Mac app

Native macOS menu-bar app that captures audio for meetings, streams it
to the server over WebSocket, and exposes meeting lifecycle controls.
Designed to be **independently useful**: a user with no PWA paired can
still run a meeting from the Mac alone (per ADR-0009 / standalone-first
principle in [`docs/PLAN.md`](../../docs/PLAN.md)).

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Roadmap: [`docs/PLAN.md`](../../docs/PLAN.md) §4 Phase 2.

## What this app is

- **Menu-bar accessory app** (`LSUIElement`-equivalent via
  `setActivationPolicy(.accessory)`). No Dock icon, no app-switcher
  entry. Lifetime is tied to the menu bar item.
- **Audio capture source** for the server (the system audio + mic
  combined, mixed into a 16 kHz mono S16LE PCM stream).
- **Standalone meeting controller** when no PWA is paired (compose,
  start, stop from the Mac alone).
- **Floating overlay** during meetings (Phase 6+) for live items + action
  buttons.
- **Native browse window** for past meetings (Phase 2h, depends on
  Phase 4's REST APIs).

What it is _not_:

- A web view embed. Pure SwiftUI + AppKit.
- A standalone meeting backend. The server still owns state, STT,
  summarizers, mnemo. The Mac is the audio source + a control surface.

## Status — Phase 2c complete (Mac talks to the server)

| Sub-phase | Goal                                                           | Status             |
| --------- | -------------------------------------------------------------- | ------------------ |
| **2a**    | Mac scaffold: SwiftPM + menu bar item + AppModel placeholder   | **✓ Done**         |
| **2b**    | Server-side device registry + control channel                  | **✓ Done**         |
| **2c**    | Settings window + token-based server connection                | **✓ Done**         |
| **2f₁**   | Mac registers as a device on connect; ownDevice + capabilities | **✓ Done**         |
| **2d**    | Permissions onboarding (Microphone + Screen Recording)         | **✓ Done**         |
| **2e**    | Audio capture in Swift (SCKit) + mixer parity with Rust        | **✓ Done**         |
| **2f₂**   | Stream PCM via `/audio` (depends on 2e)                        | Pending (after 2e) |
| **2g**    | Compose window (description input + Start Meeting flow)        | Pending            |
| **2h**    | Native Meetings browse window (depends on Phase 4 APIs)        | Deferred           |

Acceptance for Phase 2 overall: three demos pass — Mac standalone,
PWA-led with Mac as source, Mac standalone with browse (where 2h is
deferred to Phase 4).

## Build & run

Requires Xcode 15+ (Swift 5.9, macOS 14 SDK).

### From the command line

```bash
cd packages/mac
swift build       # debug binary
swift run         # build + launch
```

The menu bar icon appears (a hollow circle in the scaffold state)
top-right. Click → dropdown with status + stubbed actions. Click
"Quit Meeting Companion" or `⌘Q` to exit.

### From Xcode

```bash
open packages/mac/Package.swift
```

Xcode opens the SwiftPM package as if it were a project. Run with
`⌘R`. Set breakpoints, debug, the usual.

### From the repo root via `just`

```bash
just mac-build    # swift build
just mac-run      # swift run (launches the app)
```

## Architecture

Single executable target. Files under `Sources/MeetingCompanion/`:

| File                        | Role                                                                                                                                                            |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `MeetingCompanionApp.swift` | `@main` entry. `MenuBarExtra` scene. Sets accessory activation policy.                                                                                          |
| `MenuBarContent.swift`      | The dropdown view. Most actions are stubbed (disabled) — each Phase 2 sub-phase wires more of them.                                                             |
| `AppModel.swift`            | `@Observable` source of truth for app-wide state. Connection state today; capabilities, bound meeting state, captured-frame counters added by later sub-phases. |

### Sub-phases will add (rough sketch)

```
Sources/MeetingCompanion/
  MeetingCompanionApp.swift        ✓ 2a (extended in 2c with Settings window scene)
  MenuBarContent.swift             ✓ 2a (extended in 2c with Connect/Disconnect/Settings)
  AppModel.swift                   ✓ 2a (extended in 2c to own settings + ws)
  Settings/
    AppSettings.swift              ✓ 2c: UserDefaults-backed settings
    SettingsView.swift             ✓ 2c: server URL + token form
  Net/
    WebSocketClient.swift          ✓ 2c: URLSessionWebSocketTask wrapper
    DeviceRegistration.swift        2f: POST /devices, capability advertisement
  Permissions/
    PermissionsOnboarding.swift     2d: first-launch UX
    PermissionMonitor.swift         2d: live status of mic + screen perms
  Audio/
    AudioCapture.swift              2e: SCKit setup
    AudioMixer.swift                2e: 50fps mixer (parity with Rust)
    AudioStreamer.swift             2f: send PCM frames to /audio WS
  Compose/
    ComposeWindow.swift             2g: description + Extract Tags + Start
  Meetings/
    MeetingsWindow.swift            2h: native master/detail (deferred to Phase 4)
```

## Conventions

- **No Storyboards / NIBs.** SwiftUI for views; AppKit only where SwiftUI
  doesn't reach (custom panels, floating windows in Phase 6).
- **`@Observable` on the model**, not `ObservableObject`. We target
  macOS 14+; modern Observation is preferred.
- **State flows down, intent flows up**. Views read from `AppModel`,
  call methods on it. The model owns network/audio side effects.
- **One concept per file.** Adding a new feature is a new file,
  not a new section in an existing file.
- **Comments explain _why_, not _what_.** The menu-bar-accessory
  comment in `MeetingCompanionApp.swift` is a good template.

## Smoke test (through Phase 2f₁)

Two terminals:

```bash
# Terminal 1 — start the server:
just server-run

# Terminal 2 — launch the Mac app:
just mac-run
```

First run only: click the menu bar icon → "Open Settings to sign
in…" → enter `ws://localhost:7331` + token `dev` → close.

Click the menu bar icon → **Connect** → the status line should
quickly progress through:

```
Not signed in
   ↓ (Connect)
Connecting…
   ↓
Connected · registering…
   ↓ (server replies device_registered)
Connected · registered as <hostname>
```

A `Device id: <8-char-prefix>…` line appears below the status,
confirming the server-assigned UUID. **Disconnect** clears it.

You can verify the registry from a third terminal with `websocat`:

```bash
websocat 'ws://localhost:7331/?token=dev'
# Look at the snapshot's `devices` array — should include the Mac.
```

When the Mac disconnects, the websocat session sees a
`devices_changed` broadcast with the entry gone.

## Next: Phase 2d → 2e → 2f₂

The unblocked sub-phases now run in dependency order:

- **2d**: Permissions onboarding (Microphone + Screen Recording).
  Required before 2e can produce frames.
- **2e**: SCKit audio capture in Swift. Mirror of the existing Rust
  pipeline. Produces 16 kHz mono S16LE PCM into an
  `AsyncStream<Data>` or `AsyncChannel<Data>`.
- **2f₂**: Stream those PCM frames to the server's `/audio`
  endpoint via a second WebSocket connection. Once this lands, the
  server's `RemoteAudioSource` (Phase 1b) consumes them and the
  full Mac-as-audio-source path is wired end-to-end.
