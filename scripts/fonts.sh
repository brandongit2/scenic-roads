#!/usr/bin/env bash
# Download the glyph ranges MapLibre needs for labels (Latin, French accents, punctuation,
# Inuktitut syllabics) for three Noto Sans styles. Skips files already present.
#   scripts/fonts.sh [<folder>]           download them (default data/fonts)
#   scripts/fonts.sh --check [<folder>]   whether they're all there
set -euo pipefail
# (--check: download nothing, and fail naming how many are missing: publish.sh's check before its build.)
check=
if [ "${1:-}" = --check ]; then check=1; shift; fi
dest="${1:-data/fonts}"
base="https://raw.githubusercontent.com/protomaps/basemaps-assets/main/fonts"
fonts=("Noto Sans Regular" "Noto Sans Medium" "Noto Sans Italic")
# The ranges not in $dest, one "<font>|<range>" a line.
missing() {
  for f in "${fonts[@]}"; do
    for s in $(seq 0 256 8448) 5120 5376 5632 65024 65280; do
      r="$s-$((s + 255))"
      [ -s "$dest/$f/$r.pbf" ] && continue
      echo "$f|$r"
    done
  done
}
if [ -n "$check" ]; then
  n=$(missing | sort -u | wc -l | tr -d ' ')
  [ "$n" = 0 ] || { echo "fonts: $n glyph ranges missing in $dest (make them: scripts/fonts.sh $dest)" >&2; exit 1; }
  exit 0
fi
for f in "${fonts[@]}"; do mkdir -p "$dest/$f"; done
missing | sort -u | xargs -P 16 -I{} sh -c 'f="${1%%|*}"; r="${1##*|}"; u=$(printf "%s" "$f" | sed "s/ /%20/g"); curl -sf -o "'"$dest"'/$f/$r.pbf" "'"$base"'/$u/$r.pbf" || rm -f "'"$dest"'/$f/$r.pbf"' _ {}
echo "fonts: $(find "$dest" -name '*.pbf' | wc -l | tr -d ' ') glyph ranges"
