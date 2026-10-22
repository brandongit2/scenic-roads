#!/bin/zsh
# Builds Scenic's icon, <out>/Scenic.icns, from icon.svg (128 px and up) and icon-small.svg (16 and
# 32 px, @1x and @2x: the road alone, wider, legible in Finder's lists): every size macOS asks for,
# 16 to 1024 px, rendered by render.swift and packed by iconutil. tools/status/bundle.sh runs it.
# With --preview, it also writes <out>/icon-preview-1024.png and icon-preview-32.png.
#
#   tools/status/icon/build.sh <out> [--preview]
set -euo pipefail
usage="usage: tools/status/icon/build.sh <out> [--preview]"
case $#:${1:-}:${2:-} in
  1:-h:|1:--help:) print -r -- $usage; exit 0 ;;
  1:[^-]*:|2:[^-]*:--preview) ;;
  *) print -ru2 -- "build.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
src=${0:A:h}
out=$1
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
swiftc -O -target arm64-apple-macos26.0 -o $tmp/render $src/render.swift
set=$tmp/Scenic.iconset
mkdir $set
for pt in 16 32 128 256 512; do
  svg=$src/icon.svg
  (( pt <= 32 )) && svg=$src/icon-small.svg
  $tmp/render $svg $set/icon_${pt}x${pt}.png $pt
  $tmp/render $svg $set/icon_${pt}x${pt}@2x.png $(( pt * 2 ))
done
iconutil -c icns -o $out/Scenic.icns $set
if [[ ${2:-} == --preview ]]; then
  cp $set/icon_512x512@2x.png $out/icon-preview-1024.png
  cp $set/icon_32x32.png $out/icon-preview-32.png
fi
