#!/bin/zsh
# Publish the app (docs/plan.md §8, App publishing): build the server and the web app, run the
# tests, smoke-test the new server on a spare port against the NAS's current catalog, then copy it
# to the NAS (app/<version>/) and point app/current.json at it. Both Macs' servers pick it up within
# five minutes and restart into it when their map is idle.
#
#   tools/app/publish.sh              build, test, publish
#   tools/app/publish.sh --rollback   point app/current.json back at the previous version
set -euo pipefail
cd ${0:A:h}/../..
NAS=/Volumes/personal/projects/scenic-roads
[[ -d $NAS ]] || { echo "the NAS isn't mounted at /Volumes/personal"; exit 2; }
if [[ ${1:-} == --rollback ]]; then
  [[ -f $NAS/app/previous.json ]] || { echo "no previous version to roll back to"; exit 1; }
  cp $NAS/app/current.json $NAS/app/current.json.tmp.old
  cp $NAS/app/previous.json $NAS/app/current.json.tmp && mv $NAS/app/current.json.tmp $NAS/app/current.json
  mv $NAS/app/current.json.tmp.old $NAS/app/previous.json
  echo "rolled back to $(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")"
  exit 0
fi
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
cargo build --release -p server -p pipeline 2>&1 | tail -2
cargo test -q -p store -p names -p pipeline --lib 2>&1 | tail -3
(cd web && npm run build >/dev/null) || { echo "web build failed"; exit 1; }
fonts=data/fonts
[[ -d $fonts ]] || fonts=$NAS/app/fonts
[[ -d $fonts ]] || { echo "no fonts folder (data/fonts or the NAS's app/fonts)"; exit 1; }
# Smoke test: the new server, its own empty home (no mirror), the NAS's catalog.
home=$(mktemp -d); port=18080
./target/release/server --port $port --home $home --no-mirror --web web/dist --fonts $fonts > $home/server.log 2>&1 &
pid=$!
trap "kill $pid 2>/dev/null; rm -rf $home" EXIT
ok=1
for i in {1..60}; do curl -sf -o /dev/null http://127.0.0.1:$port/api/ping && break; sleep 0.5; done
for u in /api/ping /api/meta /api/catalog / "/tiles/terrain/5/9/11" "/tiles/roads/10/300/380"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port$u")
  [[ $code == 200 || $code == 204 ]] || { echo "smoke test: $u answered $code"; ok=0; }
done
[[ $ok == 1 ]] || { cat $home/server.log | tail -20; exit 1; }
kill $pid; wait $pid 2>/dev/null || true
version=$(date -u +%Y%m%d-%H%M)-$(git rev-parse --short HEAD)$( [[ -n $(git status --porcelain -- crates web/src) ]] && echo -dirty )
dest=$NAS/app/$version
mkdir -p $dest.tmp/web $dest.tmp/fonts
# The server, and the build agent with the programs its jobs run (the build Mac runs them from here).
cp target/release/server target/release/scenic target/release/scenic-build target/release/extract $dest.tmp/
rsync -a web/dist/ $dest.tmp/web/
rsync -a $fonts/ $dest.tmp/fonts/
mv $dest.tmp $dest
files=$(cd $dest && find . -type f ! -name .DS_Store | sed 's|^\./||' | sort | python3 -c "import json,sys;print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))")
[[ -f $NAS/app/current.json ]] && cp $NAS/app/current.json $NAS/app/previous.json
print -r -- "{\"version\": \"$version\", \"files\": $files}" > $NAS/app/current.json.tmp
mv $NAS/app/current.json.tmp $NAS/app/current.json
echo "published $version"
