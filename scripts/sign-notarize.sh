#!/usr/bin/env bash
# Build, sign, notarize and staple the universal macOS app, then zip it.
#
# Run it yourself: it uses your login keychain, and macOS may ask once to allow
# codesign or notarytool to use it. It takes names only. No key, password or
# certificate file is passed to it, read by it or written by it.
#
#   AL_SIGNING_IDENTITY  The Developer ID Application certificate's display name,
#                        as `security find-identity -v -p codesigning` prints it,
#                        for example "Developer ID Application: Your Name (TEAMID)".
#   AL_NOTARY_PROFILE    The notarytool keychain profile name you saved with
#                        `xcrun notarytool store-credentials` (default: AL_NOTARY).
#   AL_OUT_DIR           Where the zip goes (default: dist-signed/ in this repo, ignored by git).
#
# Usage: scripts/sign-notarize.sh [--skip-build]
#   --skip-build  Sign the app already built at the universal bundle path.

set -euo pipefail
# With pipefail, `cmd | grep -q` can fail on a match: grep exits at the first one and
# cmd dies writing the rest. So output is kept in a variable and grep reads it from
# a here-string.

die() { echo "sign-notarize: $*" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

skip_build=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) die "unknown option: $arg" ;;
  esac
done

identity="${AL_SIGNING_IDENTITY:-}"
profile="${AL_NOTARY_PROFILE:-AL_NOTARY}"
out_dir="${AL_OUT_DIR:-$root/dist-signed}"

[ -n "$identity" ] || die "set AL_SIGNING_IDENTITY to the certificate's display name (a name, not a key)."
case "$identity" in
  "Developer ID Application: "*) ;;
  *) die "AL_SIGNING_IDENTITY must be a \"Developer ID Application: ...\" name; other certificates are not accepted by notarization." ;;
esac
[ "$(uname -s)" = "Darwin" ] || die "this runs on macOS only."
xcrun notarytool --version >/dev/null 2>&1 || die "xcrun notarytool is missing; install Xcode 13 or later command line tools."
xcrun stapler help >/dev/null 2>&1 || die "xcrun stapler is missing."
if [ "$skip_build" -eq 0 ]; then
  # The universal build compiles for both chips, so both Rust targets must be there.
  command -v rustup >/dev/null 2>&1 || die "rustup is missing; the universal build needs it for the Rust targets."
  installed="$(rustup target list --installed)"
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    grep -qx "$target" <<<"$installed" \
      || die "the universal build needs the $target Rust target; run: rustup target add $target"
  done
fi
# The zip is named after the commit, so it must be what the commit holds.
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  [ -z "$(git status --porcelain --untracked-files=no)" ] || die "the working tree has uncommitted changes; the zip would not match $(git rev-parse --short HEAD)."
fi
# Only checks that the named identity is there; nothing from the keychain is printed.
identities="$(security find-identity -v -p codesigning)"
grep -qF "\"$identity\"" <<<"$identities" \
  || die "no valid signing identity named \"$identity\" in your keychain."

# Tauri notarizes on its own when Apple account variables are set. This script
# does that step itself, with the keychain profile, so they are cleared.
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH

bundle_dir="$root/src-tauri/target/universal-apple-darwin/release/bundle/macos"
if [ "$skip_build" -eq 0 ]; then
  echo "== Building the universal app, signed as the named identity"
  APPLE_SIGNING_IDENTITY="$identity" npm run build:universal -- --bundles app
fi

app="$(find "$bundle_dir" -maxdepth 1 -name '*.app' -print -quit 2>/dev/null || true)"
[ -n "$app" ] && [ -d "$app" ] || die "no .app in $bundle_dir; build first or drop --skip-build."
name="$(basename "$app" .app)"
version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
sha="$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"
slug="$(echo "$name" | tr ' ' '-')"

echo "== Checking the signature and the hardened runtime"
codesign --verify --deep --strict --verbose=2 "$app"
details="$(codesign -d --verbose=2 "$app" 2>&1)"
grep -q 'flags=.*runtime' <<<"$details" || die "the app is not signed with the hardened runtime."
grep -qF "Authority=$identity" <<<"$details" || die "the app is not signed by \"$identity\"."

mkdir -p "$out_dir"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
upload="$work/$slug-upload.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$upload"

echo "== Submitting to Apple's notary service and waiting (profile: $profile)"
log="$work/notary.txt"
# A failed submit exits non-zero; carry on so its log is fetched below.
xcrun notarytool submit "$upload" --keychain-profile "$profile" --wait | tee "$log" || true
if ! grep -q 'status: Accepted' "$log"; then
  id="$(awk '/^ *id:/ {print $2; exit}' "$log")"
  [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$profile" || true
  die "notarization was not accepted; the log above says why."
fi

echo "== Stapling the ticket"
xcrun stapler staple "$app"
xcrun stapler validate "$app"

echo "== Gatekeeper check"
assess="$(spctl -a -vv "$app" 2>&1 || true)"
echo "$assess"
grep -q 'source=Notarized Developer ID' <<<"$assess" || die "Gatekeeper does not report a notarized Developer ID app."

zip="$out_dir/$slug-$version-universal-$sha.zip"
rm -f "$zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
echo "== Done: $zip"
shasum -a 256 "$zip"
