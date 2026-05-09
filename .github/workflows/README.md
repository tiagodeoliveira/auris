# CI Workflows

Three publish workflows, one for each shipping surface. All triggered
by push-to-main, push of a `v*` tag, and `workflow_dispatch`.

| File               | Surface         | Output                                                          |
| ------------------ | --------------- | --------------------------------------------------------------- |
| `server-image.yml` | Rust server     | Docker image at `ghcr.io/<owner>/meeting-companion-server`      |
| `mac-bundle.yml`   | SwiftUI Mac app | Unsigned `.app` zip as workflow artifact + Release asset on tag |
| `mobile-ios.yml`   | Expo iOS app    | EAS Build (cloud) — artifact in EAS dashboard                   |

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
