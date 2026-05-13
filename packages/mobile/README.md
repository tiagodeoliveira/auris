# @auris/mobile

Native iOS + Android client built with Expo + React Native + Expo
Router. Companion to the PWA and the Mac menu-bar app; same server
contract, same Auth0 tenant.

The implementation plan lives in [`docs/MOBILE-PLAN.md`](../../docs/MOBILE-PLAN.md).

## Status

**Phases 0–5 of MOBILE-PLAN shipped:**

- **Phase 0** — skeleton, monorepo wiring, EAS project.
- **Phase 1** — Auth0 PKCE flow with refresh token in
  `expo-secure-store`, identity-aware WS auto-connect.
- **Phase 2** — bare meeting flow (compose, start, live overlay,
  pause/resume/stop, mark moment), wire types ported from PWA,
  `ReconnectingSocket`.
- **Phase 3** — mic permission + peak meter (PCM streaming
  deferred — see below).
- **Phase 4** — item interactions (chevron expand, detail load),
  chat-mode pane, mode tabs.
- **Phase 5** — past-meeting browse: bucketed history, detail view
  (title / metadata / LLM usage / transcript), artifact list.

**Deferred buckets** (still in MOBILE-PLAN):

- **3.3+** — mic PCM streaming to `/stt` and `/audio`. Blocked on
  expo-audio's per-buffer PCM (requires SDK 52+); SDK 51 here. Path
  forward: a config plugin around `react-native-live-audio-stream`,
  or a small dev-client native module bridging AVAudioEngine.
- **4.5+** — camera-attached moments (bottom sheet: camera /
  no-image / app-snapshot, JPEG upload to artifact summarizer,
  long-press shortcut).
- **5.4** — moment image rendering in detail view (auth-aware blob
  fetch via `expo-file-system`, render via `expo-image`).
- **5.7+** — artifact uploads + multi-select attach UI for compose
  - live attach.

## Prerequisites

- Node 20+
- pnpm 9+ (the workspace's package manager)
- Xcode 16+ (iOS Simulator)
- Android Studio (Android Emulator)
- A device with Expo Go for quick dev iteration, OR a dev client
  build via EAS for anything that needs native modules
  (`expo-secure-store` is in Expo Go on SDK 51; PCM streaming will
  push us to a dev client).
- An EAS account if you want OTA updates or installable binaries.

## First run

From the workspace root:

```sh
pnpm install
pnpm --filter @auris/mobile start
```

The dev server prints a QR code; scan with Expo Go on the device.

To open directly on a simulator:

```sh
pnpm --filter @auris/mobile ios
pnpm --filter @auris/mobile android
```

## Configuration

Build-time configuration via `EXPO_PUBLIC_*` env vars (Expo's public
build-time variable convention):

| Variable                      | Purpose                                                               |
| ----------------------------- | --------------------------------------------------------------------- |
| `EXPO_PUBLIC_SERVER_URL`      | WS endpoint, e.g. `ws://jarvis.tail48cb4.ts.net:7331`.                |
| `EXPO_PUBLIC_AUTH0_DOMAIN`    | Auth0 tenant.                                                         |
| `EXPO_PUBLIC_AUTH0_CLIENT_ID` | Auth0 Native client ID (currently shares the Mac's for personal use). |
| `EXPO_PUBLIC_AUTH0_AUDIENCE`  | Auth0 API identifier.                                                 |

For local dev, copy `.env.example` to `.env.local` and fill in the
four values. For EAS Builds, configure these as EAS env vars in the
Expo dashboard (per environment: development / preview / production).
See `.github/workflows/README.md` for the full inventory.

## Distribution

- **EAS Build** (`mobile-build.yml`) — matrixed `[ios, android]`,
  produces installable binaries (preview-simulator IPA for iOS
  Simulator; APK for Android sideload). Trigger via the workflow's
  `workflow_dispatch` button or push a tag.
- **EAS Update** (`mobile-update.yml`) — JS-only OTA updates,
  published to the `production` channel automatically on every
  push to main. Devices with a matching binary version pull the
  new bundle on next launch.

## Layout

```
packages/mobile/
├── app/
│   ├── _layout.tsx              root Stack + auth bootstrap + WS auto-connect
│   ├── login.tsx                Auth0 sign-in
│   ├── meeting.tsx              live-meeting fullscreen modal
│   ├── meeting/[id].tsx         past-meeting detail
│   └── (tabs)/
│       ├── _layout.tsx          tab navigator
│       ├── index.tsx            compose
│       ├── history.tsx          past-meeting list (bucketed)
│       ├── artifacts.tsx        artifact list (read-only)
│       └── settings.tsx         identity + sign-out
├── src/
│   ├── auth/auth0.ts            PKCE + secure-store
│   ├── audio/audio-capture.ts   stub (PCM streaming deferred)
│   ├── lib/meetings.ts          shared display helpers
│   ├── store/index.ts           Zustand store + WS lifecycle
│   ├── wire/contract.ts         hand-synced wire types
│   ├── wire/ws.ts               ReconnectingSocket (ported from PWA)
│   ├── wire/meetings-api.ts     REST client
│   ├── wire/artifacts-api.ts    REST client
│   └── config.ts                EXPO_PUBLIC_* reader
├── app.json                     Expo config (scheme, plugins, bundle ids, projectId)
├── babel.config.js              babel-preset-expo (router plugin auto)
├── metro.config.js              monorepo-aware (pnpm-friendly)
├── tsconfig.json                extends expo/tsconfig.base
├── eas.json                     EAS Build / Update profiles
├── package.json
└── README.md
```

## Monorepo notes

`node-linker=hoisted` is set at the workspace root `.npmrc` so
Expo's module resolver finds transitive deps in a single
`node_modules` tree (rather than pnpm's strict isolation). The
Metro config watches the entire repo. When a shared-ts wire-types
package is eventually extracted, it'll be picked up automatically.

## Where to look next

- [`docs/MOBILE-PLAN.md`](../../docs/MOBILE-PLAN.md) — full
  architecture + phase plan + deferred buckets.
- [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md) — wire protocol
  the client speaks. Same as PWA / Mac.
- [`packages/pwa`](../pwa) — visual + flow reference; the source
  for `wire/contract.ts`.
- [`packages/mac`](../mac) — control-surface + audio-source
  reference for a non-browser native client.
- [`.github/workflows/README.md`](../../.github/workflows/README.md)
  — EAS env vars and the CI pipeline.
