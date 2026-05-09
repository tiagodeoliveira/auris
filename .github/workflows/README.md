# CI Workflows

Four workflows. Three publish to a different shipping surface; one
ships JS-only updates over-the-air to already-built mobile binaries.

| File                | Surface         | Output                                                          |
| ------------------- | --------------- | --------------------------------------------------------------- |
| `server-image.yml`  | Rust server     | Docker image at `ghcr.io/<owner>/meeting-companion-server`      |
| `mac-bundle.yml`    | SwiftUI Mac app | Unsigned `.app` zip as workflow artifact + Release asset on tag |
| `mobile-ios.yml`    | Expo iOS binary | EAS Build (cloud) — artifact in EAS dashboard                   |
| `mobile-update.yml` | Expo JS bundle  | EAS Update — pushed to deployed binaries on next launch         |

## When to rebuild vs. update (mobile)

- **Rebuild (`mobile-ios.yml`)** — needed when native code or native
  config changes: new native module, `app.json` permissions /
  plugins, `eas.json` profiles, anything under `ios/` or `android/`.
  ~25 minutes; new binary lands in TestFlight / EAS dashboard.
- **Update (`mobile-update.yml`)** — for JS-only changes: TS, JSX,
  styles, asset references that don't add new native deps. ~30
  seconds; phones running a matching build see the new code on next
  launch.

## Required secrets / variables

Configure under repo **Settings → Secrets and variables → Actions**.

### `EXPO_TOKEN` (secret) — for `mobile-ios.yml`

Generate locally with `eas account:tokens:create` after
`eas-cli login`. Paste the token. Without it, `mobile-ios.yml` fails
fast at the guard step with a clear error.

### `GITHUB_TOKEN` (built-in)

Auto-provided by Actions; no setup needed. Used by `server-image.yml`
to push to GHCR. The package's visibility (private vs public)
inherits from the repo on first push — flip it under **Packages**
once it exists.

### Optional: Apple Developer credentials — for signed Mac builds

Today `mac-bundle.yml` produces an **unsigned** `.app`. macOS
Gatekeeper warns on first launch; users right-click → Open to
dismiss. To switch to a signed + notarized build (no warning,
auto-update friendly), add:

- `APPLE_DEVELOPER_ID_CERT_BASE64` — base64-encoded `.p12` of the
  Developer ID Application cert.
- `APPLE_DEVELOPER_ID_CERT_PASSWORD` — password for the `.p12`.
- `APPLE_API_KEY_BASE64` — base64-encoded App Store Connect API key
  `.p8` for notarization.
- `APPLE_API_KEY_ID`, `APPLE_API_KEY_ISSUER_ID`.

Then extend `mac-bundle.yml` with `codesign --deep --sign` and
`xcrun notarytool submit`. Out of scope for v1; the bundle is
trivially distributable to the developer's own machines without it.

### Optional: real-device iOS builds

The default `preview-simulator` profile in `eas.json` produces a
simulator-arch `.app` that runs on the iOS Simulator without any
Apple Developer enrollment. Real-device builds (`preview` or
`production` profiles) need:

- An Apple Developer Program membership.
- Apple credentials linked to the EAS project — easiest path is
  `eas credentials` interactively, which stores them in EAS's
  credential vault and the CI uses them automatically.

## First-time setup checklist

1. **GHCR**
   - First push of `server-image.yml` creates the package as public.
     Flip to **private** under **Packages → meeting-companion-server
     → Package settings**.

2. **EAS / mobile-ios.yml**
   - `pnpm dlx eas-cli login` (one-time, locally).
   - `cd packages/mobile && pnpm dlx eas-cli init` — registers the
     project on Expo's side and writes `extra.eas.projectId` into
     `app.json`. Commit that change.
   - `eas account:tokens:create --name "github-actions"` → copy
     token → add as repo secret `EXPO_TOKEN`.

3. **EAS Update / mobile-update.yml** (after step 2)
   - `cd packages/mobile && pnpm dlx eas-cli update:configure` — adds
     the `updates.url` and a `runtimeVersion` policy to `app.json`.
     Commit those changes.
   - Build the app once for the channel you want to update
     (`eas build --profile preview --platform ios`) and install on
     the device. The binary's embedded channel ("preview") tells
     it which update branch to listen to.
   - From now on: pushes to main run `mobile-update.yml`, which
     publishes to the `production` branch. Builds tracking that
     channel apply on next launch — no App Store round-trip.

### Channel ↔ branch mapping

`eas.json` profiles wire each build to a channel; updates publish
to a _branch_ of the same name by convention:

| Profile             | Channel       | Branch        | Notes                 |
| ------------------- | ------------- | ------------- | --------------------- |
| `development`       | `development` | `development` | dev-client builds     |
| `preview-simulator` | `preview`     | `preview`     | iOS Simulator         |
| `preview`           | `preview`     | `preview`     | TestFlight / internal |
| `production`        | `production`  | `production`  | App Store builds      |

`mobile-update.yml` defaults to the `production` branch on push to
main. Use `workflow_dispatch` to publish to `preview` or
`development` for testing OTA changes before promoting to prod.

3. **Mac bundle**
   - No setup needed for unsigned builds. Push to main; the artifact
     appears under **Actions → Mac bundle → <run> → Artifacts**.

## Path filters

Each workflow only runs when relevant files change. Workflow file
itself is in the path filter so editing the workflow re-triggers it.

- `server-image.yml`: `packages/server/**`, `Cargo.{toml,lock}`,
  `Dockerfile`, this workflow file.
- `mac-bundle.yml`: `packages/mac/**`, this workflow file.
- `mobile-ios.yml`: `packages/mobile/**`, this workflow file.
- `mobile-update.yml`: `packages/mobile/**` excluding native config
  paths (`app.json`, `eas.json`, `ios/`, `android/`) — those need a
  rebuild, not an OTA. Workflow file is in scope so editing the
  workflow re-triggers it.

A `v*` tag triggers all three. Releases bundle artifacts from each
surface under the same tag.

## Deploying the server

See `docker-compose.deploy.yml` and `.env.deploy.example` at the
repo root. The update flow on the host VM is:

```sh
# Copy the example once and fill it in:
cp .env.deploy.example .env.deploy
# (edit .env.deploy)

# Pull the latest server image (tag-pinned via SERVER_TAG in env):
docker compose -f docker-compose.deploy.yml --env-file .env.deploy pull server

# Bring up / restart:
docker compose -f docker-compose.deploy.yml --env-file .env.deploy up -d
```

Front it with a reverse proxy for TLS — the server's `:7331` carries
auth tokens as a WS query parameter, so plain HTTP exposure is a
credential-leak risk.
