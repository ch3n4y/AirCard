#!/usr/bin/env bash
# Sign the finished .app before placing it in the downloadable archives.
set -euo pipefail

APP="${1:-target/release/bundle/macos/AirCard.app}"
if [[ ! -d "$APP" ]]; then
    echo "App bundle not found: $APP" >&2
    exit 1
fi

APP="$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(cd "$ROOT" && node -p "require('./package.json').version")"
EXECUTABLE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
ARCHS="$(lipo -archs "$APP/Contents/MacOS/$EXECUTABLE")"
if [[ "$ARCHS" == *arm64* && "$ARCHS" == *x86_64* ]]; then
    ARCH="universal"
else
    ARCH="${ARCHS// /-}"
fi
OUT="$(dirname "$(dirname "$APP")")/dmg"
mkdir -p "$OUT"

"$ROOT/scripts/sign-macos.sh" "$APP"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/AirCard.app"
ln -s /Applications "$STAGE/Applications"

DMG="$OUT/AirCard-${VERSION}-${ARCH}.dmg"
ZIP="$OUT/AirCard-${VERSION}-${ARCH}.zip"
hdiutil create -quiet -volname AirCard -srcfolder "$STAGE" \
    -format UDZO -imagekey zlib-level=1 -ov "$DMG"
ditto -c -k --keepParent "$APP" "$ZIP"
hdiutil verify -quiet "$DMG"
echo "Packaged: $DMG"
echo "Packaged: $ZIP"
