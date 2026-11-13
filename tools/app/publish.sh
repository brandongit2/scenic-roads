#!/bin/zsh
# Publish the app (docs/building.md §2, App publishing): build the server and the web app, run the
# tests (the server's routes among them, on a fixture map: crates/server/tests/serves.rs, run against
# the release server too), then put it live on the lead over HTTP (tools/app/lib.zsh's lead_install:
# its archive and manifest uploaded, checked there and taken only over the live release this
# publish checked against), which keeps it with the release before and serves it. Every Mac's agent
# fetches it from the lead within five minutes (coordwire::releases, jobrt::appupdate) and restarts
# into it by its update drain; the Mac mini's map server, the only one, restarts into it once the
# map has been idle a minute. Until every Mac has fetched from the lead for a day, the release also
# goes to the NAS's app/releases/ as before, through the NAS's Mac (the Mac mini: nas_send and
# nas_install), for the updaters before #1118 (docs/building.md, The NAS's app/ until then).
#
#   tools/app/publish.sh              build, test, publish
#   tools/app/publish.sh --rollback   put the release before back live on the lead (and on the NAS),
#                                     unless it's before the live one's rollback floor
#                                     (tools/app/rollback-floor)
#
# Any Mac that reaches the lead (and the NAS's Mac) may publish, at home or away, and two publishes
# may run at once: each builds and tests in its own tree beside the other; the lead takes each only
# over the live release it was checked against, so the second, refused, checks again and goes in
# after the first (the NAS's install holds its lock on the NAS, app/publish.lock). A publish is only
# ever of a clean tree whose commit is on origin's main (no -dirty versions), and installs only over
# a live version its commit contains (fast-forward).
# Each publish is recorded for the dev job dashboard (tools/dev/devjob.zsh): the run and its steps,
# the tests, compiles and clippy under theirs.
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
source tools/dev/devjob.zsh
# Its tools found without a login shell's profile (over ssh, `ssh m6 '…/publish.sh'`, the PATH is the
# system's): brew's mise and rustup's cargo; the rest come from mise.toml through `mise env` below.
path=(/opt/homebrew/bin ~/.cargo/bin $path)
# The NAS's root on the NAS's Mac, where every read and write of its app/ runs (lib.zsh's on_nas_mac).
NAS=/Volumes/personal/projects/scenic-roads
sb=$(mktemp -d)
cleanup=("rm -rf $sb")
# Its last line says how it ended: "published <version>" (exit 0), or, on any other exit, "publish:
# FAILED, nothing published (exit N)" with N non-zero, whatever stopped it. (A wrapper's own exit
# status, e.g. `publish.sh > log; grep … log`, is the wrapper's last command's: read this line or
# publish.sh's own status.)
# (tools/app/lib.zsh's at_end: zsh runs no EXIT trap when errexit stops it inside a function.)
# (Or, when the NAS's Mac had the go-ahead and its answer was lost, "publish: FAILED, unknown
# whether <version> went live …": its app/releases/current.json says.)
# (Or, live on the lead and the NAS's app/ not written after it: "publish: FAILED: <version> is live
# on the lead, …": the Macs on an app before #1118 don't get it.)
published= lead_state=
on_exit() {
  local rc=$? c
  for c in $cleanup; do eval $c; done
  if [[ -z $published && ${rollback:-} != done ]]; then
    (( rc == 0 )) && rc=1
    if [[ $lead_state == live ]]; then
      echo "publish: FAILED: $version is live on the lead, but the NAS's app/ (the updaters before #1118 read it) wasn't written (exit $rc)"
    elif [[ ${nas_state:-} == unknown ]]; then
      echo "publish: FAILED, unknown whether $version went live: the NAS's Mac had the go-ahead, then didn't answer (its app/releases/current.json says; exit $rc)"
    else echo "publish: FAILED, nothing published (exit $rc)"; fi
    c=$djstep_name; step "" 1; dj_end $djrun fail exit $rc ${c:+failed_in} ${c:+$c}
  else
    step ""; dj_end $djrun ok ${published:+version} ${published:+$published}
  fi
  return $rc
}
# The run's steps, each a span of it (the one under way ended as the next begins; `step ""` ends it).
djstep= djstep_name=
step() {  # step <name> [<the exit status of the one ending: 0 ok, else fail>]
  [[ -n $djstep ]] && { (( ${2:-0} )) && dj_end $djstep fail || dj_end $djstep ok }
  djstep= djstep_name=$1
  [[ -n $1 ]] || { export SCENIC_DEVJOB_PARENT=$djrun; return 0 }
  SCENIC_DEVJOB_PARENT=$djrun dj_start step $1; djstep=$REPLY
  export SCENIC_DEVJOB_PARENT=${djstep:-$djrun}
}
dj_start publish ${${rollback:+rollback}:-publish}; djrun=$REPLY
export SCENIC_DEVJOB_PARENT=$djrun
at_end on_exit
from="$(scutil --get LocalHostName 2>/dev/null || hostname -s) $$"
# The lead's releases read and rolled back through this Mac's installed app (`scenic releases`),
# when it has them (an app before #1118 doesn't: then the NAS's alone, the rollout's).
cli="$HOME/Library/Application Support/scenic/app/current/scenic"
[[ -x $cli ]] && $cli releases --help >/dev/null 2>&1 || cli=
lead_rc=4
if [[ $rollback == asked ]]; then
  # (No `scenic releases` here is the NAS's alone only while this Mac runs the live release: one
  # behind it, an app before #1118, can't tell whether the lead keeps releases, which a rollback of
  # the NAS's alone would leave live for every Mac on the new app.)
  if [[ -z $cli ]]; then
    bounded 60 on_nas_mac nas_current $NAS </dev/null > $sb/current.json \
      || { echo "can't read the live manifest on the NAS's Mac ($nas_host)"; exit 1; }
    mine=$(readlink "$HOME/Library/Application Support/scenic/app/current" 2>/dev/null || true)
    [[ -s $sb/current.json && $mine == $(live_version $sb/current.json) ]] || {
      echo "this Mac's app (${mine:-none}) has no \`scenic releases\` and isn't the live release: roll back from a Mac on it, or once this one has updated"
      exit 1; }
  fi
  if [[ -n $cli ]]; then lead_rc=0; lead_rollback $cli $sb/lead || lead_rc=$?; fi
  (( lead_rc == 0 || lead_rc == 4 )) || exit $lead_rc
  (( lead_rc == 0 )) || echo "the lead keeps no releases (an app before #1118, here or there): the NAS's alone"
  # (Rollout, docs/building.md: the NAS's too, for the updaters before #1118.)
  bounded 120 on_nas_mac nas_rollback $NAS "$from" </dev/null || {
    rc=$?; (( rc != 124 )) || echo "no answer from the NAS's Mac ($nas_host) within 120 s"; exit $rc; }
  rollback=done
  exit 0
fi
# What's published is always the tip of origin's main, fetched now, so every version names a commit
# either Mac can check out; this checkout (its branch, its edits) plays no part. (Built from a
# snapshot of that commit, below.)
step fetch
git fetch -q origin main || { echo "can't fetch origin's main"; exit 1; }
head=$(git rev-parse --short origin/main)
dj_attr $djrun head $head
# The live version, read on the NAS's Mac: refused now, before building, when it couldn't go in
# (checked again at the install, where a rollback since now refuses it too); and the NAS's Mac
# unreachable, or without the NAS, stops the publish here, before the build.
step "NAS's Mac"
bounded 60 on_nas_mac nas_current $NAS </dev/null > $sb/current.json \
  || { echo "can't read the live manifest on the NAS's Mac ($nas_host): unreachable, or without the NAS"; exit 1; }
[[ -s $sb/current.json ]] || rm $sb/current.json
began=$(live_version $sb/current.json || true)
fast_forward $sb/current.json $head || exit 1
# The lead's live release, read now too (through this Mac's installed app, when it has
# `scenic releases`), checked as the NAS's: refused now, before building; and the lead unreachable
# stops the publish here. Checked again at the install, by the lead too.
step "the lead"
lead_began=
if [[ -n $cli ]]; then
  lead_rc=0; lead_live $cli $sb/lead || lead_rc=$?
  case $lead_rc in
    0) lead_began=$(live_version $sb/lead/current.json || true)
       fast_forward $sb/lead/current.json $head || exit 1 ;;
    4) echo "the lead keeps no releases (an app before #1118): checked at the install" ;;
    *) echo "can't read the lead's releases (the lead's coordinator unreachable?)"; exit 1 ;;
  esac
fi
# The old name of the lead's role (the pattern below) nowhere in the code, docs or UI (the owner's
# rule, 14 Oct 2026): the role is the lead's, a measure taken on a Mac names it (the M4, the M1).
# History (the task board's list of where tasks ran) is exempt.
if git grep -qniI -E 'build[ -]mac' $head -- . ':(exclude)tools/devtools/schema.mjs'; then
  echo "the old name of the lead's role is back (the role is \"the lead\"; a measure names its Mac, the M4 or the M1):"
  git grep -niI -E 'build[ -]mac' $head -- . ':(exclude)tools/devtools/schema.mjs' | head -20
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
# The fonts each version serves, from this checkout's data/fonts (not in git: scripts/fonts.sh makes
# them), every glyph range there before anything is built.
bash $repo/scripts/fonts.sh --check $repo/data/fonts || exit 1
cache=~/Library/Caches/scenic-publish
mkdir -p $cache
step snapshot
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
step tools
# Rust (rust-toolchain.toml's), and Node, pnpm and cargo-nextest at mise.toml's versions, installed
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
probe=$sb/probe; cp /usr/bin/true $probe
codesign -s "$sign_id" --force $probe </dev/null || { echo "publish: codesign can't use the \"$sign_id\" key: unlock the login keychain and let codesign use the key (docs/building.md, App publishing)"; exit 1; }
# The guards first (tools/app/guards.sh, docs/inputs.md §7): the inputs folders read only through their
# accessor, no data source named in the app's code, and the Python steps loaded from the app's dem/
# in a stand-in NAS root whose inputs folders can't be read (and checked by pyflakes).
step guards
zsh tools/app/guards.sh || exit 1
step "release build"
dj_wrap cargo "cargo build --release" --parse cargo
$reply cargo build --release -p server -p pipeline 2>&1 | tail -2
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
# The release server on a fixture map (crates/server/tests/serves.rs: every route the app starts with
# and a tile of each kind, the program on a free port, no NAS), run against target/release/server,
# the program signed and copied below: the suite runs it against the test profile's, so a fault only
# an optimized build shows stops the publish here, before the suite (the test itself takes under
# a second; its build is the test profile's of the server and the crates it uses). The test's own binary run directly,
# with CARGO_BIN_EXE_server naming the release program: cargo and nextest set that variable as they
# run a test, to the program they built beside it. Built as the suite builds it (the workspace's
# features, the test profile): when this is the test Mac the suite's build reuses it, and a test
# build changes nothing in target/release. (Its output the log's first part, the suite's after it.)
step "release server"
serves=$(cargo test --workspace --test serves --no-run --message-format=json-render-diagnostics 2>>$tests_log \
  | python3 -c 'import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "serves" and m.get("executable"): print(m["executable"])') || true
[[ -x $serves ]] || { tail -40 $tests_log; echo "the server's fixture-map test didn't build; whole output: $tests_log"; exit 1; }
CARGO_BIN_EXE_server=$PWD/target/release/server $serves >> $tests_log 2>&1 \
  || { tail -40 $tests_log; echo "the release server failed on the fixture map; whole output: $tests_log"; exit 1; }
# The tests build without debug info (task #660; Cargo.toml's [profile.test] since #918): on macOS
# each build's line tables stay in its object files beside the binaries (split-debuginfo
# "unpacked"), 80 GB of the 122 GB this tree's target/debug had grown to in 3.5 days on the mini
# (25 Oct). A failed test still says its file and line (the panic's location needs none).
# (The whole workspace's tests, #848: every crate's libraries, binaries and integration tests, by
# nextest's own discovery (#662: a list of four had missed no_instant_back, cli, grant_rules and
# whole_kill as they were added): every cache read
# through the accessor, the records written only through Out (docs/pool.md §7.3), every Wikimedia
# request through the budgets' proxy (§7.7), the Python steps' timer, no Instant dated back...)
# Both run on the test Mac, the M4 by default (tools/app/remote-tests.sh, #442: an exact copy of this snapshot, in a test
# slot's build folder kept there, at nice 20; here when this is the test Mac or with
# SCENIC_TESTS_LOCAL=1; kept from it otherwise, not run, a failure): the suite took 13-30 min on the M1 beside its builds, and a
# timing test failed under that load. Its exit status is the suite's.
suite='export SCENIC_PUBLISH=1; cargo nextest --version >/dev/null 2>&1 || { mise install -q; eval "$(mise env -s zsh)"; }; rc=0
cargo nextest run --no-fail-fast --hide-progress-bar --workspace || rc=$?
exit $rc'
# One publish's suite and clippy at a time on this Mac (two at once overload the test Mac, #684): the
# second waits for the first's (tests.busy).
step "wait for the other publish's tests"
until hold $cache/tests.busy; do
  [[ -n ${said_wait:-} ]] || { echo "waiting for the other publish's tests ($lock_held)"; said_wait=1; }
  sleep 10
done
cleanup+=("unhold $cache/tests.busy")
step "Rust tests"
dj_attr $djrun tests_log $tests_log
zsh tools/app/remote-tests.sh publish $snap $suite >> $tests_log 2>&1 || tests_ok=$?
# Then every test outside the Rust workspace (tools/app/other-tests.zsh, #848: web/test and
# tools/check/*.test.mjs by glob, the dev tools app's, the dev jobs' recorder's, the status app's
# Swift self-test, shell.zsh and test-guard.zsh; their parts at once, ~1 min, most of it swiftc),
# here, after the Rust suite rather
# than beside it: on the mini the suite runs on this same Mac. Their whole output follows the
# suite's in the log; a failing part fails the publish.
other_ok=0
step "other tests" $(( tests_ok ? 1 : 0 ))
zsh tools/app/other-tests.zsh >> $tests_log 2>&1 || other_ok=$?
# (And every child, listener and taken connection by store::spawn: clippy's disallowed-methods,
# clippy.toml, docs/pool.md §7.5. A hit fails the publish.)
step clippy $(( other_ok ? 1 : 0 ))
dj_wrap cargo "cargo clippy" --parse cargo
$reply cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods >> $tests_log 2>&1 || { tests_ok=$?; grep -A6 'disallowed' $tests_log; }
unhold $cache/tests.busy
grep -E '^ *Summary |^remote-tests: NOT RUN: ' $tests_log || true
grep -E '^other tests: ' $tests_log || true
(( other_ok == 0 )) || { (( tests_ok != 0 )) || tests_ok=$other_ok; }
if (( tests_ok != 0 )); then
  sed -n '/^ *FAIL \[/,/^ *\(PASS\|SLOW\|FAIL\|Summary\) /p' $tests_log | grep -v '^progress: ' || true
  echo "tests failed; their whole output: $tests_log"
  exit $tests_ok
fi
# The build folder kept small (task #660): cargo never deletes what it built, so each change of the
# dependencies or a profile left every workspace crate's old build behind (the mini's target/ grew
# by ~35 GB a day, to 127 GB). What these builds use is read at each publish, so only an entry a
# later publish needs again is built again (tools/app/lib.zsh's prune_target).
step prune
prune_target
step "web build"
# Built into its own folder: web/dist may be what a development server is serving. (The type check
# writes and checks our MapLibre's declarations, then the app's; the build compiles its source with
# the app: web/maplibre/FORK.md.)
(cd web && pnpm run typecheck && pnpm exec vite build --outDir dist-publish --emptyOutDir >/dev/null) || { echo "web build failed"; exit 1; }
version=$(date -u +%Y%m%d-%H%M)-$head
step "sign and bundle"
# The programs that listen (the map server; the agent, whose coordinator and broker members reach)
# signed with the owner's certificate when this Mac has it, under fixed identifiers, so a Mac's
# firewall allow outlives an update (an ad-hoc signature is the build's own hash, so its allow went
# with it). Signed here before the copy: codesign refuses files on the NAS.
codesign -s "$sign_id" -i local.scenic.server --force target/release/server
codesign -s "$sign_id" -i local.scenic.agent --force target/release/scenic
# Scenic.app (tools/status: the menu bar item and the build service), built
# and signed on this Mac (codesign refuses a bundle on the NAS, whose SMB share adds Finder info),
# then copied without extended attributes (none to keep as AppleDouble files). Each Mac's copy in
# /Applications replaces itself with it.
zsh tools/status/bundle.sh $sb $version "$sign_id" >/dev/null
step stage
# The app's folder put together here ($sb/app): the server, and the agent with the programs its
# jobs run (each Mac runs them from there): the pipeline's binaries, and the Python steps (dem/, run
# with uv) with their lock file. (The step programs a job or task runs are scenic-build's tools:
# `scenic-build tool <name>`, #917.) Every copy leaves the extended attributes behind (cp -X, and
# the archives below): the NAS refuses macOS's provenance one when it differs from the folder's.
app=$sb/app
mkdir -p $app/web $app/fonts $app/dem $sb/send
cp -X target/release/$^app_programs $app/
git ls-files dem | while read f; do cp -X "$f" "$app/$f"; done
cp -RX "$sb/Scenic.app" "$app/"  # (Scenic.app, signed above)
rsync -a web/dist-publish/ $app/web/
# The fonts: this checkout's data/fonts (checked whole before the build, below `repo=`).
rsync -a $repo/data/fonts/ $app/fonts/
# The services the Mac mini runs beside the app, the dev tools and Caddy (lib.zsh's services_stage):
# its agent installs them as it starts on this version (tools/app/services.zsh).
services_stage $PWD $app/services || { echo "publish: the services didn't stage"; exit 1; }
# The version's archive (its one file on the NAS, app/releases/<version>.tgz) and its manifest
# (app/releases/current.json's content: every file's SHA-256 and the archive's), made here from what
# is sent: the NAS's Mac checks what it received against it. One stream, its size and SHA-256
# checked on arrival.
app_pack $app $sb/send/app.tgz
app_manifest $app $version $sb/send/app.tgz > $sb/send/manifest.json
COPYFILE_DISABLE=1 tar -cf $sb/upload.tar --no-xattrs --no-acls --no-fflags --no-mac-metadata -C $sb/send app.tgz manifest.json
dj_attr $djrun upload_bytes $(stat -f %z $sb/upload.tar)
# Live on the lead (lib.zsh's lead_install, through this snapshot's own `scenic releases`): the lead's
# releases read, the order checked, the release uploaded and put live from the version read (the
# lead checks the archive and every file against the manifest); read and pushed again when another
# publish or a rollback went in meanwhile. A lead on an app before #1118 keeps none: then the NAS's
# alone (its first release with this app installed through the NAS, as before).
step lead
lead_rc=0
lead_install $PWD/target/release/scenic $sb/send/app.tgz $sb/send/manifest.json $head "$lead_began" $sb/lead || lead_rc=$?
case $lead_rc in
  0) lead_state=live; echo "live on the lead: $version" ;;
  4) echo "the lead keeps no releases (an app before #1118): the NAS's alone" ;;
  *) exit 1 ;;
esac
# Rollout (docs/building.md, The NAS's app/ until then): the NAS's app/ too, for the updaters before
# #1118. The install, on the NAS's Mac (lib.zsh's nas_send and nas_install): the upload received whole and
# checked, the order checked, the archive written beside its name; then under the lock on the NAS,
# the one part two publishes can't run at once (waited for while another publish installs; one
# older than six hours, or whose process is gone, taken over; freed only by its holder), the order
# checked again (the publish that went in meanwhile may be newer, or a rollback since); then on this
# side's go-ahead, app/releases/<version>.tgz, previous.json and current.json.
# Nothing goes in on a failure before the go-ahead.
step install
nas_send $NAS $version $head "$began" $sb/upload.tar "$from" || exit $?
published=$version
echo "published $version"
