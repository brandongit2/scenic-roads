#!/bin/sh
# Two versions of the renderer, A B A B, with the gated time.mjs (whole frames on − off) on three
# views; web/src/bldfield.ts swapped between runs (vite reloads it) and restored however it ends:
#   sh tools/bldcheck/abcode.sh <commit A> <out dir>   (B: the working tree's bldfield.ts; run from the repo)
case "$#:${1:-}" in
  1:-h|1:--help) echo "usage: abcode.sh <commit A> <out dir>"; exit 0 ;;
  2:-*) echo "usage: abcode.sh <commit A> <out dir>" >&2; exit 2 ;;
  2:*) ;;
  *) echo "usage: abcode.sh <commit A> <out dir>" >&2; exit 2 ;;
esac
A=$1; S=$2
[ -n "$A" ] && [ -n "$S" ] || { echo "usage: abcode.sh <commit A> <out dir>" >&2; exit 2; }
mkdir -p "$S" || exit 1
cp web/src/bldfield.ts "$S/bldfield-cur.ts" || exit 1
trap 'cp "$S/bldfield-cur.ts" web/src/bldfield.ts' EXIT INT TERM
git show "$A:web/src/bldfield.ts" > "$S/bldfield-a.ts" || exit 1
python3 -c "
import json, sys; v=json.load(open('tools/bldcheck/views/views-T.json')); json.dump([x for x in v if x['id'] in ('t-manhattan-p70','t-kanto-p70','t-tokyo-z11')], open(sys.argv[1],'w'))" "$S/views-AB.json"
for round in 1 2; do
  for v in a cur; do
    cp "$S/bldfield-$v.ts" web/src/bldfield.ts
    sleep 4
    echo "### $v round $round"
    BLD_MAX_LOAD=5 node tools/bldcheck/time.mjs "$S/views-AB.json" "$S/ab-$v-$round.json" --frames 300 > "$S/ab-$v-$round.log" 2>&1
    grep -E "load|busy" "$S/ab-$v-$round.log" | head -6
    python3 tools/bldcheck/tsum.py "$S/ab-$v-$round.json"
  done
done
