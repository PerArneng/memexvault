#!/usr/bin/env bash
# Render icon.svg to icon-<SIZE>x<SIZE>.png (default 64). Usage: ./render.sh [SIZE]
# Uses the first available renderer: rsvg-convert, inkscape, ImageMagick,
# or falls back to resvg via `uv` (resvg-py) (no system packages needed).
set -euo pipefail

cd "$(dirname "$0")"
SRC=icon.svg
SIZE="${1:-64}"
OUT="icon-${SIZE}x${SIZE}.png"

if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w "$SIZE" -h "$SIZE" "$SRC" -o "$OUT"
elif command -v inkscape >/dev/null 2>&1; then
  inkscape "$SRC" --export-type=png --export-filename="$OUT" -w "$SIZE" -h "$SIZE"
elif command -v magick >/dev/null 2>&1; then
  magick -background none -density 384 "$SRC" -resize "${SIZE}x${SIZE}" "$OUT"
elif command -v uv >/dev/null 2>&1; then
  uv run --quiet --no-project --with resvg-py python - "$SRC" "$OUT" "$SIZE" <<'PY'
import sys, resvg_py
src, out, size = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = resvg_py.svg_to_bytes(svg_path=src, width=size, height=size)
open(out, "wb").write(bytes(data))
PY
else
  echo "No SVG renderer found (install librsvg2-bin, inkscape, imagemagick, or uv)" >&2
  exit 1
fi

echo "Wrote $(pwd)/$OUT"
