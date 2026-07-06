#!/usr/bin/env bash
# Build, bundle, sign, notarize, and staple a release-ready Auris.app
# from a local checkout. Output: a notarized .app + a distributable
# zip alongside it.
#
# Mirrors the CI flow in .github/workflows/mac-bundle.yml but uses the
# local keychain identity and notary keychain profile instead of CI
# secrets. Useful for hand-distributing builds outside CI or for
# verifying signing/entitlements changes before pushing a tag.
#
# Prerequisites (one-time setup):
#   1. "Developer ID Application: <name> (TEAMID)" cert in your login
#      keychain — generated via developer.apple.com → Certificates →
#      Developer ID Application, then double-clicked to import.
#   2. Stored notary credentials under a named keychain profile:
#        xcrun notarytool store-credentials AURIS_NOTARY_PROFILE \
#          --apple-id <email> --team-id <TEAMID> --password <app-spec-pw>
#      App-specific password: appleid.apple.com → Sign-In and Security.
#
# Optional env overrides:
#   VERSION=...           Marketing version baked into Info.plist
#                         (default: 0.1.0-<short sha>)
#   BUILD=...             CFBundleVersion (default: epoch seconds)
#   IDENTITY=...          Override cert auto-detection (full name or sha)
#   NOTARY_PROFILE=...    Keychain profile name (default: AURIS_NOTARY_PROFILE)
#   SKIP_NOTARIZE=1       Sign-only — faster iteration loop, but the
#                         first launch will still need right-click → Open
#   AURIS_SERVER_URL, AUTH0_DOMAIN, AUTH0_MAC_CLIENT_ID,
#   AUTH0_API_AUDIENCE, SPARKLE_PUBLIC_KEY
#                         Baked into Info.plist via envsubst. Unset
#                         values fall through to the app's coded
#                         defaults (dev tenant + localhost server).

set -euo pipefail

cd "$(dirname "$0")/.."  # packages/mac

VERSION="${VERSION:-0.1.0-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}"
BUILD="${BUILD:-$(date +%s)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-AURIS_NOTARY_PROFILE}"

IDENTITY="${IDENTITY:-$(security find-identity -v -p codesigning \
  | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
if [[ -z "$IDENTITY" ]]; then
  echo "ERROR: No 'Developer ID Application' cert in login keychain." >&2
  echo "Generate one via developer.apple.com and import the .cer." >&2
  exit 1
fi

echo "==> Identity: $IDENTITY"
echo "==> Version:  $VERSION (build $BUILD)"
echo

echo "==> Building universal binary (arm64 + x86_64)..."
swift build -c release --arch arm64 --arch x86_64

BUNDLE="Auris.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources" "$BUNDLE/Contents/lib"
cp .build/apple/Products/Release/Auris "$BUNDLE/Contents/MacOS/"

echo "==> Writing Info.plist..."
export VERSION BUILD
export AURIS_SERVER_URL="${AURIS_SERVER_URL:-}"
export AUTH0_DOMAIN="${AUTH0_DOMAIN:-}"
export AUTH0_MAC_CLIENT_ID="${AUTH0_MAC_CLIENT_ID:-}"
export AUTH0_API_AUDIENCE="${AUTH0_API_AUDIENCE:-}"
export SPARKLE_PUBLIC_KEY="${SPARKLE_PUBLIC_KEY:-}"
envsubst < Info.plist.template > "$BUNDLE/Contents/Info.plist"

cp Resources/AppIcon.icns "$BUNDLE/Contents/Resources/"

SPARKLE_FW=$(find .build -path "*/Sparkle.framework" -type d -maxdepth 8 2>/dev/null | head -1)
if [[ -z "$SPARKLE_FW" ]]; then
  echo "ERROR: Sparkle.framework not found under .build/" >&2
  exit 1
fi
echo "==> Copying Sparkle.framework from $SPARKLE_FW"
ditto "$SPARKLE_FW" "$BUNDLE/Contents/lib/Sparkle.framework"

# Sign Sparkle's nested helpers in innermost-out order. --deep is
# unreliable for frameworks with XPC services + embedded .app
# bundles (the runtime helper "Updater.app"); Sparkle's official
# docs require explicit per-binary signing.
#
# --preserve-metadata=entitlements is only used on the XPC services,
# which carry Sparkle-specific entitlements baked into the framework
# (e.g. mach-lookup, jit, allow-unsigned-executable-memory). The
# other binaries don't need entitlements preserved.
#
# Order: innermost helpers -> Updater.app shell -> XPC services ->
# Autoupdate executable -> framework outer signature. The main
# bundle is signed AFTER this block so its CodeResources seal
# references the just-completed framework signature.
echo "==> Signing Sparkle.framework helpers..."
SPARKLE="$BUNDLE/Contents/lib/Sparkle.framework"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$SPARKLE/Versions/B/Updater.app/Contents/MacOS/Updater"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$SPARKLE/Versions/B/Updater.app"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --preserve-metadata=entitlements \
  "$SPARKLE/Versions/B/XPCServices/Downloader.xpc"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --preserve-metadata=entitlements \
  "$SPARKLE/Versions/B/XPCServices/Installer.xpc"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$SPARKLE/Versions/B/Autoupdate"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$SPARKLE"

echo "==> Signing main bundle with Auris.entitlements..."
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --entitlements Auris.entitlements \
  "$BUNDLE"

echo "==> Verifying signature..."
codesign --verify --deep --strict --verbose=2 "$BUNDLE"

ZIP="Auris-${VERSION}.zip"
echo "==> Zipping for notarization..."
ditto -c -k --keepParent "$BUNDLE" "$ZIP"

if [[ "${SKIP_NOTARIZE:-}" == "1" ]]; then
  echo
  echo "✓ Signed (skipped notarization)."
  echo "  Bundle: $BUNDLE"
  echo "  Zip:    $ZIP"
  echo "  First launch will need right-click → Open."
  exit 0
fi

echo "==> Submitting to notarytool — typically 1–10 min, occasionally longer..."
xcrun notarytool submit "$ZIP" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait

echo "==> Stapling notarization ticket..."
xcrun stapler staple "$BUNDLE"
xcrun stapler validate "$BUNDLE"

# Re-zip after stapling so the distributable carries the ticket inline.
# Without this, Gatekeeper's first-launch check has to hit Apple's
# server for the ticket (requires the user to be online).
rm -f "$ZIP"
ditto -c -k --keepParent "$BUNDLE" "$ZIP"

echo
echo "✓ Signed + notarized + stapled."
echo "  Bundle: $BUNDLE"
echo "  Zip:    $ZIP"
echo "  Ready to install — no Gatekeeper prompts."
