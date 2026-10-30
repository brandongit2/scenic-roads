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
#   wasm      tools/app/wasm.sh, when a crate the wasm build compiles changed (det, roadcore,
#             pipeline, store, names)
#   web       tools/dev/web-deps.sh (web/node_modules from pnpm's store, when missing or stale), node --test of web/test/*.test.{ts,mjs} and tools/check/*.test.mjs (by glob), pnpm
#             typecheck and vite build (to a scratch folder), when web/ or a tools/check test changed
#   taskboard node --test tools/taskboard/test/*.test.mjs, when tools/taskboard changed
#   shell     tools/app/test/shell.zsh (publish.sh's shell steps, tools/app/lib.zsh), when tools/app changed
#   testguard tools/dev/test/test-guard.zsh (the test guard's worktree lookup), when it changed
#   swift     swiftc of tools/status/main.swift (publish's line) and its --test (arguments, reopen, launchd), when tools/status changed
#   tests     each command tools/dev/affected-tests.sh prints, run on the test Mac (tools/app/remote-tests.sh;
#             here when it can't be reached or SCENIC_CHECK_LOCAL=1)
#   python    pyflakes on the changed .py files (a deleted one's: the rest of its folder's) and
#             unittest in their folders' test_*.py, when producers/ or dem/ changed
# Prints PASS/FAIL/skip per part, with its wall and CPU time on this Mac (tests: the test Mac's too);
# each part's whole output is kept in ~/Library/Logs/scenic/check-<time>/. A clean checkout whose
# tree and merge-base tree a pass already recorded passes at once (no part runs again).
# When every part passes on a clean tree it records HEAD (and, nothing untracked, its tree key) in
# $(git rev-parse --git-common-dir)/check-passed, which the SubagentStop hook
# (tools/dev/report-guard.sh) and tools/dev/merge-ok.sh read. Exit 0 only when all passed.
set -uo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) sed -n '2,25p' ${0:A} | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) print -u2 "check.sh takes no arguments (check.sh --help)"; exit 2 ;;
esac
root=$(git rev-parse --show-toplevel) || exit 2
cd $root
eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
logs=~/Library/Logs/scenic/check-$(date +%Y%m%d-%H%M%S)-$$
mkdir -p $logs
base=$(git merge-base main HEAD)
changed=( ${(f)"$(git diff --name-only $base; git ls-files --others --exclude-standard)"} )
# A tree already checked is not checked again (#887): a clean checkout (no untracked files either)
# whose tree and merge-base's tree a pass recorded passes at once, recording HEAD. (Every part reads
# only the tree and its diff against that base.)
passed=$(git rev-parse --path-format=absolute --git-common-dir)/check-passed
treekey="tree $(git rev-parse 'HEAD^{tree}') base $(git rev-parse "$base^{tree}")"
# (Clean at the start: a file untracked or edited during the run and gone by its end was in the
# check, so the end's state alone can't vouch for the tree.)
[[ -z $(git status --porcelain) ]] || treekey=
if [[ -n $treekey ]] && grep -qF -- " $treekey" $passed 2>/dev/null; then
  print -r -- "$(git rev-parse HEAD) $(date -u +%FT%TZ) $treekey" >> $passed
  print "PASS: this tree was already checked (against the same base): recorded $(git rev-parse --short HEAD) as checked"
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

part fmt zsh -c 'cargo fmt --check && cargo fmt --check --manifest-path tools/bldfield/Cargo.toml'
part clippy cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods
part guards env SCENIC_GUARDS_IN_CHECK=1 tools/app/guards.sh
if has 'crates/(det|roadcore|pipeline|store|names)/*' || has Cargo.lock; then part wasm tools/app/wasm.sh
else skip wasm "no wasm crate changed"; fi
if has 'web/*' || has 'tools/check/*.test.mjs'; then
  webtests=( web/test/*.test.(ts|mjs)(N) tools/check/*.test.mjs(N) )
  part web zsh -c "tools/dev/web-deps.sh && node --experimental-transform-types --no-warnings --test ${(j: :)${(@q)webtests}} && cd web && pnpm run typecheck && pnpm exec vite build --outDir ${(q)logs}/web-dist --emptyOutDir"
else skip web "web/ unchanged"; fi
if has 'tools/taskboard/*'; then part taskboard zsh -c "cd tools/taskboard && node --test test/*.test.mjs"
else skip taskboard "tools/taskboard unchanged"; fi
if has 'tools/app/*'; then part shell zsh tools/app/test/shell.zsh
else skip shell "tools/app unchanged"; fi
if has 'tools/dev/test-guard*' || has 'tools/dev/test/*'; then part testguard zsh tools/dev/test/test-guard.zsh
else skip testguard "tools/dev/test-guard unchanged"; fi
if has 'tools/status/*'; then
  part swift zsh -c "swiftc -O -swift-version 5 -target arm64-apple-macos26.0 -o ${(q)logs}/scenic-status tools/status/*.swift && ${(q)logs}/scenic-status --test tools/status/test"
else skip swift "tools/status unchanged"; fi
tests=( ${(f)"$(${0:A:h}/affected-tests.sh | grep -v '^#')"} )
# The tests run on the test Mac (tools/app/remote-tests.sh, #442: an exact copy of this tree, its own
# build folder per worktree, three runs at a time there, the rest queued); here on the test Mac itself
# (the publishing Mac), when it is down, or with SCENIC_CHECK_LOCAL=1.
rt=$root/tools/app/remote-tests.sh
# Test builds without debug info, as publish's (#660, #887): less to write and link; a failed test
# still names its file and line.
nodebug='export CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_TEST_DEBUG=false'
if (( ${#tests} )) && [[ -x $rt ]]; then
  key=${root:t}; [[ $root == */.claude/worktrees/* ]] || key=main-checkout
  part tests $rt $key $root "$nodebug; set -e; ${(j:; :)tests}"
  # (The test Mac's own wall and CPU, which this Mac's time of the part doesn't count.)
  r=$(grep -h '^remote-tests: the run took' $logs/tests.log | tail -1)
  [[ -n $r ]] && summary[-1]+="  [${r#remote-tests: }]"
elif (( ${#tests} )); then part tests zsh -c "set -e; ${(j:; :)tests}"
else skip tests "no affected Rust tests"; fi
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
# checked: a commit made during the run gets no key.)
[[ -z $(git status --porcelain) && $treekey == "tree $(git rev-parse 'HEAD^{tree}') "* ]] || treekey=
print -r -- "$(git rev-parse HEAD) $(date -u +%FT%TZ)${treekey:+ $treekey}" >> $passed
print "PASS: recorded $(git rev-parse --short HEAD) as checked"
