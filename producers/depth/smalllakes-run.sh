#!/bin/zsh
# The small lakes' full run (docs/inputs.md §6.10, smalllakes.py), on the M4. Not run by the app.
#
#   producers/depth/smalllakes-run.sh [workers]      (default 3; 4 at most)
#
# - Its own server instance: the clone's release `server` on port $PORT with a --home of its own
#   under $WORK and --root on the NAS's project folder (--no-mirror), never the app's (7311) or its
#   folder. Stopped by its exact PID on exit.
# - The boxes: one a z6 tile of the basemap's coverage (the NAS's base/ sections), each its own
#   smalllakes.py call with its own index ($WORK/index/<z6>.geojson); the workers take every
#   workers-th box of the list, so each holds its own boxes.
# - Output local ($WORK/grids), packed by z6 tile (packs.py); after a box, its packs are tarred over SSH into the NAS's
#   sources/fetched/smalllakes/, their sizes there compared, then deleted here and the box marked
#   done ($WORK/index/<z6>.ok). Re-running skips the boxes done and resumes a box stopped part way
#   (smalllakes.py's <index>.done). At the end the boxes' indexes are merged into
#   smalllakes-grids.geojson, copied beside the grids.
# - Memory: ~0.7 GB a worker (measured on a Massachusetts box) and the server's, under ~4 GB for 3–4.
# - caffeinate holds the Mac awake while it runs; a box starts only on mains power or with the
#   battery at the pool's battery floor or more (`scenic battery-floor`, kept on the NAS; 30 % while
#   none is set: the agent's DEFAULT_BATTERY_MIN), else it waits.
set -euo pipefail
WORKERS=${1:-3}
(( WORKERS >= 1 && WORKERS <= 4 )) || { echo "workers: 1–4" >&2; exit 2; }
REPO=${REPO:-$HOME/repos/scenic-roads}
NASROOT=/Volumes/personal/projects/scenic-roads
HYDROLAKES_NAS=/Volumes/personal/projects/scenic-lab/bathy/raw/hydrolakes/HydroLAKES_polys_v10_shp.zip
NAS=brandontsang@fishandchips.local
STORE=/volume1/personal/projects/scenic-roads/sources/fetched/smalllakes
WORK=${WORK:-$HOME/scenic-smalllakes}
PORT=${PORT:-18170}
mkdir -p $WORK/home $WORK/grids $WORK/index $WORK/logs
LOG=$WORK/logs/run-$(date +%Y%m%d-%H%M%S).log
exec > >(tee -a $LOG) 2>&1

caffeinate -i -w $$ &

[[ -d $NASROOT/base ]] || { echo "the NAS isn't mounted ($NASROOT)" >&2; exit 1; }
ssh -o BatchMode=yes $NAS "mkdir -p $STORE" || { echo "no SSH to the NAS (1Password locked?)" >&2; exit 1; }

# HydroLAKES read locally (each box scans the shapefile).
[[ -s $WORK/HydroLAKES_polys_v10_shp.zip ]] || cp $HYDROLAKES_NAS $WORK/HydroLAKES_polys_v10_shp.zip

(cd $REPO && PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release -p server)
$REPO/target/release/server --port $PORT --home $WORK/home --root $NASROOT --no-mirror --web $REPO/web/dist --fonts $REPO/data/fonts > $WORK/logs/server.log 2>&1 &
SERVER=$!
pids=()
# (The server and the workers stopped by their own PIDs.)
stop() { kill $SERVER $pids 2>/dev/null || true; }
trap stop EXIT
trap 'stop; exit 130' INT TERM
for i in {1..60}; do
  curl -s -o /dev/null http://127.0.0.1:$PORT/ && break
  sleep 1
done

# The boxes: the coverage's z6 tiles.
ls $NASROOT/base | sed -n 's/^6-\([0-9]*\)-\([0-9]*\)\..*/\1 \2/p' | sort -u > $WORK/z6.txt
(cd $REPO/producers/depth && uv run --project .. python -c '
import sys
from grid import Window
for l in open(sys.argv[1]):
    x, y = map(int, l.split())
    w, s, e, n = Window(6, x * 256, y * 256, 256, 256).bounds_lonlat()
    print(f"6-{x}-{y} {w},{s},{e},{n}")
' $WORK/z6.txt) > $WORK/boxes.txt
echo "$(wc -l < $WORK/boxes.txt | tr -d ' ') boxes, $WORKERS workers"

# The pool's battery floor (%), read again each wait so a change holds.
floor() {
  local f
  f=$(sed -n 's/.*"pct":\([0-9]*\).*/\1/p' $NASROOT/state/pool/battery-floor.json 2>/dev/null)
  echo ${f:-30}
}

power_ok() {
  local b
  b=$(pmset -g batt)
  [[ $b == *"AC Power"* ]] && return 0
  local pct=${${b#*$'\t'}%%%*}
  (( pct >= $(floor) ))
}

one_box() {
  local id=$1 box=$2
  [[ -e $WORK/index/$id.ok ]] && return 0
  until power_ok; do echo "$id: battery under $(floor) %, waiting"; sleep 300; done
  (cd $REPO/producers/depth && uv run --project .. python smalllakes.py \
    --hydrolakes $WORK/HydroLAKES_polys_v10_shp.zip --server http://127.0.0.1:$PORT \
    --box=$box --out $WORK/grids --index $WORK/index/$id.geojson) >> $WORK/logs/$id.log 2>&1 \
    || { echo "$id: failed (logs/$id.log)"; return 1; }
  local files=$WORK/index/$id.files
  uv run --project $REPO/producers python -c '
import json, sys
for p in sorted({f["properties"]["pack"] for f in json.load(open(sys.argv[1]))["features"]}):
    print(p)
' $WORK/index/$id.geojson > $files || { echo "$id: its index unreadable"; return 1; }
  if [[ -s $files ]]; then
    COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C $WORK/grids -cf - -T $files \
      | ssh -o BatchMode=yes $NAS "cd $STORE && tar -xf -" || { echo "$id: the copy to the NAS failed"; return 1; }
    (cd $WORK/grids && xargs stat -f '%N %z' < $files | sort) > $files.here || { echo "$id: its grids unlisted here"; return 1; }
    ssh -o BatchMode=yes $NAS "cd $STORE && xargs stat -c '%n %s'" < $files | sort > $files.nas || { echo "$id: its grids unlisted on the NAS"; return 1; }
    cmp -s $files.here $files.nas || { echo "$id: sizes on the NAS differ"; return 1; }
  fi
  # (Done once on the NAS; the local copies go after, so a stop in between only leaves them.)
  touch $WORK/index/$id.ok
  [[ -s $files ]] && (cd $WORK/grids && xargs rm -f < $files)
  echo "$id: done, $(wc -l < $files | tr -d ' ') grids"
}

worker() {
  local k=$1 n=0 bad=0
  while read -r id box; do
    (( n++ % WORKERS == k )) || continue
    one_box $id $box || bad=1
  done < $WORK/boxes.txt
  return $bad
}

for k in $(seq 0 $((WORKERS - 1))); do
  worker $k &
  pids+=$!
done
fail=0
for p in $pids; do wait $p || fail=1; done
(( fail == 0 )) || { echo "some boxes failed: run again to retry them"; exit 1; }

# The boxes' indexes merged, beside the grids.
uv run --project $REPO/producers python -c '
import glob, json, sys
feats = []
for p in sorted(glob.glob(sys.argv[1] + "/*.geojson")):
    feats += json.load(open(p))["features"]
feats.sort(key=lambda f: f["properties"]["file"])
with open(sys.argv[2], "w") as f:
    json.dump({"type": "FeatureCollection", "features": feats}, f, separators=(",", ":"))
    f.write("\n")
print(len(feats), "grids")
' $WORK/index $WORK/smalllakes-grids.geojson
scp -q $WORK/smalllakes-grids.geojson $NAS:$STORE/
echo "all done: $STORE/smalllakes-grids.geojson"
