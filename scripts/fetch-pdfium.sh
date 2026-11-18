#!/bin/sh
# Fetches the pinned pdfium into vendor/pdfium: statically linkable on macOS, as spec 002 chose, and libpdfium.so on Linux x86_64, as spec 091 chose. .cargo/config.toml points the build at it.
set -eu
cd "$(dirname "$0")/.."
TAG=8046d
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) FILE=macos.zip; SHA=1f718f8aeee279c8021de5d61ea9a604613fbfa2d83d79172d9086d711778554 ;;
  Linux-x86_64) FILE=linux.zip; SHA=d80d50b3a5d1d132ba2fbd6083918b504c6371d3f1e91236d1628c1cb06439a3 ;;
  *) echo "no pinned pdfium for $(uname -s)-$(uname -m) yet" >&2; exit 2 ;;
esac
if [ -f vendor/pdfium/release/lib/libpdfium.a ] || [ -f vendor/pdfium/release/lib/libpdfium.so ]; then echo "pdfium is already here"; exit 0; fi
mkdir -p vendor/pdfium
curl -fsSL -o vendor/pdfium/$FILE "https://github.com/paulocoutinhox/pdfium-lib/releases/download/$TAG/$FILE"
echo "$SHA  vendor/pdfium/$FILE" | shasum -a 256 -c -
(cd vendor/pdfium && unzip -q $FILE && rm $FILE)
# The Linux archive keeps one library per architecture. Put ours where the macOS archive puts its own.
if [ -f vendor/pdfium/release/lib/x64/libpdfium.so ]; then
  mv vendor/pdfium/release/lib/x64/libpdfium.so vendor/pdfium/release/lib/
  rm -r vendor/pdfium/release/lib/x64 vendor/pdfium/release/lib/arm64
fi
echo "pdfium $TAG is in vendor/pdfium"
