#!/usr/bin/env bash
#
# Sign the macOS bundle and prove the signature.
#
# Tauri's own ad-hoc signing leaves the bundle unable to pass
# `codesign --verify --deep --strict`:
#
#   AirCard.app: code has no resources but signature indicates they must be present
#
# Gatekeeper reports that to users as a damaged app, so the bundle is re-signed
# here, which rewrites CodeResources. The previous implementation did the same in
# its build script, for the same reason.
#
# Set AIR_CARD_IDENTITY to a Developer ID to sign for real; without it the bundle
# stays ad-hoc, which is enough to run locally.

set -euo pipefail

APP="${1:-target/release/bundle/macos/AirCard.app}"
IDENTITY="${AIR_CARD_IDENTITY:--}"

if [ ! -d "$APP" ]; then
    echo "no bundle at $APP -- run 'npm run tauri build' first" >&2
    exit 1
fi

if [ "$IDENTITY" = "-" ]; then
    codesign --force --deep --sign - "$APP"
else
    codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
fi

# Every Mach-O inside the bundle has to verify, not only the outer signature:
# a nested binary signed ad-hoc is what makes a downloaded app refuse to launch.
find "$APP/Contents" -type f -perm -u+x -exec sh -c '
    file "$1" | grep -q "Mach-O" || exit 0
    codesign --verify --strict "$1" || { echo "unsigned nested binary: $1" >&2; exit 1; }
' _ {} \;

codesign --verify --deep --strict --verbose=2 "$APP"
echo "signature verified: $APP"
