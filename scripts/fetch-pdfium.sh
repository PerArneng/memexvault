#!/bin/sh
# Fetches the pinned, statically linkable pdfium into vendor/pdfium, as spec 002 chose. .cargo/config.toml points the build at it.
set -eu
cd "$(dirname "$0")/.."
TAG=8046d
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) FILE=macos.zip; SHA=1f718f8aeee279c8021de5d61ea9a604613fbfa2d83d79172d9086d711778554 ;;
  *) echo "no pinned pdfium for $(uname -s)-$(uname -m) yet" >&2; exit 2 ;;
esac
if [ -f vendor/pdfium/release/lib/libpdfium.a ]; then echo "pdfium is already here"; exit 0; fi
mkdir -p vendor/pdfium
curl -fsSL -o vendor/pdfium/$FILE "https://github.com/paulocoutinhox/pdfium-lib/releases/download/$TAG/$FILE"
echo "$SHA  vendor/pdfium/$FILE" | shasum -a 256 -c -
(cd vendor/pdfium && unzip -q $FILE && rm $FILE)
echo "pdfium $TAG is in vendor/pdfium"
