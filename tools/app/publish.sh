#!/bin/zsh
# Publish the app (docs/plan.md §8, App publishing): build the server and the web app, run the
# tests, smoke-test the new server on a spare port against the NAS's current catalog, then copy it
# to the NAS (app/<version>/) and point app/current.json at it. Both Macs' servers pick it up within
# five minutes and restart into it when their map is idle.
#
#   tools/app/publish.sh              build, test, publish
#   tools/app/publish.sh --rollback   point app/current.json back at the previous version
#
# Either Mac may publish: a lock on the NAS (app/publish.lock) keeps two publishes from writing
# app/current.json at once, and a publish is only ever of a clean tree whose commit is on origin's
# main (no -dirty versions; the task board's database, tools/taskboard/taskboard.db, aside).
set -euo pipefail
cd ${0:A:h}/../..
NAS=/Volumes/personal/projects/scenic-roads
# (Every copy leaves the extended attributes behind, cp -X: the NAS refuses macOS's provenance one
# when it differs from the folder's, failing the copy.)
[[ -d $NAS ]] || { echo "the NAS isn't mounted at /Volumes/personal"; exit 2; }
# The lock: made with create-new (zsh's noclobber opens with O_EXCL, which the share does
# atomically), naming its Mac, process and time; removed as this script exits. One older than six
# hours (its Mac gone mid-publish) is moved aside and taken.
lock=$NAS/app/publish.lock
cleanup=()
# Its last line says how it ended: "published <version>" (exit 0), or, on any other exit, "publish:
# FAILED, nothing published (exit N)" with N non-zero, whatever stopped it. (A wrapper's own exit
# status, e.g. `publish.sh > log; grep … log`, is the wrapper's last command's: read this line or
# publish.sh's own status.)
published=
on_exit() {
  local rc=$?
  for c in $cleanup; do eval $c; done
  if [[ -z $published && ${rollback:-} != done ]]; then
    (( rc == 0 )) && rc=1
    echo "publish: FAILED, nothing published (exit $rc)"
  fi
  exit $rc
}
trap on_exit EXIT
# (Ctrl-C or a kill leaves through EXIT too, so the lock is freed.)
trap 'exit 130' INT TERM HUP
who="$(scutil --get LocalHostName 2>/dev/null || hostname -s) $$ $(date -u +%Y-%m-%dT%H:%M:%SZ)"
if ! ( set -o noclobber; print -r -- "$who" > $lock ) 2>/dev/null; then
  held=$(cat $lock 2>/dev/null)
  age=$(( $(date +%s) - $(stat -f %m $lock 2>/dev/null || date +%s) ))
  if (( age < 6 * 3600 )); then
    echo "another publish holds $lock ($held, $(( age / 60 )) min ago): try again once it's over"
    exit 1
  fi
  # Moved aside, then checked to be the stale lock read above: one another publish took meanwhile
  # (moving the stale one first) is put back and left to it.
  mv $lock $lock.stale-$$ 2>/dev/null || { echo "another publish took $lock just now"; exit 1; }
  if [[ $(cat $lock.stale-$$ 2>/dev/null) != $held ]]; then
    ( set -o noclobber; cat $lock.stale-$$ > $lock ) 2>/dev/null
    rm -f $lock.stale-$$
    echo "another publish took $lock just now"; exit 1
  fi
  ( set -o noclobber; print -r -- "$who" > $lock ) 2>/dev/null || { echo "another publish took $lock just now"; exit 1; }
  echo "publish: took over a lock $(( age / 3600 )) h old ($(cat $lock.stale-$$ 2>/dev/null))"
  rm -f $lock.stale-$$
fi
# (Removed only while it's still this publish's.)
cleanup+=('[[ $(cat $lock 2>/dev/null) == $who ]] && rm -f $lock')
if [[ ${1:-} == --rollback ]]; then
  [[ -f $NAS/app/previous.json ]] || { echo "no previous version to roll back to"; exit 1; }
  cp -X $NAS/app/current.json $NAS/app/current.json.tmp.old
  cp -X $NAS/app/previous.json $NAS/app/current.json.tmp && mv $NAS/app/current.json.tmp $NAS/app/current.json
  mv $NAS/app/current.json.tmp.old $NAS/app/previous.json
  rollback=done
  echo "rolled back to $(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")"
  exit 0
fi
# Only a clean tree's commit is published, and only one on origin's main, so every version names a
# commit either Mac can check out. (Built from a snapshot of that commit, below.)
# (The task board's database changes all the time and isn't published; untracked files, such as
# notes, aren't counted.)
unclean() { git status --porcelain --untracked-files=no -- . ':(exclude)tools/taskboard/taskboard.db*'; }
[[ -z $(unclean) ]] || { echo "the tree isn't clean: commit or set aside what changed first"; unclean | head -20; exit 1; }
git fetch -q origin main || { echo "can't fetch origin's main"; exit 1; }
git merge-base --is-ancestor HEAD origin/main || { echo "HEAD ($(git rev-parse --short HEAD)) isn't on origin's main: push it first"; exit 1; }
# The old name of the lead's role (the pattern below) nowhere in the code, docs or UI (the owner's
# rule, 14 Oct 2026): the role is the lead's, a measure taken on a Mac names it (the M4, the M1).
# History (the task board) is exempt.
if git grep -qniI -E 'build[ -]mac' -- . ':(exclude)tools/taskboard'; then
  echo "the old name of the lead's role is back (the role is \"the lead\"; a measure names its Mac, the M4 or the M1):"
  git grep -niI -E 'build[ -]mac' -- . ':(exclude)tools/taskboard' | head -20
  exit 1
fi
# The commit it's built from, read now, and built from a snapshot of it: a checkout of its own kept
# for publishing (~/Library/Caches/scenic-publish/tree), so commits and edits here meanwhile change
# nothing published. Kept between publishes as a build cache: its target/ and web/node_modules
# (reinstalled when web's lockfile changed) carry over, so only what changed is rebuilt.
head=$(git rev-parse --short HEAD)
repo=$PWD
snap=~/Library/Caches/scenic-publish/tree
if [[ ! -e $snap/.git ]]; then
  git worktree prune
  mkdir -p ${snap:h}
  git worktree add -q --detach $snap $head
fi
git -C $snap checkout -q --detach --force $head
git -C $snap clean -qfdx -e /target -e /web/node_modules
cd $snap
mise trust -q 2>/dev/null || true
# Rust (rust-toolchain.toml's), and the wasi-sdk, Node, pnpm and cargo-nextest at mise.toml's versions, installed
# here when this Mac lacks one (a version pinned since this Mac last published).
mise install -q || { echo "mise install failed"; exit 1; }
eval "$(mise env -s zsh)"
cargo nextest --version >/dev/null 2>&1 || { echo "publish: cargo nextest is missing (mise.toml pins it; mise install should have put it on the PATH)"; exit 1; }
lockhash=$(shasum -a 256 web/pnpm-lock.yaml | cut -c1-64)
if [[ ! -d web/node_modules || $(cat target/.publish-pnpm-lock 2>/dev/null) != $lockhash ]]; then
  rm -rf web/node_modules; (cd web && pnpm install --frozen-lockfile --reporter=silent) || { echo "pnpm install failed"; exit 1; }
  mkdir -p target && print -r -- $lockhash > target/.publish-pnpm-lock
fi
# The certificate the listening programs are signed with (below), tried now on a scratch file so a
# keychain that won't let codesign use its key (locked, or no GUI to answer its prompt) stops the
# publish before the build rather than after.
sign_id=-
if security find-identity -p codesigning 2>/dev/null | grep -q '"Scenic Code Signing"'; then
  sign_id="Scenic Code Signing"
  probe=$(mktemp -d)/probe; cp /usr/bin/true $probe
  codesign -s "$sign_id" --force $probe || { echo "publish: codesign can't use the \"Scenic Code Signing\" key: allow it in Keychain Access (Always Allow), or unlock the login keychain"; exit 1; }
else
  echo "publish: no \"Scenic Code Signing\" certificate on this Mac: signing ad hoc (a firewall's allow won't outlive this update)"
fi
# The guards first (tools/app/guards.sh, docs/inputs.md §7): the drop boxes read only through their
# accessor, no data source named in the app's code, and the Python steps loaded from the app's dem/
# in a stand-in NAS root whose drop boxes can't be read (and checked by pyflakes).
zsh tools/app/guards.sh || exit 1
cargo build --release -p server -p pipeline 2>&1 | tail -2
# Room for the files the tests open at once: a session over SSH starts with 256, which the tests
# run in parallel go past (8 Oct: 15 failed so on the M4, all passing at 8192). They run under
# cargo-nextest (mise.toml), each test its own process: pipeline's whole suite took 13.8 min against
# cargo test's 20.3 (M1, 18 Oct, #269). Their whole output is kept
# (~/Library/Logs/scenic/publish-tests-<time>.log); shown: the summaries, and each failed test's own output.
ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
mkdir -p ~/Library/Logs/scenic
tests_log=~/Library/Logs/scenic/publish-tests-$(date +%Y%m%d-%H%M%S).log
tests_ok=0
# (Tests that pass over what a Mac lacks fail instead when they're run for a publish: uv for the
# translations' checker.)
export SCENIC_PUBLISH=1
# (After the libraries, the pipeline's whole-crate checks: every cache read through the accessor,
# the records written only through Out (docs/pool.md §7.3), every Wikimedia request through the
# budgets' proxy (§7.7), the Python steps' timer. The drop boxes' and the sources' guards ran first.)
# Both run on the M4 (tools/app/remote-tests.sh, #442: an exact copy of this snapshot, its build
# folder kept there between publishes, at nice 10; here on the M4 itself, when it can't be reached,
# or with SCENIC_TESTS_LOCAL=1): the suite took 13-30 min on the M1 beside its builds, and a
# timing test failed under that load. Its exit status is the suite's.
suite='export SCENIC_PUBLISH=1; cargo nextest --version >/dev/null 2>&1 || { mise install -q; eval "$(mise env -s zsh)"; }; rc=0
cargo nextest run --no-fail-fast --hide-progress-bar -p store -p names -p pipeline --lib || rc=$?
cargo nextest run --no-fail-fast --hide-progress-bar -p pipeline --test cache_accessor --test write_sets --test budgets --test timings_python || rc=$?
exit $rc'
zsh tools/app/remote-tests.sh publish $snap $suite > $tests_log 2>&1 || tests_ok=$?
# (And every child, listener and taken connection by store::spawn: clippy's disallowed-methods,
# clippy.toml, docs/pool.md §7.5. A hit fails the publish.)
cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods >> $tests_log 2>&1 || { tests_ok=$?; grep -A6 'disallowed' $tests_log; }
grep -E '^ *Summary ' $tests_log || true
if (( tests_ok != 0 )); then
  sed -n '/^ *FAIL \[/,/^ *\(PASS\|SLOW\|FAIL\|Summary\) /p' $tests_log | grep -v '^progress: ' || true
  echo "tests failed; their whole output: $tests_log"
  exit $tests_ok
fi
# The programs' WebAssembly builds, which the coordinator serves to web workers (docs/workers.md).
zsh tools/app/wasm.sh >/dev/null || { echo "WebAssembly build failed"; exit 1; }
# Built into its own folder: web/dist may be what a development server is serving. (The type check
# writes and checks our MapLibre's declarations, then the app's; the build compiles its source with
# the app: web/maplibre/FORK.md.)
(cd web && pnpm run typecheck && pnpm exec vite build --outDir dist-publish --emptyOutDir >/dev/null) || { echo "web build failed"; exit 1; }
fonts=$repo/data/fonts
[[ -d $fonts ]] || fonts=$NAS/app/fonts
[[ -d $fonts ]] || { echo "no fonts folder (data/fonts or the NAS's app/fonts)"; exit 1; }
# Smoke test: the new server, its own empty home (no mirror), the NAS's catalog.
home=$(mktemp -d); port=18080
./target/release/server --port $port --home $home --no-mirror --web web/dist-publish --fonts $fonts > $home/server.log 2>&1 &
pid=$!
cleanup+=("kill $pid 2>/dev/null; rm -rf $home")
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
version=$(date -u +%Y%m%d-%H%M)-$head
dest=$NAS/app/$version
mkdir -p $dest.tmp/web $dest.tmp/fonts
# The programs that listen (the map server; the agent, whose coordinator and broker members reach)
# signed with the owner's certificate when this Mac has it, under fixed identifiers, so a Mac's
# firewall allow outlives an update (an ad-hoc signature is the build's own hash, so its allow went
# with it). Signed here before the copy: codesign refuses files on the NAS.
codesign -s "$sign_id" -i local.scenic.server --force target/release/server
codesign -s "$sign_id" -i local.scenic.agent --force target/release/scenic
# The server, and the agent with the programs its jobs run (each Mac runs them from here):
# the pipeline's binaries, and the Python steps (dem/, run with uv) with their lock file.
cp -X target/release/server target/release/scenic target/release/scenic-build target/release/extract \
   target/release/tile target/release/scenic-metrics target/release/elev target/release/areaflags \
   target/release/landcover target/release/railfreq target/release/trees target/release/bldtile \
   target/release/terrainsub $dest.tmp/
mkdir -p $dest.tmp/wasm
cp -X target/wasm32-wasip1/release/{extract,tile,scenic-metrics,areaflags,elev,landcover,trees,bldtile,terrainsub}.wasm $dest.tmp/wasm/
mkdir -p $dest.tmp/dem
git ls-files dem | while read f; do cp -X "$f" "$dest.tmp/$f"; done
# Scenic.app (tools/status: the menu bar item, the build service, the map server's controls), built
# and signed on this Mac (codesign refuses a bundle on the NAS, whose SMB share adds Finder info),
# then copied without extended attributes (none to keep as AppleDouble files). Each Mac's copy in
# /Applications replaces itself with it.
sb=$(mktemp -d)
zsh tools/status/bundle.sh $sb $version "$sign_id" >/dev/null
cp -RX "$sb/Scenic.app" "$dest.tmp/"
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
published=$version
echo "published $version"
# The NAS's own planet fetch (DSM's Task Scheduler runs it daily): the repository's copy, when it
# differs.
if ! cmp -s tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh; then
  mkdir -p $NAS/nas
  cp -X tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh.tmp && chmod 755 $NAS/nas/fetch-planet.sh.tmp
  mv $NAS/nas/fetch-planet.sh.tmp $NAS/nas/fetch-planet.sh
  echo "updated nas/fetch-planet.sh"
fi
