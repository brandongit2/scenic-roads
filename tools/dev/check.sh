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
#   taskboard node --test tools/taskboard/test/*.test.mjs, when tools/taskboard changed
#   shell     tools/app/test/shell.zsh (publish.sh's shell steps, tools/app/lib.zsh) and remote-tests.zsh (the test Mac's disk room), when tools/app changed
#   testguard tools/dev/test/test-guard.zsh (the test guard's worktree lookup), when it changed
#   reuse     tools/dev/test/check-reuse.zsh (the reuse rule, below), when it or what it tests changed
#   sharedtarget tools/dev/test/shared-target.zsh (tools/dev/bin/cargo on a toy workspace), when either changed
#   sweep     tools/dev/test/sweep-targets.zsh (the sweep's room, on a fake disk), when the sweep, its
#             test or the settings holding its default margin changed
#   swift     swiftc of tools/status/main.swift (publish's line) and its --test (arguments, reopen, launchd), when tools/status changed
#   tests     each command tools/dev/affected-tests.sh prints, run on the test Mac (tools/app/remote-tests.sh;
#             here when it can't be reached or SCENIC_CHECK_LOCAL=1)
#   python    pyflakes on the changed .py files (a deleted one's: the rest of its folder's) and
#             unittest in their folders' test_*.py, when producers/ or dem/ changed
# Its cargo is tools/dev/bin/cargo: in an agent worktree, this Mac's shared build folder, one build
# at a time (#915): a wait for another worktree's build is printed here as it happens (SCENIC_CARGO_SAY).
# Prints PASS/FAIL/REUSE/skip per part, with its wall and CPU time on this Mac (tests: the test Mac's
# too); each part's whole output is kept in ~/Library/Logs/scenic/check-<time>/. A clean checkout
# whose tree and merge-base tree a pass already recorded passes at once (no part runs again).
# A rebased branch (#950): on a clean checkout, clippy and the tests part are reused from an earlier
# pass when tools/dev/check-reuse.py shows it holds (this tree is that pass's plus main's changes
# since its base, and those touch none of the part's inputs); the tests reading main's changes at
# run time run again. A REUSE line names the pass and says why; otherwise a line says why not.
# When every part passes (or is reused) on a clean tree it records HEAD (and, nothing untracked,
# its tree key, base, test set and what it reused) in $(git rev-parse --git-common-dir)/check-passed,
# which the SubagentStop hook (tools/dev/report-guard.sh) and tools/dev/merge-ok.sh read. Exit 0 only
# when all passed.
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
base=$(git merge-base main HEAD)
changed=( ${(f)"$(git diff --name-only $base; git ls-files --others --exclude-standard)"} )
# A tree already checked is not checked again (#887): a clean checkout (no untracked files either)
# whose tree and merge-base's tree a pass recorded passes at once, recording HEAD with that pass's
# fields and `reused all=<its commit>`. (Every part reads only the tree and its diff against that base.)
passed=$(git rev-parse --path-format=absolute --git-common-dir)/check-passed
treekey="tree $(git rev-parse 'HEAD^{tree}') base $(git rev-parse "$base^{tree}")"
# (Clean at the start: a file untracked or edited during the run and gone by its end was in the
# check, so the end's state alone can't vouch for the tree.)
[[ -z $(git status --porcelain) ]] || treekey=
if [[ -n $treekey ]] && same=$(grep -F -- " $treekey" $passed 2>/dev/null | tail -1) && [[ -n $same ]]; then
  print -r -- "$(git rev-parse HEAD) $(date -u +%FT%TZ) ${${same#* }#* } reused all=${same%% *}" >> $passed
  print "PASS: this tree was already checked (against the same base) at ${same:0:7}: recorded $(git rev-parse --short HEAD) as checked"
  exit 0
fi
has() { local p; for p in $changed; do [[ $p == ${~1} ]] && return 0; done; return 1 }
typeset -a summary; failed=0
part() {  # part <name> <command...>: runs it, its output to $logs/<name>.log, with its wall and CPU time
  local name=$1 rc t; shift
  print -n -- "$name ... "
  # (zsh's time of the subshell: wall, and user + sys CPU of all it ran on this Mac.)
  t=$( { TIMEFMT='%E %U %S'; time ( "$@" > $logs/$name.log 2>&1 ) } 2>&1 ); rc=$?
  t=( ${${=t}%s} 0 0 0 ); t=$(printf '%.0f s, cpu %.0f s' ${t[1]} $(( t[2] + t[3] )))
  if (( rc == 0 )); then summary+="PASS  $name  ($t)"; print "PASS  ($t)"
  else summary+="FAIL  $name  ($t; $logs/$name.log)"; print "FAIL  ($t)"; failed=1
    tail -25 $logs/$name.log | sed 's/^/    /'; fi
}
skip() { summary+="skip  $1  ($2)" }
reused=
reuse() {  # reuse <name> <commit> <why>: the part's pass at <commit> holds here
  summary+="REUSE $1  (passed at ${2:0:7}: $3)"; reused+=" reused $1=$2"
  print -r -- "$1 ... REUSE  (passed at ${2:0:7}: $3)"
}
tests=( ${(f)"$(${0:A:h}/affected-tests.sh | grep -v '^#')"} )
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
  part clippy cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods; fi
part guards env SCENIC_GUARDS_IN_CHECK=1 tools/app/guards.sh
if has 'web/*' || has 'tools/check/*.test.mjs'; then
  webtests=( web/test/*.test.(ts|mjs)(N) tools/check/*.test.mjs(N) )
  part web zsh -c "tools/dev/web-deps.sh && node --experimental-transform-types --no-warnings --test ${(j: :)${(@q)webtests}} && cd web && pnpm run typecheck && pnpm exec vite build --outDir ${(q)logs}/web-dist --emptyOutDir"
else skip web "web/ unchanged"; fi
if has 'tools/taskboard/*'; then part taskboard zsh -c "cd tools/taskboard && node --test test/*.test.mjs"
else skip taskboard "tools/taskboard unchanged"; fi
if has 'tools/app/*'; then part shell zsh -c 'zsh tools/app/test/shell.zsh && zsh tools/app/test/remote-tests.zsh'
else skip shell "tools/app unchanged"; fi
if has 'tools/dev/test-guard*' || has 'tools/dev/test/test-guard.zsh'; then part testguard zsh tools/dev/test/test-guard.zsh
else skip testguard "tools/dev/test-guard unchanged"; fi
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
  if (( ${#tests} )) && [[ -x $rt ]]; then
    key=${root:t}; [[ $root == */.claude/worktrees/* ]] || key=main-checkout
    part tests $rt $key $root "set -e; ${(j:; :)tests}"
    # (The test Mac's own wall and CPU, which this Mac's time of the part doesn't count.)
    r=$(grep -h '^remote-tests: the run took' $logs/tests.log | tail -1)
    [[ -n $r ]] && summary[-1]+="  [${r#remote-tests: }]"
  elif (( ${#tests} )); then part tests zsh -c "set -e; ${(j:; :)tests}"
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

print "\n== check.sh summary ($(git rev-parse --short HEAD)) =="
print -l -- $summary
if (( failed )); then print "FAIL: fix the parts above before reporting"; exit 1; fi
if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
  print "PASS, but uncommitted changes: commit them and run again (the pass is recorded for a commit)"
  exit 1
fi
# (The tree key only when the checkout was clean at the start, still is, and HEAD's tree is the one
# checked: a commit made during the run gets no key, nor what it reused.) With it, the base commit
# and the hash of the test commands, which a later rebase's reuse compares (tools/dev/check-reuse.py).
[[ -z $(git status --porcelain) && $treekey == "tree $(git rev-parse 'HEAD^{tree}') "* ]] || treekey=
if [[ -n $treekey ]]; then
  treekey+=" at $base"
  [[ -s $logs/tests-commands ]] && treekey+=" tests $(shasum $logs/tests-commands | cut -d' ' -f1)"
  treekey+=$reused
fi
print -r -- "$(git rev-parse HEAD) $(date -u +%FT%TZ)${treekey:+ $treekey}" >> $passed
print "PASS: recorded $(git rev-parse --short HEAD) as checked"
