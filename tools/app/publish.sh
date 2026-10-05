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
# (Every copy leaves the extended attributes behind, cp -X: the NAS refuses macOS's provenance one
# when it differs from the folder's, failing the copy.)
[[ -d $NAS ]] || { echo "the NAS isn't mounted at /Volumes/personal"; exit 2; }
if [[ ${1:-} == --rollback ]]; then
  [[ -f $NAS/app/previous.json ]] || { echo "no previous version to roll back to"; exit 1; }
  cp -X $NAS/app/current.json $NAS/app/current.json.tmp.old
  cp -X $NAS/app/previous.json $NAS/app/current.json.tmp && mv $NAS/app/current.json.tmp $NAS/app/current.json
  mv $NAS/app/current.json.tmp.old $NAS/app/previous.json
  echo "rolled back to $(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")"
  exit 0
fi
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
# The commit it's built from, read now: one made while it runs would name a version it isn't.
head=$(git rev-parse --short HEAD)
cargo build --release -p server -p pipeline 2>&1 | tail -2
cargo test -q -p store -p names -p pipeline --lib 2>&1 | tail -3
# The programs' WebAssembly builds, which the coordinator serves to web workers (docs/workers.md).
zsh tools/app/wasm.sh >/dev/null || { echo "WebAssembly build failed"; exit 1; }
# Built into its own folder: web/dist may be what a development server is serving.
(cd web && npx tsc --noEmit && npx vite build --outDir dist-publish --emptyOutDir >/dev/null) || { echo "web build failed"; exit 1; }
fonts=data/fonts
[[ -d $fonts ]] || fonts=$NAS/app/fonts
[[ -d $fonts ]] || { echo "no fonts folder (data/fonts or the NAS's app/fonts)"; exit 1; }
# Smoke test: the new server, its own empty home (no mirror), the NAS's catalog.
home=$(mktemp -d); port=18080
./target/release/server --port $port --home $home --no-mirror --web web/dist-publish --fonts $fonts > $home/server.log 2>&1 &
pid=$!
trap "kill $pid 2>/dev/null; rm -rf $home" EXIT
ok=1
for i in {1..60}; do curl -sf -o /dev/null http://127.0.0.1:$port/api/ping && break; sleep 0.5; done
for u in /api/ping /api/meta /api/catalog / "/tiles/terrain/5/9/11" "/tiles/roads/10/300/380"; do
  # A busy NAS can answer 503 for a moment (a slow read fails alone): a few tries each.
  for try in 1 2 3 4 5; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port$u")
    [[ $code == 503 ]] || break
    sleep 3
  done
  [[ $code == 200 || $code == 204 ]] || { echo "smoke test: $u answered $code"; ok=0; }
done
[[ $ok == 1 ]] || { cat $home/server.log | tail -20; exit 1; }
kill $pid; wait $pid 2>/dev/null || true
# The Python steps the build agent runs load from the app's dem/, which has nothing of the
# repository around it: each must import there (one read the repository's regions.json once).
pyt=$(mktemp -d)
mkdir -p $pyt/dem
git ls-files dem | while read f; do cp -X "$f" "$pyt/$f"; done
steps=(${(f)"$(grep -rhoE '"[a-z_]+\.py"' crates/pipeline/src | tr -d '"' | sed 's/\.py$//' | sort -u)"})
(cd $pyt/dem && uv run python -c "import importlib, sys; [importlib.import_module(m) for m in sys.argv[1:]]" $steps) || { echo "a Python step doesn't load from the app's dem/"; rm -rf $pyt; exit 1; }
# (Loading isn't running: a name used before it's bound, say, shows only then. pyflakes reads them.)
(cd $pyt/dem && uvx --quiet pyflakes ${steps/%/.py}) || { echo "pyflakes finds a mistake in a Python step"; rm -rf $pyt; exit 1; }
rm -rf $pyt
dirty=""
# (The Python steps and the status app are copied from the working tree too.)
[[ -z $(git status --porcelain -- crates web/src dem tools/status) ]] || dirty=-dirty
[[ $(git rev-parse --short HEAD) == $head ]] || { echo "a commit was made while publishing ($head, now $(git rev-parse --short HEAD)): publish again"; exit 1; }
version=$(date -u +%Y%m%d-%H%M)-$head$dirty
dest=$NAS/app/$version
mkdir -p $dest.tmp/web $dest.tmp/fonts
# The server, and the build agent with the programs its jobs run (the build Mac runs them from here):
# the pipeline's binaries, and the Python steps (dem/, run with uv) with their lock file.
cp -X target/release/server target/release/scenic target/release/scenic-build target/release/extract \
   target/release/tile target/release/scenic-metrics target/release/elev target/release/areaflags \
   target/release/landcover target/release/railfreq $dest.tmp/
mkdir -p $dest.tmp/wasm
cp -X target/wasm32-wasip1/release/{extract,tile,scenic-metrics,areaflags,elev,landcover}.wasm $dest.tmp/wasm/
mkdir -p $dest.tmp/dem
git ls-files dem | while read f; do cp -X "$f" "$dest.tmp/$f"; done
# The menu bar item (tools/status): an app bundle, built and signed ad hoc on this Mac (codesign
# refuses a bundle on the NAS, whose SMB share adds Finder info), then copied without extended
# attributes (none to keep as AppleDouble files).
sb=$(mktemp -d)/Scenic.app
mkdir -p "$sb/Contents/MacOS"
swiftc -O -swift-version 5 -o "$sb/Contents/MacOS/scenic-status" tools/status/main.swift
cp -X tools/status/Info.plist "$sb/Contents/Info.plist"
xattr -cr "$sb"
codesign -s - --force "$sb"
cp -RX "$sb" "$dest.tmp/"
rsync -a web/dist-publish/ $dest.tmp/web/
rsync -a $fonts/ $dest.tmp/fonts/
mv $dest.tmp $dest
# The manifest: every file with its SHA-256 (the servers check their copies against it).
manifest=$(cd $dest && find . -type f ! -name .DS_Store ! -name '._*' | sed 's|^\./||' | sort | python3 -c "
import hashlib, json, sys
files = [l.strip() for l in sys.stdin if l.strip()]
sha = {f: hashlib.sha256(open(f, 'rb').read()).hexdigest() for f in files}
print(json.dumps({'version': sys.argv[1], 'files': files, 'sha256': sha}))" "$version")
[[ -f $NAS/app/current.json ]] && cp -X $NAS/app/current.json $NAS/app/previous.json
print -r -- "$manifest" > $NAS/app/current.json.tmp
mv $NAS/app/current.json.tmp $NAS/app/current.json
echo "published $version"
# The NAS's own planet fetch (DSM's Task Scheduler runs it daily): the repository's copy, when it
# differs.
if ! cmp -s tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh; then
  mkdir -p $NAS/nas
  cp -X tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh.tmp && chmod 755 $NAS/nas/fetch-planet.sh.tmp
  mv $NAS/nas/fetch-planet.sh.tmp $NAS/nas/fetch-planet.sh
  echo "updated nas/fetch-planet.sh"
fi
