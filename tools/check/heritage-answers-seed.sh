#!/bin/zsh
# The heritage chain's answers for a pass as one archive in the shape the app keeps them
# (pipeline::answers::heritage: `sources/items/<date>/heritage.tar.zst`, tar with zstd's checksum),
# made from the registers' snapshot the chain worked in before the registers became an input
# (`sources/registers/legacy`: its Wikidata, Wikipedia and World Heritage caches, wd/, desc/ and
# whs/) and the pass's answers kept beside it (`sources/items/<date>/heritage-<id>.tar.zst`), laid
# over it. Reads the NAS; writes only <out> (docs/inputs.md §6.5, Migration: the PM copies it to
# the NAS before the app that reads it is published).
#
#   heritage-answers-seed.sh <nas project folder> <date> <work folder> <out .tar.zst>
set -euo pipefail
nas=$1 date=$2 work=$3 out=$4
[[ $out != $nas/* && $work != $nas/* ]] || { echo "write outside the NAS project folder" >&2; exit 2; }
snap=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sources/registers/legacy"])' "$nas/state/build/manifest.json")
answers=(${(f)"$(ls "$nas/sources/items/$date"/heritage-*.tar.zst 2>/dev/null)"})
(( ${#answers} <= 1 )) || { echo "more than one heritage answers archive for $date: ${answers[*]}" >&2; exit 2; }
rm -rf "$work"; mkdir -p "$work/copy"
# The snapshot's caches (only those: the registers themselves are inputs now).
zstd -dcq "$nas/$snap" | tar -xf - -C "$work/copy" wd desc whs
# The pass's answers over them (what the chain fetched since the snapshot), the caches alone.
if (( ${#answers} == 1 )); then
  mkdir -p "$work/answers"
  zstd -dcq "${answers[1]}" | tar -xf - -C "$work/answers"
  for d in wd desc whs; do
    [[ -d $work/answers/$d ]] && cp -Rp "$work/answers/$d/." "$work/copy/$d/"
  done
  # (areas-wikidata.json: areadetails.py's park facts, which the chain keeps at the top.)
  [[ -f $work/answers/areas-wikidata.json ]] && cp -p "$work/answers/areas-wikidata.json" "$work/copy/"
fi
# What the chain makes again each run (pipeline::answers REMADE), and temporary files: not answers.
rm -f "$work/copy"/whs/{tagged.osm.pbf,ids.txt,whs.osm.pbf,whs.geojsonseq,relations.opl} "$work/copy/wd/items.jsonl"
find "$work/copy" \( -name '.*' -o -name '*.tmp' -o -name '*.part' \) -prune -exec rm -rf {} +
(cd "$work/copy" && find . -type f | sed 's|^\./||' | LC_ALL=C sort | tr '\n' '\0' > ../list)
COPYFILE_DISABLE=1 tar --null -cf - -C "$work/copy" -T "$work/list" | zstd -10 --check -q -f -o "$out"
echo "$(tr '\0' '\n' < "$work/list" | wc -l | tr -d ' ') files, $(du -h "$out" | cut -f1) → $out"
