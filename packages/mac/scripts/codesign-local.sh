#!/usr/bin/env bash
# Sign a downloaded MeetingCompanion.app with your local Developer ID
# certificate so macOS Gatekeeper stops nagging on every launch.
#
# Usage:
#   ./packages/mac/scripts/codesign-local.sh ~/Downloads/MeetingCompanion.app
#
# Prerequisites:
#   - A "Developer ID Application: <your name>" cert in your login keychain.
#     Generate via developer.apple.com → Certificates, IDs & Profiles →
#     Developer ID Application. Download → double-click to add to Keychain.
#
# This is signed-but-not-notarized: Gatekeeper accepts the bundle but
# the *first* launch still requires right-click → Open (or `xattr -dr
# com.apple.quarantine`). Subsequent launches are silent. Notarization
# (zero prompts even on first launch) requires xcrun notarytool +
# Apple ID — not part of this script.

set -euo pipefail

APP_PATH="${1:-}"
if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  echo "usage: $0 <path-to-MeetingCompanion.app>" >&2
  exit 1
fi

# Auto-detect the cert by name. If you have multiple Developer ID certs
# (e.g. one expired, one current), pass `IDENTITY=<sha1>` instead:
#   IDENTITY=ABCD1234... ./codesign-local.sh /path/to/Foo.app
IDENTITY="${IDENTITY:-$(security find-identity -v -p codesigning \
  | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"

if [[ -z "$IDENTITY" ]]; then
  echo "ERROR: No 'Developer ID Application' cert found in the keychain." >&2
  echo "Open Keychain Access and verify the cert is in the 'login' keychain," >&2
  echo "or pass IDENTITY=<sha1> as an env var." >&2
  exit 1
fi

echo "Identity: $IDENTITY"
echo "Bundle:   $APP_PATH"

# `--deep` recurses into nested frameworks (Sparkle.framework). The
# Sparkle.framework is itself signed by the Sparkle project, but
# re-signing keeps the chain consistent so Gatekeeper sees a single
# author for the whole bundle.
codesign \
  --force \
  --deep \
  --sign "$IDENTITY" \
  --options runtime \
  --timestamp \
  "$APP_PATH"

echo
echo "Signed. Verifying:"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

# Strip the quarantine xattr so the very first launch doesn't trigger
# Gatekeeper's "downloaded from internet" warning. Subsequent launches
# don't need this.
xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

echo
echo "Done. Drag $(basename "$APP_PATH") into /Applications and launch."
