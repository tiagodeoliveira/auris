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

## Status — Phase 2a complete (scaffold only)

| Sub-phase | Goal                                                         | Status     |
| --------- | ------------------------------------------------------------ | ---------- |
| **2a**    | Mac scaffold: SwiftPM + menu bar item + AppModel placeholder | **✓ Done** |
| **2b**    | Server-side device registry + control channel                | Pending    |
| **2c**    | Settings window + token-based server connection              | Pending    |
| **2d**    | Permissions onboarding (Microphone + Screen Recording)       | Pending    |
| **2e**    | Audio capture in Swift (SCKit) + mixer parity with Rust      | Pending    |
| **2f**    | Mac → server: register as device, stream PCM via `/audio`    | Pending    |
| **2g**    | Compose window (description input + Start Meeting flow)      | Pending    |
| **2h**    | Native Meetings browse window (depends on Phase 4 APIs)      | Deferred   |

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
  MeetingCompanionApp.swift        ✓ 2a
  MenuBarContent.swift             ✓ 2a
  AppModel.swift                   ✓ 2a
  Settings/
    SettingsWindow.swift            2b: Account / General / Permissions tabs
  Net/
    WebSocketClient.swift           2c: WS to server, JWT/token auth
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

## Next: Phase 2b

Two parallel tracks open up after 2a:

- **Server-side**: implement device registry endpoints + control channel
  (`DeviceCommand` enum) on the existing Rust server.
- **Mac-side**: Settings window + WebSocket client to connect to the
  server (using the dev token for now; OAuth lands in Phase 3).

Either can land first. The Mac `WebSocketClient` is independently
testable against the server's existing WS endpoint (which already
accepts the dev token).
