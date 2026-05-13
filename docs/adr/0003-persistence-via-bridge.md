# ADR-0003: Persistence via `bridge.setLocalStorage`, not browser `localStorage`

**Status:** Accepted
**Date:** 2026-05-02
**Context for:** PWA spec (`docs/specs/pwa.md`), augments [`ARCHITECTURE.md` §4 Phone PWA](../ARCHITECTURE.md#4-component--phone-pwa).

## Context

The PWA needs to persist a small set of values between app restarts:

- Server WebSocket URL (e.g. `ws://laptop.local:7331` or `wss://meeting.tiago.tail-scale.ts.net:7331`).
- Server WebSocket auth token (the same shared secret the server reads from `AURIS_TOKEN`).
- Soniox API key (or whichever STT provider's credential the PWA uses).
- Optionally: last-used metadata KV pairs to pre-fill the next meeting's editor.

The architecture doc doesn't specify where these live. The natural first instinct is browser `localStorage`. The Even Hub `device-features` skill states unambiguously:

> Browser IndexedDB and browser `localStorage` do NOT reliably persist across app restarts in this environment — data saved there can be lost when the user closes and reopens the app.

The cause is the host environment: the Even Realities App embeds a Flutter WebView, and the host's storage isolation rules can wipe browser-layer storage between sessions. The Even Hub SDK exposes a separate persistent storage surface owned by the Even App itself.

## Decision

All persistent PWA-side state goes through the bridge:

- `await bridge.setLocalStorage(key, value)` — writes a string; returns boolean for success.
- `await bridge.getLocalStorage(key)` — reads a string; returns empty string if absent.

The PWA wraps these in a thin typed module (`packages/pwa/src/storage.ts`) that:

- Namespaces all keys with a `mc.` prefix to avoid collisions with other Even Hub apps that share the same host storage.
- JSON-encodes/decodes non-string values transparently.
- Provides typed accessors for the known keys (`mc.serverUrl`, `mc.serverToken`, `mc.sonioxKey`, `mc.lastMetadata`).
- Surfaces a single async `loadSettings()` at startup that reads all keys in parallel and returns a `Settings` object.

For developer ergonomics, Vite `import.meta.env.VITE_*` variables seed the values on first run if storage is empty:

- `VITE_DEFAULT_SERVER_URL`
- `VITE_DEFAULT_SERVER_TOKEN`
- `VITE_DEFAULT_SONIOX_KEY`

These are read once at boot, only used as fallbacks when bridge storage returns empty, and never overwrite values the user has explicitly set via the settings page.

The `.env.local` file holding these values is git-ignored (already covered by the existing `.gitignore` `.env.local` rule from Task 1 of the stub-server plan).

## Consequences

**Positive:**

- Settings actually survive app restarts on real glasses. Without this ADR, every restart on hardware would silently wipe the user's config.
- Credentials live in host-managed storage, out of reach of any DOM-XSS-style injection within the WebView.
- The seed-from-env pattern lets the developer avoid retyping creds on every fresh sideload, while the production user flow is "open the app, type your credentials in settings once."
- The same code path works in the simulator (the simulator implements `bridge.setLocalStorage` correctly).

**Negative:**

- `bridge.setLocalStorage` is async, so `loadSettings()` becomes async too. Top-level app boot has to await it before opening the WebSocket. Acceptable — the boot sequence already awaits `waitForEvenAppBridge()`.
- Each setting is a separate bridge round-trip on read. For the small number of keys (≤ 5) this is negligible, but a profligate caller could add latency. The wrapper batches reads at startup to avoid per-key awaits in hot paths.
- If we ever want to migrate to a different host (web preview, a dev tool that runs the PWA in plain Chrome), we'd need to swap the storage backend. We isolate this behind the `storage.ts` module so the swap is one file.

**Accepted risks:**

- The Even Hub host could theoretically rate-limit or wipe `bridge.setLocalStorage` too; the `device-features` skill doesn't promise it's bulletproof, only that it's the most reliable option available. If we hit that, we switch to either a server-side settings store or a host-issued auth token flow.
- We don't currently encrypt the stored Soniox API key. Anyone with physical access to the user's phone + the Even Realities App's storage could read it. For a personal-tool stub this is acceptable; for a public release we'd want OAuth-style token issuance instead of long-lived API keys.

## Alternatives considered

### (a, chosen) `bridge.setLocalStorage` only

See above.

### (b) Browser `localStorage`

Tempting because it's the standard pattern and works in the simulator. Rejected because the Even Hub `device-features` skill explicitly warns it's unreliable on real hardware. We'd discover this only when a user complained that their settings were gone after a phone restart.

### (c) Server-side settings store

Push all PWA settings to the laptop server, which persists them and replays on connect. The server already has a snapshot model that could naturally accommodate this.

Rejected because:

- The PWA needs the server URL and token to _connect to_ the server in the first place — they can't live on the server. So at least those two settings have to live somewhere local.
- Adds a new contract surface (settings sync events) for marginal benefit.
- Couples settings persistence to the laptop being reachable, which is exactly the time when the user might need to fix a setting.

### (d) Hand the user a pre-populated `.ehpk` per device

Build the `.ehpk` with credentials baked in at packaging time, ship a different binary per device. Rejected — operational nightmare, no rotation story, and `.ehpk` files would carry secrets if shared.

## Follow-ups

- PWA spec: define the exact `Settings` type, the `mc.*` key namespace, the seed-from-env fallback rules, and the settings-page UI.
- PWA implementation plan: a Vitest test that mocks `bridge.setLocalStorage` and confirms the seed-from-env pattern only fires when the bridge returns empty.
- Phase 1 hardware: confirm `bridge.setLocalStorage` actually persists across an Even Realities App restart (not just an in-app reload).
