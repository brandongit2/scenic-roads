#!/bin/zsh
# The checks an agent passes before reporting (tools/dev/testing.md; task #273). Run it in your
# worktree, after committing:
#   tools/dev/check.sh
# It checks the branch's change against main (merge-base..working tree, untracked files included):
#   fmt       cargo fmt --check (and tools/bldfield's)
#   clippy    publish's line: cargo clippy -q --workspace --all-targets -- -A clippy::all
#             -D clippy::disallowed_methods (processes only through store::spawn; clippy.toml)
#   guards    tools/app/guards.sh
#   wasm      tools/app/wasm.sh, when a crate the wasm build compiles changed (det, roadcore,
#             pipeline, store, names)
#   web       web/test/rings.test.ts, chartmath.test.mjs and tools/check/worksums.test.mjs under node, pnpm typecheck and vite build (to a scratch folder), when web/ changed
#   swift     swiftc of tools/status/main.swift (publish's line) and its --test (arguments, reopen, launchd), when tools/status changed
#   tests     each command tools/dev/affected-tests.sh prints, run on the test Mac (tools/app/remote-tests.sh;
#             here when it can't be reached or SCENIC_CHECK_LOCAL=1)
#   python    pyflakes on the changed .py files (a deleted one's: the rest of its folder's) and
#             unittest in their folders' test_*.py, when producers/ or dem/ changed
# Prints PASS/FAIL/skip per part; each part's whole output is kept in
# ~/Library/Logs/scenic/check-<time>/. When every part passes on a clean tree it records HEAD in
# $(git rev-parse --git-common-dir)/check-passed, which the SubagentStop hook
# (tools/dev/report-guard.sh) and tools/dev/merge-ok.sh read. Exit 0 only when all passed.
set -uo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) sed -n '2,20p' ${0:A} | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) print -u2 "check.sh takes no arguments (check.sh --help)"; exit 2 ;;
esac
root=$(git rev-parse --show-toplevel) || exit 2
cd $root
eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
logs=~/Library/Logs/scenic/check-$(date +%Y%m%d-%H%M%S)-$$
mkdir -p $logs
base=$(git merge-base main HEAD)
changed=( ${(f)"$(git diff --name-only $base; git ls-files --others --exclude-standard)"} )
has() { local p; for p in $changed; do [[ $p == ${~1} ]] && return 0; done; return 1 }
typeset -a summary; failed=0
part() {  # part <name> <command...>: runs it, its output to $logs/<name>.log
  local name=$1; shift
  print -n -- "$name ... "
  if "$@" > $logs/$name.log 2>&1; then summary+="PASS  $name"; print PASS
  else summary+="FAIL  $name  ($logs/$name.log)"; print FAIL; failed=1
    tail -25 $logs/$name.log | sed 's/^/    /'; fi
}
skip() { summary+="skip  $1  ($2)" }

part fmt zsh -c 'cargo fmt --check && cargo fmt --check --manifest-path tools/bldfield/Cargo.toml'
part clippy cargo clippy -q --workspace --all-targets -- -A clippy::all -D clippy::disallowed_methods
part guards tools/app/guards.sh
if has 'crates/(det|roadcore|pipeline|store|names)/*' || has Cargo.lock; then part wasm tools/app/wasm.sh
else skip wasm "no wasm crate changed"; fi
if has 'web/*' || has 'tools/check/worksums.test.mjs'; then
  part web zsh -c "cd web && node test/rings.test.ts && node test/chartmath.test.mjs && node --test ../tools/check/worksums.test.mjs && pnpm run typecheck && pnpm exec vite build --outDir ${(q)logs}/web-dist --emptyOutDir"
else skip web "web/ unchanged"; fi
if has 'tools/status/*'; then
  part swift zsh -c "swiftc -O -swift-version 5 -target arm64-apple-macos26.0 -o ${(q)logs}/scenic-status tools/status/*.swift && ${(q)logs}/scenic-status --test tools/status/test"
else skip swift "tools/status unchanged"; fi
tests=( ${(f)"$(${0:A:h}/affected-tests.sh | grep -v '^#')"} )
# The tests run on the test Mac (tools/app/remote-tests.sh, #442: an exact copy of this tree, its own
# build folder per worktree, three runs at a time there, the rest queued); here on the test Mac itself
# (the publishing Mac), when it is down, or with SCENIC_CHECK_LOCAL=1.
rt=$root/tools/app/remote-tests.sh
if (( ${#tests} )) && [[ -x $rt ]]; then
  key=${root:t}; [[ $root == */.claude/worktrees/* ]] || key=main-checkout
  part tests $rt $key $root "set -e; ${(j:; :)tests}"
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
print -r -- "$(git rev-parse HEAD) $(date -u +%FT%TZ)" >> $(git rev-parse --path-format=absolute --git-common-dir)/check-passed
print "PASS: recorded $(git rev-parse --short HEAD) as checked"
