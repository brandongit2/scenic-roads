#!/bin/zsh
# Publish the app (docs/building.md §2, App publishing): build the server and the web app, run the
# tests, smoke-test the new server on a spare port against the NAS's current catalogue, then copy it
# to the NAS (app/<version>/) and point app/current.json at it. Both Macs' servers pick it up within
# five minutes and restart into it when their map is idle.
#
#   tools/app/publish.sh              build, test, publish
#   tools/app/publish.sh --rollback   point app/current.json back at the previous version
#
# Either Mac may publish, and two publishes may run at once: each builds and tests in its own tree
# beside the other, and only the install (app/<version>/, app/current.json) holds the lock on the NAS
# (app/publish.lock), the second waiting for the first's. A publish is only ever of a clean tree whose
# commit is on origin's main (no -dirty versions), and installs only over a live version its commit
# contains (fast-forward).
set -euo pipefail
# (Its arguments checked before anything: `--help` publishes nothing.)
usage="usage: tools/app/publish.sh [--rollback]"
rollback=
case $#:${1:-} in
  0:) ;;
  1:--rollback) rollback=asked ;;
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  *) print -ru2 -- "publish.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
cd ${0:A:h}/../..
source tools/app/lib.zsh
# Its tools found without a login shell's profile (over ssh, `ssh m6 '…/publish.sh'`, the PATH is the
# system's): brew's mise and rustup's cargo; the rest come from mise.toml through `mise env` below.
path=(/opt/homebrew/bin ~/.cargo/bin $path)
NAS=/Volumes/personal/projects/scenic-roads
# (Every copy leaves the extended attributes behind, cp -X: the NAS refuses macOS's provenance one
# when it differs from the folder's, failing the copy.)
[[ -d $NAS ]] || { echo "the NAS isn't mounted at /Volumes/personal"; exit 2; }
lock=$NAS/app/publish.lock
cleanup=()
# Its last line says how it ended: "published <version>" (exit 0), or, on any other exit, "publish:
# FAILED, nothing published (exit N)" with N non-zero, whatever stopped it. (A wrapper's own exit
# status, e.g. `publish.sh > log; grep … log`, is the wrapper's last command's: read this line or
# publish.sh's own status.)
# (tools/app/lib.zsh's at_end: zsh runs no EXIT trap when errexit stops it inside a function.)
published=
on_exit() {
  local rc=$? c
  for c in $cleanup; do eval $c; done
  if [[ -z $published && ${rollback:-} != done ]]; then
    (( rc == 0 )) && rc=1
    echo "publish: FAILED, nothing published (exit $rc)"
  fi
  return $rc
}
at_end on_exit
who="$(scutil --get LocalHostName 2>/dev/null || hostname -s) $$ $(date -u +%Y-%m-%dT%H:%M:%SZ)"
if [[ $rollback == asked ]]; then
  nas_lock $lock "$who" || { echo "another publish holds $lock ($lock_held): try again once it's over"; exit 1; }
  cleanup+=('nas_unlock $lock "$who"')
  [[ -f $NAS/app/previous.json ]] || { echo "no previous version to roll back to"; exit 1; }
  cp -X $NAS/app/current.json $NAS/app/current.json.tmp.old
  cp -X $NAS/app/previous.json $NAS/app/current.json.tmp && mv $NAS/app/current.json.tmp $NAS/app/current.json
  mv $NAS/app/current.json.tmp.old $NAS/app/previous.json
  rollback=done
  echo "rolled back to $(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")"
  exit 0
fi
# What's published is always the tip of origin's main, fetched now, so every version names a commit
# either Mac can check out; this checkout (its branch, its edits) plays no part. (Built from a
# snapshot of that commit, below.)
git fetch -q origin main || { echo "can't fetch origin's main"; exit 1; }
head=$(git rev-parse --short origin/main)
# (Refused now, before building, when it couldn't go in; checked again at the install, where a
# rollback since now refuses it too.)
began=$(live_version $NAS/app/current.json || true)
fast_forward $NAS/app/current.json $head || exit 1
# The old name of the lead's role (the pattern below) nowhere in the code, docs or UI (the owner's
# rule, 14 Oct 2026): the role is the lead's, a measure taken on a Mac names it (the M4, the M1).
# History (the task board) is exempt.
if git grep -qniI -E 'build[ -]mac' $head -- . ':(exclude)tools/taskboard'; then
  echo "the old name of the lead's role is back (the role is \"the lead\"; a measure names its Mac, the M4 or the M1):"
  git grep -niI -E 'build[ -]mac' $head -- . ':(exclude)tools/taskboard' | head -20
  exit 1
fi
# Built from a snapshot of that commit: a checkout of its own kept
# for publishing (~/Library/Caches/scenic-publish/tree), so commits and edits here meanwhile change
# nothing published. Kept between publishes as a build cache: its target/ and web/node_modules
# (reinstalled when web's lockfile changed) carry over, so only what changed is rebuilt.
# Each publish running at once has its own: the first free of tree and tree-2, held (tree.busy) until
# it ends; with both held a publish waits for one to free (claim_tree), making no third. tree-2,
# built only beside another publish, is removed by a publish that finds it unused for 3 days; both have their target/ pruned (prune_target, below).
repo=$PWD
cache=~/Library/Caches/scenic-publish
mkdir -p $cache
claim_tree $cache
spare=$cache/tree-2
# (Moved aside while held, then removed after: its lock is held a moment, not for the removal, so a
# publish starting meanwhile still finds it free.)
if [[ $snap != $spare && -e $spare && -z $(found $cache/tree-2.used -newermt '3 days ago') ]] && hold $spare.busy; then
  mv $spare $cache/tree-2.old-$$ && rm -f $cache/tree-2.used; unhold $spare.busy
  rm -rf $cache/tree-2.old-$$; git worktree prune
  echo "removed the spare publish tree (unused for 3 days)"
fi
touch $cache/${snap:t}.used
if [[ ! -e $snap/.git ]]; then
  # (A tree whose creation stopped part way is made again.)
  rm -rf $snap
  git worktree prune
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
tools/dev/web-deps.sh --online || exit 1
# The certificate the listening programs are signed with (below), tried now on a scratch file so a
# keychain that won't let codesign use its key (locked, or no GUI to answer its prompt) stops the
# publish before the build rather than after.
# Without it nothing is published: an ad-hoc signature would lose every Mac's firewall allow.
sign_id="Scenic Code Signing"
# A Mac that publishes over ssh (the mini) keeps the key in its own keychain, unlocked here with
# the password beside it: an ssh session can't reach the login keychain the desktop unlocked.
signing_kc=~/Library/Keychains/scenic-signing.keychain-db
signing_pw=~/.config/scenic/signing-keychain-pass
if [[ -f $signing_kc && -r $signing_pw ]]; then
  security unlock-keychain -p "$(<$signing_pw)" $signing_kc || { echo "publish: can't unlock $signing_kc"; exit 1; }
fi
security find-identity -p codesigning 2>/dev/null | grep -q "\"$sign_id\"" || { echo "publish: no \"$sign_id\" identity in this Mac's keychain (docs/building.md, App publishing)"; exit 1; }
probe=$(mktemp -d)/probe; cp /usr/bin/true $probe
codesign -s "$sign_id" --force $probe </dev/null || { echo "publish: codesign can't use the \"$sign_id\" key: unlock the login keychain and let codesign use the key (docs/building.md, App publishing)"; exit 1; }
# The guards first (tools/app/guards.sh, docs/inputs.md §7): the inputs folders read only through their
# accessor, no data source named in the app's code, and the Python steps loaded from the app's dem/
# in a stand-in NAS root whose inputs folders can't be read (and checked by pyflakes).
zsh tools/app/guards.sh || exit 1
cargo build --release -p server -p pipeline 2>&1 | tail -2
# Room for the files the tests open at once: a session over SSH starts with 256, which the tests
# run in parallel go past (8 Oct: 15 failed so on the M4, all passing at 8192). They run under
# cargo-nextest (mise.toml), each test its own process: pipeline's whole suite took 13.8 min against
# cargo test's 20.3 (M1, 18 Oct, #269). Their whole output is kept
# (~/Library/Logs/scenic/publish-tests-<time>.log); shown: the summaries, and each failed test's own output.
ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
mkdir -p ~/Library/Logs/scenic
tests_log=~/Library/Logs/scenic/publish-tests-$(date +%Y%m%d-%H%M%S)-$$.log
tests_ok=0
# (Tests that pass over what a Mac lacks fail instead when they're run for a publish: uv for the
# translations' checker.)
export SCENIC_PUBLISH=1
# The tests' and clippy's builds without debug info (task #660): on macOS each build's line tables
# stay in its object files beside the binaries (split-debuginfo "unpacked"), 80 GB of the 122 GB this
# tree's target/debug had grown to in 3.5 days on the mini (25 Oct). A failed test still says its
# file and line (the panic's location needs none), and no backtrace is asked for here. (Passed to
# the test Mac with the other CARGO_PROFILE_* variables: tools/app/remote-tests.sh.)
export CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_TEST_DEBUG=false
# (The whole workspace's tests, #848: every crate's libraries, binaries and integration tests, by
# nextest's own discovery (#662: a list of four had missed no_instant_back, cli, grant_rules and
# whole_kill as they were added): every cache read
# through the accessor, the records written only through Out (docs/pool.md §7.3), every Wikimedia
# request through the budgets' proxy (§7.7), the Python steps' timer, no Instant dated back...)
# Both run on the test Mac, the mini by default (tools/app/remote-tests.sh, #442: an exact copy of this snapshot, its build
# folder kept there between publishes, at nice 20; here when this is the test Mac, when it can't be reached,
# or with SCENIC_TESTS_LOCAL=1): the suite took 13-30 min on the M1 beside its builds, and a
# timing test failed under that load. Its exit status is the suite's.
suite='export SCENIC_PUBLISH=1; cargo nextest --version >/dev/null 2>&1 || { mise install -q; eval "$(mise env -s zsh)"; }; rc=0
cargo nextest run --no-fail-fast --hide-progress-bar --workspace || rc=$?
exit $rc'
# One publish's suite and clippy at a time on this Mac (two at once overload the test Mac, #684): the
# second waits for the first's (tests.busy).
until hold $cache/tests.busy; do
  [[ -n ${said_wait:-} ]] || { echo "waiting for the other publish's tests ($lock_held)"; said_wait=1; }
  sleep 10
done
cleanup+=("unhold $cache/tests.busy")
zsh tools/app/remote-tests.sh publish $snap $suite > $tests_log 2>&1 || tests_ok=$?
# Then every test outside the Rust workspace (tools/app/other-tests.zsh, #848: web/test and
# tools/check/*.test.mjs by glob, the taskboard's, the status app's Swift self-test, shell.zsh and
# test-guard.zsh; their parts at once, ~1 min, most of it swiftc), here, after the Rust suite rather
# than beside it: on the mini the suite runs on this same Mac. Their whole output follows the
# suite's in the log; a failing part fails the publish.
other_ok=0
zsh tools/app/other-tests.zsh >> $tests_log 2>&1 || other_ok=$?
# (And every child, listener and taken connection by store::spawn: clippy's disallowed-methods,
# clippy.toml, docs/pool.md §7.5. A hit fails the publish.)
cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods >> $tests_log 2>&1 || { tests_ok=$?; grep -A6 'disallowed' $tests_log; }
unhold $cache/tests.busy
# (Only the final result counts: when the M4's run ended without one and the suite ran again here,
# remote-tests.sh marked where the rerun starts.)
from=$(grep -n -- '^--- remote result discarded, local rerun ---$' $tests_log | tail -1 | cut -d: -f1 || true)
final() { tail -n +${from:-1} $tests_log; }
final | grep -E '^ *Summary ' || true
grep -E '^other tests: ' $tests_log || true
(( other_ok == 0 )) || { (( tests_ok != 0 )) || tests_ok=$other_ok; }
if (( tests_ok != 0 )); then
  final | sed -n '/^ *FAIL \[/,/^ *\(PASS\|SLOW\|FAIL\|Summary\) /p' | grep -v '^progress: ' || true
  echo "tests failed; their whole output: $tests_log"
  exit $tests_ok
fi
# The programs' WebAssembly builds, which the coordinator serves to web workers (docs/workers.md).
zsh tools/app/wasm.sh >/dev/null || { echo "WebAssembly build failed"; exit 1; }
# The build folder kept small (task #660): cargo never deletes what it built, so each change of the
# dependencies or a profile left every workspace crate's old build behind (the mini's target/ grew
# by ~35 GB a day, to 127 GB). What these builds use is read at each publish, so only an entry a
# later publish needs again is built again (tools/app/lib.zsh's prune_target).
prune_target
# Built into its own folder: web/dist may be what a development server is serving. (The type check
# writes and checks our MapLibre's declarations, then the app's; the build compiles its source with
# the app: web/maplibre/FORK.md.)
(cd web && pnpm run typecheck && pnpm exec vite build --outDir dist-publish --emptyOutDir >/dev/null) || { echo "web build failed"; exit 1; }
fonts=$repo/data/fonts
[[ -d $fonts ]] || fonts=$NAS/app/fonts
[[ -d $fonts ]] || { echo "no fonts folder (data/fonts or the NAS's app/fonts)"; exit 1; }
# Smoke test: the new server, its own empty home (no mirror), the NAS's catalogue.
# (Its port its tree's, so two publishes' smoke tests can run at once.)
home=$(mktemp -d); port=18080; [[ $snap == $spare ]] && port=18081
./target/release/server --port $port --home $home --no-mirror --web web/dist-publish --fonts $fonts > $home/server.log 2>&1 &
pid=$!
cleanup+=("kill $pid 2>/dev/null; rm -rf $home")
# Up to 10 min for its first answer: its catalogue is read from the NAS first, which a busy NAS, or
# this Mac under a second publish's build, slows (21 Oct: 19 s+ under the depth repack; 26 Oct:
# past 2 min beside another publish's build, task #698). The shading layers' tiles too, as stored and
# made from an ancestor (z16): every slope tile once answered 503, its packs' tiles undecodable.
smoke $port $pid $home/server.log 600 /api/ping /api/meta /api/catalog / "/tiles/terrain/5/9/11" "/tiles/roads/10/300/380" \
  "/tiles/slope/5/9/11" "/tiles/slope/12/1202/1463" "/tiles/slope/16/18732/22928" "/tiles/depth-shade/16/4608/16896" || exit 1
kill $pid; wait $pid 2>/dev/null || true
version=$(date -u +%Y%m%d-%H%M)-$head
dest=$NAS/app/$version
sb=$(mktemp -d)
# The programs that listen (the map server; the agent, whose coordinator and broker members reach)
# signed with the owner's certificate when this Mac has it, under fixed identifiers, so a Mac's
# firewall allow outlives an update (an ad-hoc signature is the build's own hash, so its allow went
# with it). Signed here before the copy: codesign refuses files on the NAS.
codesign -s "$sign_id" -i local.scenic.server --force target/release/server
codesign -s "$sign_id" -i local.scenic.agent --force target/release/scenic
# Scenic.app (tools/status: the menu bar item, the build service, the map server's controls), built
# and signed on this Mac (codesign refuses a bundle on the NAS, whose SMB share adds Finder info),
# then copied without extended attributes (none to keep as AppleDouble files). Each Mac's copy in
# /Applications replaces itself with it.
zsh tools/status/bundle.sh $sb $version "$sign_id" >/dev/null
# The install, the one part two publishes can't run at once: the lock on the NAS, waited for while
# another publish installs (one older than six hours is taken over), freed only by its holder.
until nas_lock $lock "$who"; do
  [[ -n ${said_lock:-} ]] || { echo "waiting for the publish holding $lock ($lock_held)"; said_lock=1; }
  sleep 10
done
cleanup+=('nas_unlock $lock "$who"')
# (Again, under the lock: the publish that went in meanwhile may be newer, or a rollback since.)
fast_forward $NAS/app/current.json $head "$began" || exit 1
[[ ! -e $dest ]] || { echo "$dest exists already"; exit 1; }
rm -rf $dest.tmp; mkdir -p $dest.tmp/web $dest.tmp/fonts
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
cp -RX "$sb/Scenic.app" "$dest.tmp/"  # (Scenic.app, signed above)
rsync -a web/dist-publish/ $dest.tmp/web/
rsync -a $fonts/ $dest.tmp/fonts/
mv $dest.tmp $dest
# The manifest: every file with its SHA-256 (the servers check their copies against it).
manifest=$(cd $dest && find . -type f ! -name .DS_Store ! -name '._*' | sed 's|^\./||' | sort | python3 -c "
import hashlib, json, sys
files = [l.strip() for l in sys.stdin if l.strip()]
sha = {f: hashlib.sha256(open(f, 'rb').read()).hexdigest() for f in files}
print(json.dumps({'version': sys.argv[1], 'files': files, 'sha256': sha}))" "$version")
# The NAS's own planet fetch (DSM's Task Scheduler runs it daily): the repository's copy, when it
# differs. (Before the version goes live, so "published …" stays the last line and a failure here
# publishes nothing.)
if ! cmp -s tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh; then
  mkdir -p $NAS/nas
  cp -X tools/nas/fetch-planet.sh $NAS/nas/fetch-planet.sh.tmp && chmod 755 $NAS/nas/fetch-planet.sh.tmp
  mv $NAS/nas/fetch-planet.sh.tmp $NAS/nas/fetch-planet.sh
  echo "updated nas/fetch-planet.sh"
fi
[[ -f $NAS/app/current.json ]] && cp -X $NAS/app/current.json $NAS/app/previous.json
print -r -- "$manifest" > $NAS/app/current.json.tmp
mv $NAS/app/current.json.tmp $NAS/app/current.json
published=$version
echo "published $version"
