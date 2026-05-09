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
The workflows reference repo-level entries only — no environments
needed (single deployment target per surface).

### Quick-reference table

`secret` = masked in logs, accessed via `${{ secrets.NAME }}`.
`variable` = visible in logs, accessed via `${{ vars.NAME }}`. Auth0
client IDs / domains / audiences are _embedded in the public binary
anyway_, so they live as variables rather than secrets — secrecy is
meaningless once shipped.

| Name                           | Kind     | Used by             | Purpose                                                                         |
| ------------------------------ | -------- | ------------------- | ------------------------------------------------------------------------------- |
| `GITHUB_TOKEN`                 | built-in | server-image.yml    | GHCR push auth. No setup needed.                                                |
| `EXPO_TOKEN`                   | secret   | mobile-{ios,update} | EAS Build + EAS Update access token. Generate with `eas account:tokens:create`. |
| `SPARKLE_PRIVATE_KEY`          | secret   | mac-bundle.yml      | EdDSA private key signing OTA update bundles.                                   |
| `SPARKLE_PUBLIC_KEY`           | variable | mac-bundle.yml      | Matching public key, embedded in Info.plist via envsubst.                       |
| `MEETING_COMPANION_SERVER_URL` | variable | mac + mobile        | WebSocket endpoint the bundled apps connect to (`wss://...`).                   |
| `AUTH0_DOMAIN`                 | variable | mac + mobile        | Auth0 tenant host (e.g., `dev-xyz.us.auth0.com`).                               |
| `AUTH0_API_AUDIENCE`           | variable | mac + mobile        | Auth0 API identifier (e.g., `https://meeting-companion.api`).                   |
| `AUTH0_MAC_CLIENT_ID`          | variable | mac-bundle.yml      | Client ID of the **Native** Auth0 application registered for the Mac.           |
| `AUTH0_MOBILE_CLIENT_ID`       | variable | mobile-{ios,update} | Client ID of the **Native** Auth0 application registered for mobile.            |

Missing values are not fatal: bundles ship with empty strings for
those keys, and the apps fall back to compiled-in defaults
(localhost server, dev Auth0 tenant). That keeps `swift run` /
`expo start` development zero-configuration while letting CI swap
in environment-specific values without code changes.

### `EXPO_TOKEN` (secret) — for `mobile-ios.yml`

Generate locally with `eas account:tokens:create` after
`eas-cli login`. Paste the token. Without it, `mobile-ios.yml` fails
fast at the guard step with a clear error.

### `GITHUB_TOKEN` (built-in)

Auto-provided by Actions; no setup needed. Used by `server-image.yml`
to push to GHCR. The package's visibility (private vs public)
inherits from the repo on first push — flip it under **Packages**
once it exists.

### `SPARKLE_PRIVATE_KEY` (secret) + `SPARKLE_PUBLIC_KEY` (variable) — for `mac-bundle.yml`

Sparkle drives the Mac auto-update flow. Each tagged release is
signed with an EdDSA private key in CI and verified by the bundled
public key on every user's machine. Without the keys, the app ships
with an empty `SUPublicEDKey` and Sparkle fail-closes — no updates
will install.

One-time generation (locally):

```sh
SPARKLE_VERSION=2.6.4
curl -L -o /tmp/sparkle.tar.xz \
  "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
mkdir -p /tmp/sparkle && tar -xf /tmp/sparkle.tar.xz -C /tmp/sparkle

# 1. Create the EdDSA key pair. Bare `generate_keys` (no flags)
#    generates a new pair and stores the private key in the macOS
#    Keychain. Running it again later just prints "Existing signing
#    key found" — it doesn't overwrite. The `-p` / `-x` flags below
#    only work after this initial step has been taken.
/tmp/sparkle/bin/generate_keys

# 2. Export both halves. `-p` prints the public key, `-x <file>`
#    writes the private key to a file (decrypts from the Keychain
#    on the way out — your user password may prompt).
/tmp/sparkle/bin/generate_keys -p > sparkle_pub.txt
/tmp/sparkle/bin/generate_keys -x sparkle_priv.pem
```

Then in **Settings → Secrets and variables → Actions**:

- Add `SPARKLE_PRIVATE_KEY` as a **secret** (paste the contents of
  `sparkle_priv.pem`).
- Add `SPARKLE_PUBLIC_KEY` as a **variable** (paste the contents of
  `sparkle_pub.txt`, single line of base64).

Delete `sparkle_priv.pem` from your machine after pasting. Keep a
backup somewhere safe (1Password, etc.) — losing the private key
means existing installed bundles can never auto-update again, and
all users have to manually download a freshly-keyed build.

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

4. **Mac bundle**
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
