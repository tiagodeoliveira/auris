#!/usr/bin/env bash
# Attach one or more artifacts to the shared `nightly` GitHub release.
#
# Called by mac-bundle.yml and pwa-ehpk.yml on every push to main.
# Both workflows race against the same release: the script is
# idempotent and tolerates concurrent invocation.
#
# Race-safety strategy:
#   1. Force-move the `nightly` git tag to $COMMIT. Both workflows on
#      the same commit produce the same tag value, so a race is a
#      no-op; on a slightly later commit the later push wins, which
#      matches the user-visible "tag tracks main" property.
#   2. Idempotent release create. If the release already exists,
#      `gh release view` succeeds and we skip `release create`. If
#      two workflows both observe absence and both attempt create,
#      one wins and the other's `release create` errors out — which
#      the `|| true` swallows; the subsequent `release upload` lands
#      against whichever release exists.
#   3. Per-file `release upload --clobber` — overwrites only the
#      named assets, never touches sibling-workflow assets. So a
#      Mac upload won't blow away a PWA asset and vice versa.
#
# Required env (set by caller):
#   GH_TOKEN  GitHub token with `contents: write`. Workflows already
#             set this via `secrets.GITHUB_TOKEN`.
#   COMMIT    The commit SHA the release should point at.
#   REPO      `owner/repo` slug, used to build documentation URLs in
#             the release body.
#
# Arguments: one or more paths (relative to the caller's CWD) to
# attach to the release. Filenames are preserved as-is on the
# release page, so callers are responsible for naming (e.g.
# `Auris-nightly.zip` for the stable URL, `Auris-{version}.zip` for
# the pinned copy).

set -euo pipefail

: "${GH_TOKEN:?GH_TOKEN is required}"
: "${COMMIT:?COMMIT is required}"
: "${REPO:?REPO is required}"

if [[ $# -lt 1 ]]; then
  echo "usage: attach-nightly.sh <artifact> [<artifact>...]" >&2
  exit 1
fi

# Force-move the nightly tag to this commit so the release page
# reflects the most recent main push. `git push -f` is required —
# the tag is intentionally mutable on this rolling release.
git tag -f nightly "$COMMIT"
git push origin -f "refs/tags/nightly"

body=$(cat <<EOF
Auto-generated nightly build from the latest main push.

Stable download URLs (always the newest):

- Mac (\`.zip\`, signed + notarized):
  \`\`\`
  https://github.com/${REPO}/releases/download/nightly/Auris-nightly.zip
  \`\`\`
- PWA (\`.ehpk\` for EvenHub glasses):
  \`\`\`
  https://github.com/${REPO}/releases/download/nightly/auris-pwa-nightly.ehpk
  \`\`\`

The version-pinned copies of each artifact ride along on the same
release for users who want to lock to a specific build.

Sparkle-managed auto-updates (Mac) and TestFlight builds (iOS) come
from real \`vX.Y.Z\` tags, not this one. Mobile build artifacts
(\`.apk\` / \`.ipa\`) live on the EAS dashboard, not in this release.
EOF
)

# Idempotent release-ensure. The `|| true` covers the race where a
# sibling workflow won `release create` between our `view` and
# `create` calls — the subsequent `release edit` still lands
# against whichever release exists.
if ! gh release view nightly --json tagName >/dev/null 2>&1; then
  gh release create nightly \
    --title "Nightly build (main)" \
    --notes "$body" \
    --prerelease \
    --target "$COMMIT" \
    || true
fi

# Refresh title + body each run so an existing release left over
# from before the unified-nightly migration (Mac-only or PWA-only
# body) gets rewritten to the new schema. Race-safe under
# concurrent invocation: both workflows emit identical content.
gh release edit nightly \
  --title "Nightly build (main)" \
  --notes "$body" \
  --prerelease

# One-time migration: drop the legacy `pwa-nightly` release. Safe
# to keep in perpetuity — the `|| true` covers the steady-state
# case where the release no longer exists.
gh release delete pwa-nightly --yes --cleanup-tag 2>/dev/null || true

# Upload our artifacts. --clobber overwrites only the named files,
# so this run can't accidentally erase a sibling workflow's asset.
gh release upload nightly "$@" --clobber

# Prune any assets that aren't rolling `*-nightly.*` files. Keeps
# the release at a fixed asset count regardless of how many builds
# have run; self-heals the legacy version-pinned copies (e.g.
# `Auris-0.1.0-abc1234.zip`) left over from before the rolling-only
# policy. Race-safe: sibling workflows never upload non-`*-nightly.*`
# assets, so the per-name match here can only target stale files.
gh release view nightly --json assets --jq '.assets[].name' \
  | while IFS= read -r asset; do
      case "$asset" in
        *-nightly.*) ;;  # rolling artifact, keep
        *) gh release delete-asset nightly "$asset" --yes 2>/dev/null || true ;;
      esac
    done
