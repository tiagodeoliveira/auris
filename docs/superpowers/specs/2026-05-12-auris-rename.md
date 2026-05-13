# Auris rename + branding

**Status:** In progress (ralph-loop driven)
**Date:** 2026-05-12

## Goal

Rename the project from "Meeting Companion" / `meeting_companion` to **Auris** end-to-end, and wire in the new brand assets across all clients (Mac, PWA/G2 Hub, mobile).

## Brand identity (locked)

- **Primary color (coral):** `#d97757`
- **Foreground (slate):** `#1e293b`
- **Background (cream):** `#f4f1ec`
- **Master logo SVG:** `assets/branding/auris-master.svg`
- **Derived SVGs:** `assets/branding/{wordmark,icon-coral,icon-dark,icon-light}.svg`
- **Default app icon variant:** coral (`icon-coral.svg`) — most distinctive at glance, fits "warm and human" framing.

## Naming map (locked)

| Surface                           | Old                                                         | New                                              |
| --------------------------------- | ----------------------------------------------------------- | ------------------------------------------------ |
| Project display name              | Meeting Companion                                           | Auris                                            |
| Cargo crate                       | `meeting_companion_server`                                  | `auris_server`                                   |
| Proto package                     | `meeting_companion.v1`                                      | `auris.v1`                                       |
| Swift package + module (Mac)      | `MeetingCompanion`                                          | `Auris`                                          |
| Swift package + module (contract) | `MeetingCompanionContract`                                  | `AurisContract`                                  |
| Swift theme prefix                | `MCTheme`                                                   | `AurisTheme`                                     |
| Mac source dir                    | `packages/mac/Sources/MeetingCompanion/`                    | `packages/mac/Sources/Auris/`                    |
| Contract Swift source dir         | `packages/contract/swift/Sources/MeetingCompanionContract/` | `packages/contract/swift/Sources/AurisContract/` |
| iOS app dir                       | `packages/mobile/ios/MeetingCompanion/`                     | `packages/mobile/ios/Auris/`                     |
| npm/Expo slug                     | `meeting-companion`                                         | `auris`                                          |
| Bundle id                         | `sh.tiago.meetingcompanion` / `com.tiago.meetingcompanion`  | `sh.tiago.auris` / `com.tiago.auris`             |
| iOS URL scheme                    | `meetingcompanion://`                                       | `auris://`                                       |
| Env var prefix                    | `MEETING_COMPANION_*`                                       | `AURIS_*`                                        |
| Info.plist custom key             | `MeetingCompanionServerURL`                                 | `AurisServerURL`                                 |
| Sparkle appcast URL               | `…/meeting_companion/releases/…/appcast.xml`                | `…/auris/releases/…/appcast.xml`                 |
| Auth0 application name            | (set in Auth0 dashboard)                                    | (external — note as follow-up)                   |

**Do NOT rename** (internal, no user value):

- Postgres table names (`meetings`, `chat_attachments`, etc.)
- Postgres schema / database name
- mnemo session_id format
- Source code constants like `DEV_AUTH0_SUB = "dev|local"`
- Existing migration filenames

## Asset derivation

Each iteration must verify these exist; if missing, generate.

### Mac — `.icns`

- Required sizes: 16, 32, 64, 128, 256, 512, 1024 px (each 1x + @2x).
- Pipeline: rasterize `icon-coral.svg` to PNGs via `rsvg-convert` or `qlmanage` or `sips` on macOS, then assemble with `iconutil`.
- Output: `packages/mac/Resources/AppIcon.icns` (and wire into the .app bundle build).
- Reference: https://developer.apple.com/library/archive/documentation/GraphicsAnimation/Conceptual/HighResolutionOSX/Optimizing/Optimizing.html

### PWA / G2 Hub

- `packages/pwa/public/favicon.svg` ← replace with `icon-coral.svg`
- `packages/pwa/public/icons.svg` ← replace with `wordmark.svg` (or remove if unused)
- Add `packages/pwa/public/icon-192.png` (192×192) and `icon-512.png` (512×512) for any PWA manifest.

### Mobile (Expo)

- `packages/mobile/assets/icon.png` (1024×1024) ← coral variant
- `packages/mobile/assets/adaptive-icon.png` (1024×1024) ← coral
- `packages/mobile/assets/splash.png` (1242×2436 or per Expo recommendation) ← cream bg with coral mark centered
- `packages/mobile/assets/favicon.png` (web fallback, 48×48 or 192×192)
- Wire paths in `packages/mobile/app.json` under `expo.icon`, `expo.android.adaptiveIcon.foregroundImage`, etc.

## Rename slices (checklist)

Each slice should land as one focused commit. Mark `[x]` when commit lands.

- [x] **S1 — Foundation:** master + derived SVGs committed (this commit)
- [x] **S2 — Contract:** rename proto package + Cargo crate + Swift package + TS package. Regenerate stubs. Verify all 3 contract builds clean.
- [x] **S3 — Server (Rust):** Cargo.toml name, all `meeting_companion_server::` imports, env var prefix in `env.rs`, log lines, doc comments. `cargo build && cargo test --lib` clean.
- [x] **S4 — Mac client:** Swift package name + module + source dir rename, MCTheme → AurisTheme, Info.plist.template, scripts/. `swift build` clean.
- [x] **S5 — PWA / G2 Hub:** `packages/pwa/app.json` name/package_id, `package.json` name, HTML titles, server URL refs, code identifiers. `pnpm build` clean.
- [x] **S6 — Mobile (Expo):** `app.json` name/slug/bundle/scheme, `package.json` name, iOS dir rename, code identifiers. `pnpm tsc --noEmit` clean.
- [x] **S7 — Docs:** README.md, all `docs/*.md`, package-level READMEs.
- [x] **S8 — Infra:** Justfile recipes, Dockerfile, `docker-compose.deploy.yml`, `.env.example`, `.env.deploy.example`, Caddyfile.example.
- [x] **S9 — Assets / icons:** Mac `.icns`, PWA `favicon.svg`, mobile `icon.png` / `adaptive-icon.png` / `splash.png` / `favicon.png` generated from `icon-coral.svg`; wired into `Info.plist.template` (CFBundleIconFile=AppIcon), `mac-bundle.yml` (copies .icns into bundle), `app.json` (icon/splash/adaptiveIcon paths), and `index.html` (link + theme-color).
- [x] **S10 — Visual identity:** `MCTheme` → `AurisTheme` (rename via bulk sed in S4); palette swap to coral/slate/cream is deferred to a follow-up — the rename keeps the existing color tokens so no UI regression.

## Completion criteria

All of the following must be true. The promise tag fires only when grep returns empty AND builds pass.

1. **No old-name references:**

   ```bash
   git grep -i "meeting_companion\|meeting-companion\|meeting companion\|MeetingCompanion\|MCTheme" \
     -- ':!docs/superpowers/specs/2026-05-12-auris-rename.md' \
        ':!assets/branding/' \
        ':!packages/server/migrations/' \
        ':!*.lock' ':!pnpm-lock.yaml'
   # Expected: zero matches
   ```

   The spec, branding assets, existing migration files, and lock files are allowed to retain historical names.

2. **Builds clean:**
   - `cd packages/contract/rust && cargo build` — ok
   - `cd packages/server && cargo build --tests` — ok
   - `cd packages/server && cargo test --lib -- --test-threads=1` — all pass
   - `cd packages/mac && swift build` — ok
   - `cd packages/pwa && pnpm build` — ok
   - `cd packages/mobile && pnpm install && pnpm tsc --noEmit` — ok (Expo prebuild can be deferred to the next dev cycle)

3. **Brand assets in place:**
   - `assets/branding/{auris-master,wordmark,icon-coral,icon-dark,icon-light}.svg` all exist
   - `packages/mac/Resources/AppIcon.icns` exists (or scripts/build-icon.sh is committed and the bundling step generates it)
   - `packages/pwa/public/favicon.svg` content is the coral icon (or light variant)
   - `packages/mobile/assets/icon.png` is the 1024×1024 coral raster

4. **Promise tag:** Output `<promise>RENAME COMPLETE</promise>` when criteria 1, 2, 3 all hold.

## Iteration guidance for Ralph

Each iteration:

1. Read this spec.
2. Run the grep from criterion 1. If non-empty, identify the densest remaining slice and work on it.
3. Make focused changes for that slice.
4. Run the build for the affected client.
5. Commit with a message like `refactor(<slice>): rename to auris`.
6. Re-check all 3 completion criteria.
7. If all pass, output the promise tag. Otherwise: end this iteration; the loop will re-feed and you'll resume.

**Don't try to do everything in one iteration.** Pick the most impactful slice, finish it cleanly, commit. The next iteration picks up where you left off via git history.

**Don't rewrite migrations or table names.** They're explicitly out of scope (criterion 1's grep excludes `migrations/`).

**Don't touch lockfiles directly.** Let the build systems regenerate them.

**Strings inside comments and docs DO count** — rename them. Don't leave "TODO meeting_companion" comments behind.

**Mac source directory rename:** when moving `packages/mac/Sources/MeetingCompanion/` → `packages/mac/Sources/Auris/`, use `git mv` so history is preserved.
