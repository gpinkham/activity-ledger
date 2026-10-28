#!/bin/bash
# Build Activity Ledger as one universal app, sign it with a Developer ID, have
# Apple notarize it, staple the ticket, and zip it for download.
#
# Run it yourself, on the Mac whose keychain holds the certificate and the
# notarization profile. Nothing secret is passed to it or written by it: it uses
# the identity's display name and the keychain profile's name only. macOS may ask
# once to allow keychain access.
#
#   APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)" scripts/sign-notarize.sh
#
# One-time setup:
#   - the Developer ID Application certificate in the login keychain
#     (security find-identity -v -p codesigning prints its name);
#   - xcrun notarytool store-credentials "AL_NOTARY" --apple-id <id> --team-id <TEAMID>
#     (or set AL_NOTARY_PROFILE to the profile name you used).
# Optional: AL_OUT_DIR (default ./dist-signed).
set -euo pipefail

: "${APPLE_SIGNING_IDENTITY:?Set APPLE_SIGNING_IDENTITY to the Developer ID Application identity name (security find-identity -v -p codesigning).}"
PROFILE="${AL_NOTARY_PROFILE:-AL_NOTARY}"
cd "$(dirname "$0")/.."
OUT="${AL_OUT_DIR:-$PWD/dist-signed}"
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
SHA=$(git rev-parse --short HEAD)
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Activity Ledger.app"
ZIP="$OUT/Activity-Ledger-$VERSION-$SHA.zip"

step() { printf '\n== %s\n' "$*"; }

step "Checks"
xcrun notarytool --version >/dev/null || { echo "notarytool is missing: install Xcode 13 or later command line tools."; exit 1; }
if ! security find-identity -v -p codesigning | grep -qF "\"$APPLE_SIGNING_IDENTITY\""; then
  echo "No valid signing identity named \"$APPLE_SIGNING_IDENTITY\" in the keychain."
  exit 1
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "The working tree has uncommitted changes; the zip would not match $SHA."
  exit 1
fi

step "Build and sign (hardened runtime)"
# Tauri signs every binary in the bundle with APPLE_SIGNING_IDENTITY and the
# hardened runtime (tauri.conf.json). It skips notarization here: no Apple ID
# variables are set, and the keychain profile below does that step.
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
npx tauri build --target universal-apple-darwin --bundles app

step "Verify the signature"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -dvv "$APP" 2>&1 | grep -E "^Authority=Developer ID Application|^TeamIdentifier|flags=.*runtime" \
  || { echo "The app is not signed with a Developer ID and the hardened runtime."; exit 1; }

step "Notarize"
mkdir -p "$OUT"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
RESULT=$(mktemp)
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait --output-format json | tee "$RESULT"
STATUS=$(plutil -extract status raw -o - "$RESULT" 2>/dev/null || echo unknown)
ID=$(plutil -extract id raw -o - "$RESULT" 2>/dev/null || echo unknown)
rm -f "$RESULT"
if [ "$STATUS" != "Accepted" ]; then
  echo "Notarization status: $STATUS. Apple's log: xcrun notarytool log $ID --keychain-profile $PROFILE"
  exit 1
fi

step "Staple and zip again"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

step "Gatekeeper"
spctl -a -vv "$APP" 2>&1 | tee /dev/stderr | grep -q "source=Notarized Developer ID" \
  || { echo "Gatekeeper does not report a notarized Developer ID app."; exit 1; }

step "Done"
shasum -a 256 "$ZIP"
echo "$ZIP"
