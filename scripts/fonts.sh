#!/usr/bin/env bash
# Download the glyph ranges MapLibre needs for labels (Latin, French accents, punctuation,
# Inuktitut syllabics) for three Noto Sans styles. Skips files already present.
set -euo pipefail
dest="${1:-data/fonts}"
base="https://raw.githubusercontent.com/protomaps/basemaps-assets/main/fonts"
for f in "Noto Sans Regular" "Noto Sans Medium" "Noto Sans Italic"; do
  mkdir -p "$dest/$f"
  for s in $(seq 0 256 8448) 5120 5376 5632 65024 65280; do
    r="$s-$((s + 255))"
    [ -s "$dest/$f/$r.pbf" ] && continue
    echo "$f|$r"
  done
done | xargs -P 16 -I{} sh -c 'f="${1%%|*}"; r="${1##*|}"; u=$(printf "%s" "$f" | sed "s/ /%20/g"); curl -sf -o "'"$dest"'/$f/$r.pbf" "'"$base"'/$u/$r.pbf" || rm -f "'"$dest"'/$f/$r.pbf"' _ {}
echo "fonts: $(find "$dest" -name '*.pbf' | wc -l | tr -d ' ') glyph ranges"
