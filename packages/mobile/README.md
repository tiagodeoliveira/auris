# @meeting-companion/mobile

Native iOS + Android client built with Expo + React Native + Expo
Router. Companion to the PWA and the Mac menu-bar app; same server
contract, same Auth0 tenant.

The implementation plan lives in [`docs/MOBILE-PLAN.md`](../../docs/MOBILE-PLAN.md).
This README covers the local dev loop only.

## Status

**Phase 0 — skeleton.** Tab bar shape (Compose / History / Artifacts /
Settings) renders; every tab is a placeholder. Auth, transport,
audio, and every interaction lands in Phases 1–6 per the plan.

## Prerequisites

- Node 20+
- pnpm 9+ (the workspace's package manager)
- Xcode 15+ (for iOS Simulator)
- Android Studio (for the Android Emulator)
- An iOS device + Expo Go app, OR a dev build via EAS (Phase 0 of the
  plan suggests EAS; for now Expo Go is fine since we have no native
  modules yet).

## First run

From the workspace root:

```sh
pnpm install
pnpm --filter @meeting-companion/mobile expo install --fix
pnpm --filter @meeting-companion/mobile start
```

`expo install --fix` aligns React, React Native, and the Expo SDK
modules to whatever Expo version is current — the `package.json`
here is pinned to a known-good baseline (SDK 51) but Expo bumps
quickly. Once aligned, the dev menu opens with QR + simulator
shortcuts.

To open directly on a simulator:

```sh
pnpm --filter @meeting-companion/mobile ios
pnpm --filter @meeting-companion/mobile android
```

## Layout

```
packages/mobile/
├── app/
│   ├── _layout.tsx          ← root Stack (auth gate lands in Phase 1)
│   ├── login.tsx
│   └── (tabs)/
│       ├── _layout.tsx      ← Tab navigator
│       ├── index.tsx        ← Compose
│       ├── history.tsx
│       ├── artifacts.tsx
│       └── settings.tsx
├── app.json                 ← Expo config (scheme, plugins, bundle ids)
├── babel.config.js          ← babel-preset-expo (router plugin auto)
├── metro.config.js          ← monorepo-aware (pnpm-friendly)
├── tsconfig.json            ← extends expo/tsconfig.base
├── package.json
└── README.md                ← you are here
```

## Monorepo notes

The Metro config in `metro.config.js` watches the entire repo and
disables hierarchical lookup so pnpm's non-hoisted `node_modules`
trees resolve cleanly. When `packages/shared-ts` is extracted (per
plan §3), it'll be picked up automatically.

## Where to look next

- [`docs/MOBILE-PLAN.md`](../../docs/MOBILE-PLAN.md) — full
  architecture + phase plan.
- [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md) — wire protocol the
  client must speak. Same as PWA / Mac.
- [`packages/pwa`](../pwa) — visual + flow reference.
- [`packages/mac`](../mac) — control-surface + audio-source
  reference for a non-browser native client.
