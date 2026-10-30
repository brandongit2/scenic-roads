#!/bin/zsh
# Every test outside the Rust workspace, for publish.sh (task #848; tools/dev/testing.md): run in the
# tree it is started in (publish's snapshot), its parts at once, each part's whole output printed
# after all have ended (in this order, nothing filtered), then one "other tests: <part> ok|FAIL" line
# per part. Exit 0 only when every part passed.
#   js         node --test of web/test/*.test.{ts,mjs} and tools/check/*.test.mjs (by glob: a new
#              test file runs without a list to edit), types stripped (web/node_modules installed)
#   taskboard  node --test tools/taskboard/test/*.test.mjs
#   swift      swiftc of tools/status/*.swift and its --test tools/status/test
#   shell      tools/app/test/shell.zsh
#   testguard  tools/dev/test/test-guard.zsh
set -u
cd ${0:A:h}/../..
out=$(mktemp -d "${TMPDIR:-/tmp}/scenic-other-tests.XXXXXX") || exit 2
trap 'rm -rf $out' EXIT
js=( web/test/*.test.(ts|mjs)(N) tools/check/*.test.mjs(N) )
typeset -A cmd
cmd[js]="node --experimental-transform-types --no-warnings --test ${(j: :)${(q)js}}"
cmd[taskboard]='cd tools/taskboard && node --test test/*.test.mjs'
cmd[swift]="swiftc -O -swift-version 5 -target arm64-apple-macos26.0 -o ${(q)out}/scenic-status tools/status/*.swift && ${(q)out}/scenic-status --test tools/status/test"
cmd[shell]='zsh tools/app/test/shell.zsh'
cmd[testguard]='zsh tools/dev/test/test-guard.zsh'
order=(js taskboard swift shell testguard)
typeset -a pids
for p in $order; do
  zsh -c "${cmd[$p]}" > $out/$p.log 2>&1 &
  pids+=($!)
done
typeset -A rc; failed=0
for i in {1..${#order}}; do
  p=${order[i]}
  wait ${pids[i]}; rc[$p]=$?
  (( rc[$p] )) && failed=1
done
for p in $order; do
  print -r -- "=== other tests: $p: ${cmd[$p]}"
  cat $out/$p.log
done
for p in $order; do
  if (( rc[$p] )); then print -r -- "other tests: $p FAIL (exit ${rc[$p]})"
  else print -r -- "other tests: $p ok"; fi
done
exit $failed
