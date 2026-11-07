#!/bin/zsh
# The checks an agent passes before reporting (tools/dev/testing.md; task #273). Run it in your
# worktree, after committing:
#   tools/dev/check.sh
# It checks the branch's change against main (merge-base..working tree, untracked files included):
#   fmt       cargo fmt --check (and tools/bldfield's)
#   clippy    publish's line: cargo clippy -q --workspace --all-targets -- -A clippy::all
#             -D clippy::disallowed_methods (processes only through store::spawn; clippy.toml)
#   guards    tools/app/guards.sh, its fmt and two tests left to the fmt and tests parts
#             (SCENIC_GUARDS_IN_CHECK=1: tools/dev/affected-tests.sh runs both whenever their files change)
#   web       tools/dev/web-deps.sh (web/node_modules from pnpm's store, when missing or stale), node --test of web/test/*.test.{ts,mjs} and tools/check/*.test.mjs (by glob), pnpm
#             typecheck and vite build (to a scratch folder), when web/ or a tools/check test changed
#   devtools  node --test tools/devtools/test/*.test.mjs (the task board's and the dev job dashboard's
#             APIs), when tools/devtools or the build page's timeline modules it serves changed
#   devjob    tools/dev/test/devjob.zsh (the dev jobs' recorder), when tools/dev/devjob* or its tests changed
#   shell     tools/app/test/shell.zsh (publish.sh's shell steps, tools/app/lib.zsh) and remote-tests.zsh (the copy to the test Mac, its disk room), when tools/app changed
#   testguard tools/dev/test/test-guard.zsh (the test guard's worktree lookup), when it or shellwords.py changed
#   gitguard  tools/dev/test/git-guard.zsh (whose checkout a git write goes to), when the git guard,
#             shell-guard.sh or shellwords.py changed
#   record    tools/dev/test/check-record.zsh (this script on a toy repo: what it records), when it changed
#   reuse     tools/dev/test/check-reuse.zsh (the reuse rule, below), when it or what it tests changed
#   sharedtarget tools/dev/test/shared-target.zsh (tools/dev/bin/cargo on a toy workspace), when either changed
#   sweep     tools/dev/test/sweep-targets.zsh (the sweep's room, on a fake disk), when the sweep, its
#             test or the settings holding its default margin changed
#   swift     swiftc of tools/status/main.swift (publish's line) and its --test (arguments, reopen, launchd), when tools/status changed
#   tests     the tests tools/dev/affected-tests.sh prints, as one `cargo nextest run` (its --nextest),
#             run on the test Mac (tools/app/remote-tests.sh; here when it can't be reached or SCENIC_CHECK_LOCAL=1)
#   python    pyflakes on the changed .py files (a deleted one's: the rest of its folder's) and
#             unittest in their folders' test_*.py, when producers/ or dem/ changed
# Its cargo is tools/dev/bin/cargo: in an agent worktree, this Mac's shared build folder, one build
# at a time (#915): a wait for another worktree's build is printed here as it happens (SCENIC_CARGO_SAY).
# Prints PASS/FAIL/REUSE/skip per part, with its wall and CPU time on this Mac (tests: the test Mac's
# too); each part's whole output is kept in ~/Library/Logs/scenic/check-<time>/. A clean checkout
# whose tree and merge-base tree a pass already recorded passes at once (no part runs again).
# A rebased branch (#950), or one edited since a pass (#969): on a clean checkout, clippy and the
# tests part are reused from an earlier pass when tools/dev/check-reuse.py shows it holds (main's
# changes since its base touch none of the part's inputs; clippy: nor the branch's own since); of
# the tests, those the changes since that pass affect run again, and those it didn't run. A REUSE
# line names the pass and says why; otherwise a line says why not.
# Each run is recorded for the dev job dashboard (tools/dev/devjob.zsh): the run, each part with what
# its output says of its compiles, a skipped or reused part as a mark; cargo and the test runs under
# their part.
# When every part passes (or is reused) on a clean checkout (nothing untracked either), and HEAD,
# the checkout's state and its files are as they were at the start (#980), it records HEAD, its tree
# key, base, test set and what it reused in $(git rev-parse --git-common-dir)/check-passed, which
# the SubagentStop hook (tools/dev/report-guard.sh) and tools/dev/merge-ok.sh read; otherwise it
# says why it records nothing. Exit 0 only when all passed and the pass is recorded.
set -uo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) sed -n '2,/^set -uo/p' ${0:A} | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) print -u2 "check.sh takes no arguments (check.sh --help)"; exit 2 ;;
esac
root=$(git rev-parse --show-toplevel) || exit 2
cd $root
eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
path=($root/tools/dev/bin $path)  # (after mise's: its PATH puts rustup's cargo first)
exec {say}>&2; export SCENIC_CARGO_SAY=/dev/fd/$say  # (the parts' output goes to their logs; a wait comes here too)
logs=~/Library/Logs/scenic/check-$(date +%Y%m%d-%H%M%S)-$$
mkdir -p $logs
# What is checked, noted at the start (#980): HEAD, its tree key, and whether the checkout is clean
# (nothing untracked either). A pass is recorded only when all three still hold at the end and no
# file of the checkout (tracked, or untracked and not ignored) was written meanwhile ($logs/start's
# time): an edit made and undone during the run, or a reset to another commit and back, was in
# what the parts read. (A check ran on while another session reset its worktree, and recorded the
# new HEAD as passed, 6 Nov.)
: > $logs/start
head0=$(git rev-parse HEAD) || exit 2
base=$(git merge-base main $head0)
treekey="tree $(git rev-parse "$head0^{tree}") base $(git rev-parse "$base^{tree}")"
clean0=$(git status --porcelain)
[[ -z $clean0 ]] || treekey=
# (How the tests build, from the environment, #969: no tree says what it held, so a pass neither
# stands for such a run nor is taken from one.)
buildenv=( ${(M)${(f)"$(env)"}:#(CARGO_PROFILE_|RUSTFLAGS=|CARGO_BUILD_RUSTFLAGS=|CARGO_ENCODED_RUSTFLAGS=)*} )
(( ! ${#buildenv} )) || treekey=
source ${0:A:h}/devjob.zsh
dj_start check "check ${root:t}" log $logs base ${base:0:7}; djrun=$REPLY djstatus=stopped djwhy=
export SCENIC_DEVJOB_PARENT=$djrun
# (zsh runs no EXIT trap on a signal: a TERM or INT ends it by exit, so the run is ended "stopped".)
trap 'dj_end $djrun $djstatus ${djwhy:+why} ${djwhy:+$djwhy}' EXIT
trap 'exit 143' TERM; trap 'exit 130' INT
changed=( ${(f)"$(git diff --name-only $base; git ls-files --others --exclude-standard)"} )
# A tree already checked is not checked again (#887): a clean checkout (no untracked files either)
# whose tree and merge-base's tree a pass recorded passes at once, recording HEAD with that pass's
# fields and `reused all=<its commit>`. (Every part reads only the tree and its diff against that base.)
passed=$(git rev-parse --path-format=absolute --git-common-dir)/check-passed
# (The pass's commit must have the tree it names: a record names the tree it checked.)
if [[ -n $treekey ]] && same=$(grep -F -- " $treekey" $passed 2>/dev/null | tail -1) && [[ -n $same ]] &&
    [[ $(git rev-parse -q --verify "${same%% *}^{tree}") == $(git rev-parse "$head0^{tree}") ]]; then
  print -r -- "$head0 $(date -u +%FT%TZ) ${${same#* }#* } reused all=${same%% *}" >> $passed
  print "PASS: this tree was already checked (against the same base) at ${same:0:7}: recorded ${head0:0:7} as checked"
  djstatus=reuse djwhy="this tree was checked at ${same:0:7}"
  exit 0
fi
has() { local p; for p in $changed; do [[ $p == ${~1} ]] && return 0; done; return 1 }
typeset -a summary; failed=0
part() {  # part [--parse <kinds>] <name> <command...>: runs it, its output to $logs/<name>.log, with its wall and CPU time
  local name rc t parse=; [[ $1 == --parse ]] && { parse=$2; shift 2 }
  name=$1; shift
  print -n -- "$name ... "
  # (A span of the run: devjob.py wrap reads its output for what --parse names; a function, run
  # here, gets one of its own.)
  local -a pre; local fid=
  if (( $+functions[$1] )); then dj_start part $name; fid=$REPLY
  else dj_wrap part $name ${parse:+--parse} ${parse:+$parse}; pre=( $reply ); fi
  # (zsh's time of the subshell: wall, and user + sys CPU of all it ran on this Mac.)
  t=$( { TIMEFMT='%E %U %S'; time ( [[ -n $fid ]] && export SCENIC_DEVJOB_PARENT=$fid; $pre "$@" > $logs/$name.log 2>&1 ) } 2>&1 ); rc=$?
  [[ -n $fid ]] && { (( rc )) && dj_end $fid fail exit $rc || dj_end $fid ok }
  t=( ${${=t}%s} 0 0 0 ); t=$(printf '%.0f s, cpu %.0f s' ${t[1]} $(( t[2] + t[3] )))
  if (( rc == 0 )); then summary+="PASS  $name  ($t)"; print "PASS  ($t)"
  else summary+="FAIL  $name  ($t; $logs/$name.log)"; print "FAIL  ($t)"; failed=1
    tail -25 $logs/$name.log | sed 's/^/    /'; fi
}
skip() { summary+="skip  $1  ($2)"; dj_span part $1 $EPOCHREALTIME $EPOCHREALTIME skip why $2 }
reused=
reuse() {  # reuse <name> <commit> <why>: the part's pass at <commit> holds here
  summary+="REUSE $1  (passed at ${2:0:7}: $3)"; reused+=" reused $1=$2"
  dj_span part $1 $EPOCHREALTIME $EPOCHREALTIME reuse why "passed at ${2:0:7}: $3"
  print -r -- "$1 ... REUSE  (passed at ${2:0:7}: $3)"
}
# (Its failure is check.sh's: an empty list would pass as "no affected Rust tests".)
affected=$(${0:A:h}/affected-tests.sh) || { print "FAIL: tools/dev/affected-tests.sh failed: no test list"; djstatus=fail; exit 1 }
tests=( ${(f)"$(print -r -- $affected | grep -v '^#')"} )
# Reuse (#950), on a clean checkout only: tools/dev/check-reuse.py's verdict for clippy and tests.
typeset -A verdict from why; typeset -a again
if [[ -n $treekey ]]; then
  if (( ${#tests} )); then print -lr -- $tests > $logs/tests-commands; else : > $logs/tests-commands; fi
  while IFS=$'\t' read -r pt v c w; do
    if [[ $v == cmd ]]; then again+=$w; else verdict[$pt]=$v from[$pt]=$c why[$pt]=$w; fi
  done < <(python3 ${0:A:h}/check-reuse.py $passed $logs/tests-commands 2>$logs/check-reuse.log)
  [[ -n ${verdict[clippy]:-} ]] || why[clippy]="tools/dev/check-reuse.py failed: $logs/check-reuse.log" why[tests]=${why[clippy]}
fi
notreused() { [[ -n ${why[$1]:-} ]] && print -r -- "$1: not reused: ${why[$1]}" }

part fmt zsh -c 'cargo fmt --check && cargo fmt --check --manifest-path tools/bldfield/Cargo.toml'
if [[ ${verdict[clippy]:-} == reuse ]]; then reuse clippy ${from[clippy]} ${why[clippy]}
else notreused clippy
  part --parse cargo clippy cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods; fi
part guards env SCENIC_GUARDS_IN_CHECK=1 tools/app/guards.sh
if has 'web/*' || has 'tools/check/*.test.mjs'; then
  webtests=( web/test/*.test.(ts|mjs)(N) tools/check/*.test.mjs(N) )
  part web zsh -c "tools/dev/web-deps.sh && node --experimental-transform-types --no-warnings --test ${(j: :)${(@q)webtests}} && cd web && pnpm run typecheck && pnpm exec vite build --outDir ${(q)logs}/web-dist --emptyOutDir"
else skip web "web/ unchanged"; fi
if has 'tools/devtools/*' || has 'web/work/(timeline|sums|chartmath).js'; then part devtools zsh -c "cd tools/devtools && node --test test/*.test.mjs"
else skip devtools "tools/devtools unchanged"; fi
if has 'tools/dev/devjob*' || has 'tools/dev/test/(devjob.zsh|test_devjob.py)'; then part devjob zsh tools/dev/test/devjob.zsh
else skip devjob "the dev jobs' recorder unchanged"; fi
if has 'tools/app/*'; then part shell zsh -c 'zsh tools/app/test/shell.zsh && zsh tools/app/test/remote-tests.zsh'
else skip shell "tools/app unchanged"; fi
if has 'tools/dev/test-guard*' || has 'tools/dev/shellwords.py' || has 'tools/dev/test/test-guard.zsh'; then part testguard zsh tools/dev/test/test-guard.zsh
else skip testguard "tools/dev/test-guard unchanged"; fi
if has 'tools/dev/(git|shell)-guard*' || has 'tools/dev/shellwords.py' || has 'tools/dev/test/git-guard.zsh'; then part gitguard zsh tools/dev/test/git-guard.zsh
else skip gitguard "the git guard unchanged"; fi
if has 'tools/dev/check.sh' || has 'tools/dev/test/check-record.zsh'; then part record zsh tools/dev/test/check-record.zsh
else skip record "check.sh unchanged"; fi
if has 'tools/dev/check*' || has 'tools/dev/affected-tests.sh' || has 'tools/dev/merge-ok.sh' || has 'tools/dev/test/check-reuse.zsh'; then
  part reuse zsh tools/dev/test/check-reuse.zsh
else skip reuse "the reuse rule unchanged"; fi
if has 'tools/dev/bin/*' || has 'tools/dev/test/shared-target.zsh'; then part sharedtarget zsh tools/dev/test/shared-target.zsh
else skip sharedtarget "tools/dev/bin unchanged"; fi
if has 'tools/dev/sweep-targets.sh' || has 'tools/dev/test/sweep-targets.zsh' || has 'crates/pipeline/src/agent/settings.rs'; then
  part sweep zsh tools/dev/test/sweep-targets.zsh
else skip sweep "the sweep unchanged"; fi
if has 'tools/status/*'; then
  part swift zsh -c "swiftc -O -swift-version 5 -target arm64-apple-macos26.0 -o ${(q)logs}/scenic-status tools/status/*.swift && ${(q)logs}/scenic-status --test tools/status/test"
else skip swift "tools/status unchanged"; fi
# The tests run on the test Mac (tools/app/remote-tests.sh, #442: an exact copy of this tree, its own
# build folder per worktree, three runs at a time there, the rest queued); here on the test Mac itself
# (the publishing Mac), when it is down, or with SCENIC_CHECK_LOCAL=1.
rt=$root/tools/app/remote-tests.sh
# (Test builds without debug info: Cargo.toml's [profile.test], #918, the same as an agent's own runs.)
if (( ${#tests} )) && [[ ${verdict[tests]:-} == reuse ]]; then reuse tests ${from[tests]} ${why[tests]}
else
  if (( ${#tests} )) && [[ ${verdict[tests]:-} == partial ]]; then
    # (Only the tests reading main's changes at run time; the pass vouches for the rest.)
    print -r -- "tests: the rest reused from ${from[tests]:0:7}: ${why[tests]}"
    reused+=" reused tests=${from[tests]}/partial"; tests=( $again )
  elif (( ${#tests} )); then notreused tests; fi
  # (Run as one `cargo nextest run`, #969: affected-tests.sh --nextest. One build of every package
  # together, each line's own had compiled the shared crates again under its own features; one pool
  # of tests across the binaries; every failure, not the first command's. Saved in tests-nextest.)
  if (( ${#tests} )); then
    print -lr -- $tests > $logs/tests-run
    nx=( ${(f)"$(${0:A:h}/affected-tests.sh --nextest $logs/tests-run)"} ) || nx=()
    print -lr -- $nx > $logs/tests-nextest
    run='cargo nextest --version >/dev/null 2>&1 || { mise install -q; eval "$(mise env -s zsh)"; }; rc=0'
    for l in $nx; do run+="; $l || rc=\$?"; done
    run+='; exit $rc'
  fi
  if (( ${#tests} )) && (( ! ${#nx} )); then
    summary+="FAIL  tests  (affected-tests.sh --nextest couldn't read $logs/tests-run)"; print "tests ... FAIL"; failed=1
  elif (( ${#tests} )) && [[ -x $rt ]]; then
    key=${root:t}; [[ $root == */.claude/worktrees/* ]] || key=main-checkout
    part tests $rt $key $root $run
    # (The test Mac's own wall and CPU, which this Mac's time of the part doesn't count.)
    r=$(grep -h '^remote-tests: the run took' $logs/tests.log | tail -1)
    [[ -n $r ]] && summary[-1]+="  [${r#remote-tests: }]"
  elif (( ${#tests} )); then part --parse cargo,tests tests zsh -c $run
  else skip tests "no affected Rust tests"; fi
  [[ $reused == *" reused tests="* ]] && summary[-1]+="  (only these; the rest reused from ${from[tests]:0:7})"
fi
py=( ${(M)changed:#(producers|dem)/*.py} ); py=( ${py:#*/.venv/*} )
# (A deleted file's folder: its remaining .py files, which may have imported it.)
kept=( ${^py}(N) ); gone=( ${py:|kept} ); py=( $kept )
for d in ${(u)gone:h}; do py+=( $d/*.py(N) ); done
py=( ${(u)py} )
if (( ${#py} )); then
  pycheck() {
    uvx --quiet --from 'pyflakes==4.0.2' pyflakes $py || return 1
    local d proj
    for d in ${(u)py:h}; do
      [[ -n $(print -l $d/test_*.py(N)) ]] || continue
      proj=${d%%/*}
      (cd $d && uv run --quiet --project $root/$proj python -m unittest discover -p 'test_*.py') || return 1
    done
  }
  part python pycheck
else skip python "no producers/ or dem/ Python changed"; fi

print "\n== check.sh summary (${head0:0:7}) =="
print -l -- $summary
djstatus=fail
if (( failed )); then print "FAIL: fix the parts above before reporting"; exit 1; fi
# (Not recorded: a pass names the tree it checked, which must be HEAD's, the same at both ends.)
notrec() { print -r -- "PASS, but NOT recorded: $1"; djwhy=$2; exit 1 }
head1=$(git rev-parse HEAD)
(( ! ${#buildenv} )) ||
  notrec "the environment sets how the tests build (${buildenv[1]%%=*} …): a pass is recorded for a tree alone" "build settings in the environment"
[[ $head1 == $head0 ]] ||
  notrec "HEAD moved during the run, from ${head0:0:7} to ${head1:0:7} (a commit, reset or checkout here): what was checked isn't HEAD. Run check.sh again, changing nothing in this checkout while it runs." "HEAD moved"
[[ -z $(git status --porcelain --untracked-files=no) ]] ||
  notrec "uncommitted changes: commit them and run again (the pass is recorded for a commit)" "uncommitted changes"
[[ -z $clean0 && -z $(git status --porcelain) ]] ||
  notrec "untracked files at the start or the end (${${(f)clean0}[1]:-$(git status --porcelain | head -1)} …): commit, ignore or delete them and run again (a pass is recorded for a tree)" "untracked files"
# (Nanoseconds: find -newer and zsh's -nt compare whole seconds, and a run can be shorter.)
written=( ${(f)"$(git ls-files -z -co --exclude-standard | python3 -c '
import os, sys
t = os.stat(sys.argv[1]).st_mtime_ns
for f in sys.stdin.read().split("\0"):
    try:
        if f and os.lstat(f).st_mtime_ns > t: print(f)
    except OSError: pass
' $logs/start | head -3)"} )
(( ! ${#written} )) ||
  notrec "files written during the run (${written[*]}): what was checked may not be what's committed. Run check.sh again, changing nothing in this checkout while it runs." "files written during the run"
# The record: HEAD, its tree key, the base commit and the hash of the test commands, which a later
# rebase's reuse compares (tools/dev/check-reuse.py), and what it reused.
treekey+=" at $base"
if [[ -s $logs/tests-commands ]]; then
  th=$(shasum $logs/tests-commands | cut -d' ' -f1); treekey+=" tests $th"
  # (The commands themselves, for a later check whose own differ: check-reuse.py runs only the
  # tests these didn't, #969. A few KB each; older than 30 days go. Shared by the Mac's
  # worktrees, so put in place whole by a rename: a check reading it meanwhile never sees it cut
  # short, and check-reuse.py takes it only when its bytes are its name's hash.)
  mkdir -p ${passed:h}/check-tests && cp $logs/tests-commands ${passed:h}/check-tests/$th.$$.tmp &&
    mv -f ${passed:h}/check-tests/$th.$$.tmp ${passed:h}/check-tests/$th
  find ${passed:h}/check-tests -type f -mtime +30 -delete 2>/dev/null
fi
print -r -- "$head0 $(date -u +%FT%TZ) $treekey$reused" >> $passed
djstatus=ok
print "PASS: recorded ${head0:0:7} as checked"
