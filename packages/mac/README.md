# Auris — Mac app

Native macOS menu-bar app. Captures audio (system + mic) and streams
it to the server, hosts a floating overlay during meetings (mode
tabs, items list, mark-moment, dictation, stop), and provides a
native browse window for past meetings. Auto-updates via Sparkle
from GitHub Releases.

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Roadmap: [`docs/PLAN.md`](../../docs/PLAN.md).

## What it does

- **Menu-bar accessory app** (`setActivationPolicy(.accessory)`). No
  Dock icon, no app-switcher entry.
- **Audio source** for active meetings: ScreenCaptureKit system audio
  - microphone, mixed at ~50 fps to 16 kHz mono S16LE PCM, streamed
    over `/audio` WS. Same pipeline as the server's local-capture
    path; the choice is which side of the Tailnet runs the bytes.
- **Standalone meeting controller**: compose, start, pause, resume,
  stop, mark moments, dictate descriptions, browse history — all
  from the Mac alone.
- **Floating overlay** during meetings. Mode tabs (TRANSCRIPT,
  HIGHLIGHTS, ACTIONS, QUESTIONS, SUMMARY, CHAT), items list with
  detail expand, peak meter, dictation mic, mark-moment, stop.
- **Drives the G2 glasses** via the EvenHub bridge when this is the
  active client (alternative to PWA-driven glasses).
- **Sparkle auto-update.** Tagged releases ship a signed `appcast.xml`
  on GitHub Releases; the app polls daily and prompts the user.

## Build & run

Requires Xcode 16+ (Swift 5.9+, macOS 15 SDK — needed for the
`SCStream` microphone path).

### From the command line

```bash
cd packages/mac
swift build       # debug binary
swift run         # build + launch
```

### From Xcode

```bash
open packages/mac/Package.swift
```

Run with `⌘R`. Standard breakpoints / debugger.

### From the repo root via `just`

```bash
just mac-build    # swift build
just mac-run      # swift run (launches the app)
```

### Producing a signed `.app` bundle locally

```bash
packages/mac/scripts/codesign-local.sh   # auto-detects Developer ID identity
```

The script runs `swift build -c release`, packages a universal `.app`
bundle, copies the Sparkle framework into `Contents/lib/`, signs with
`--deep --options runtime --timestamp`, and strips the `com.apple.quarantine`
xattr. The output `.app` lives under `packages/mac/.build/release/`.

## Configuration

Build-time configuration is baked into `Info.plist` via envsubst in
`mac-bundle.yml`. Locally, `Info.plist.template` reads the same env
vars at build time. Variables (with hardcoded defaults in
`Auth0Client.swift` and `AppSettings.swift` for personal-use safety):

| Variable              | Used for                                                       |
| --------------------- | -------------------------------------------------------------- |
| `AURIS_SERVER_URL`    | WS / REST endpoint (e.g. `ws://jarvis.tail48cb4.ts.net:7331`). |
| `AUTH0_DOMAIN`        | Auth0 tenant (e.g. `your-tenant.us.auth0.com`).                |
| `AUTH0_MAC_CLIENT_ID` | Auth0 Native application client ID.                            |
| `AUTH0_API_AUDIENCE`  | Auth0 API identifier the JWT must `aud`-match.                 |
| `SPARKLE_PUBLIC_KEY`  | EdDSA public key Sparkle uses to verify update signatures.     |

Auth0 access + refresh tokens are persisted in the macOS Keychain.
Overlay theme + opacity are persisted in `UserDefaults`.

## Auto-update (Sparkle)

`mac-bundle.yml` runs on each `v*` tag:

1. `swift build -c release` (universal binary).
2. Wraps into a `.app` bundle.
3. Signs the bundle's contents with the Developer ID (when
   `MAC_SIGNING_*` secrets are configured; otherwise builds an
   ad-hoc signed dev artifact).
4. Generates an EdDSA signature with `sign_update` (Sparkle's CLI
   tool, using the `SPARKLE_PRIVATE_KEY` secret).
5. Updates `appcast.xml` with the new version, signature, and
   download URL.
6. Uploads `.app.zip` + `appcast.xml` as a GitHub Release asset.

The shipping app reads `appcast.xml` from the configured
`SUFeedURL`, validates the EdDSA signature against the embedded
`SUPublicEDKey`, downloads the `.app.zip`, and prompts the user.

To generate a new key pair:

```bash
sign_update --generate-keys                # writes to macOS Keychain
sign_update --print-public-key             # prints SPARKLE_PUBLIC_KEY
sign_update --export-secret-key file.pem   # exports SPARKLE_PRIVATE_KEY
```

Set `SPARKLE_PUBLIC_KEY` as a repo variable, `SPARKLE_PRIVATE_KEY`
as a repo secret. See `.github/workflows/README.md` for the
complete secret/variable inventory.

## Architecture

Single executable target. Files under `Sources/Auris/`:

| File                                      | Role                                                                                              |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `AurisApp.swift`                          | `@main` entry. App delegate. Accessory activation policy.                                         |
| `AppModel.swift`                          | `@Observable` source of truth. Mirrors per-user server state (modes, items, devices, transcript). |
| `MenuBarContent.swift`                    | Menu dropdown. Status, Start / Stop / Compose, Meetings…, Settings…, Permissions….                |
| `MeetingOverlayView.swift`                | Floating overlay panel during meetings. Mode tabs, items, peak meter, mark-moment, mic, stop.     |
| `DictationController.swift`               | Mic-only `/stt` flow for the description-dictation path.                                          |
| `Net/WebSocketClient.swift`               | Control-channel WS client. Reconnect, heartbeat, intent dispatch.                                 |
| `Net/Auth0Client.swift`                   | Native PKCE OAuth flow, refresh-token-in-Keychain.                                                |
| `Net/MeetingsAPI.swift`                   | REST client (`GET/DELETE /meetings`, moments, artifacts).                                         |
| `Net/Protocol.swift`                      | Wire types, hand-synced with `packages/server/src/contract.rs`.                                   |
| `Net/SttSession.swift`                    | `/stt` WS client for the dictation mic path.                                                      |
| `Audio/AudioCapture.swift`                | ScreenCaptureKit system audio + mic. SCStream microphone path (macOS 15+).                        |
| `Audio/MicCapture.swift`                  | Mic-only capture for dictation. Realtime audio thread, `@unchecked Sendable`.                     |
| `Audio/AudioStreamer.swift`               | 50 fps mixer + `/audio` WS sender. Drops on backpressure rather than buffering.                   |
| `Capture/ScreenshotCapture.swift`         | On-demand screenshot for moments. Triggered by server's `capture_moment_screenshot`.              |
| `Permissions/PermissionMonitor.swift`     | Live status of mic + screen-recording permissions. Refresh on app activation.                     |
| `Permissions/PermissionsOnboarding.swift` | First-run UX with deep links to System Settings.                                                  |
| `Settings/AppSettings.swift`              | UserDefaults-backed: overlay theme, overlay opacity, server URL display.                          |
| `Settings/SettingsView.swift`             | Settings window: account (signed-in identity + logout), overlay appearance, LLM usage rollup.     |
| `UpdaterController.swift`                 | Sparkle wrapper (`SPUStandardUpdaterController` + `@Published canCheckForUpdates`).               |

## Permissions

Two TCC permissions are required:

1. **Screen Recording** — for ScreenCaptureKit system audio + the
   moment-screenshot path.
2. **Microphone** — for the system+mic mixer and dictation.

The first-run onboarding sheet deep-links to System Settings →
Privacy & Security → Screen Recording / Microphone with an "Open
Settings" button. After granting, **the app must be quit and
relaunched**: macOS caches the screen-recording check at process
launch (`CGPreflightScreenCaptureAccess`), so granting alone
doesn't propagate. The mic permission updates live thanks to the
`PermissionMonitor` observer on `NSApplication.didBecomeActiveNotification`.

## Smoke test

Two terminals (or just the Mac if you're hitting the deployed
server):

```bash
# Terminal 1 — server (skip if pointing at production):
just server-run

# Terminal 2 — launch the Mac app:
just mac-run
```

First launch:

1. Sign in with Auth0 from the menu dropdown.
2. Grant Microphone + Screen Recording in System Settings; relaunch.
3. Click the menu bar icon → **Compose meeting…**.
4. Type a description (or click the mic to dictate). Click **Start**.
5. Floating overlay appears. Speak; verify the transcript items
   appear within ~1–2s of sentence boundaries.
6. After ~30s of speech, switch to `HIGHLIGHTS` / `ACTIONS` /
   `QUESTIONS` tabs — the agent loop populates them.
7. Switch to `SUMMARY` — the rolling summary appears as one card.
8. Switch to `CHAT` — type a question, send. Agent reply lands in
   the same panel.
9. Click the camera icon → moment marked, screenshot taken,
   summary populates async.
10. Click **Stop** to end the meeting.

## Update path for users

```
sparkle-pinned app → polls appcast.xml → user clicks "Install Update"
                                              → downloads .app.zip
                                              → verifies EdDSA signature
                                              → relaunches into new version
```

Users with a Developer-ID-signed bundle get seamless auto-update.
For ad-hoc-signed builds (no `MAC_SIGNING_*` secrets), users must
manually run `xattr -cr` on the new `.app` after Gatekeeper marks
it quarantined.
