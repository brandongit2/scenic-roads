#!/bin/zsh
# Tests of tools/app/lib.zsh, the shell steps publish.sh sources, each in a scratch folder (nothing
# of the NAS, app/ or target/ touched). Run by tools/dev/check.sh when tools/app changes:
#   zsh tools/app/test/shell.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
lib=${0:A:h:h}/lib.zsh
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
failed=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1: $2"; failed=1 }

# at_end: each way a script under errexit can end gives its last line and its cleanup.
ending() {  # <name> <expected status> <script body>
  local s=$tmp/end-$1.zsh out rc
  print -r -- "set -euo pipefail
source ${(q)lib}
touch $tmp/lock-$1
done_() { local rc=\$?; rm -f $tmp/lock-$1; echo \"last: \$rc\"; return \$rc; }
at_end done_
$3" > $s
  out=$(zsh $s 2>&1); rc=$?
  local last=${out##*$'\n'}
  if (( rc != $2 )); then no "at_end $1" "exit $rc, not $2 (output: $out)"
  elif [[ $last != "last: $2" ]]; then no "at_end $1" "last line ${(qq)last}"
  elif [[ $(grep -c '^last: ' <<< $out) != 1 ]]; then no "at_end $1" "ran more than once: $out"
  elif [[ -e $tmp/lock-$1 ]]; then no "at_end $1" "cleanup didn't run"
  else ok "at_end $1"; fi
}
ending success 0 'echo hi'
ending explicit-exit 3 'exit 3'
ending errexit-top 1 'false; echo no'
ending errexit-in-function 1 'f() { false; echo no; }; f; echo no'
ending errexit-nested-function 4 'g() { return 4; }; f() { g; echo no; }; f; echo no'
ending subshell 5 '(exit 5); echo no'
ending subshell-in-function 5 'f() { (exit 5); echo no; }; f'
ending command-substitution 1 'f() { local x; x=$(false); echo no; }; f'
ending pipe 1 'f() { false | cat; echo no; }; f'
ending term 130 'f() { kill -TERM $$; sleep 2; echo no; }; f'
ending exit-in-function 7 'f() { exit 7; }; f'
# A failure errexit lets through (a condition, `|| true`) neither ends it nor runs at_end early.
ending tolerated 0 'f() { false; echo inside; }; f || true; if f; then :; fi; ! false; x=$(false || true); echo fine'

# found: nothing found where the start folder is missing, without failing under errexit.
out=$(zsh -euo pipefail -c "source ${(q)lib}; x=\$(found $tmp/missing -type f); print -r -- \"[\$x]\"" 2>&1)
[[ $out == '[]' ]] && ok "found missing folder" || no "found missing folder" "$out"

# prune_target: a stand-in target/ with old and fresh entries, a profile without deps/
# (target/tmp, which ended the 25 Oct publishes) and a WebAssembly triple.
t=$tmp/target
old() { mkdir -p ${1:h}; print x > $1; touch -t 202601010000 $1 }  # read and written long ago
new() { mkdir -p ${1:h}; print x > $1 }
old $t/release/deps/libold.rlib
new $t/release/deps/libnew.rlib
old $t/release/build/oldcrate-1/out/a
old $t/release/build/mixed-2/out/a; new $t/release/build/mixed-2/out/b
old $t/debug/.fingerprint/gone-3/x
new $t/debug/incremental/kept-4/x
mkdir -p $t/tmp/scratch
old $t/wasm32-wasip1/release/deps/old.wasm
new $t/wasm32-wasip1/release/deps/new.wasm
out=$(zsh -euo pipefail -c "source ${(q)lib}; prune_target ${(q)t}; echo done" 2>&1); rc=$?
gone=($t/release/deps/libold.rlib $t/release/build/oldcrate-1 $t/debug/.fingerprint/gone-3 $t/wasm32-wasip1/release/deps/old.wasm)
kept=($t/release/deps/libnew.rlib $t/release/build/mixed-2/out/a $t/debug/incremental/kept-4 $t/tmp/scratch $t/wasm32-wasip1/release/deps/new.wasm)
left=(${^gone}(N)); lost=(); for f in $kept; do [[ -e $f ]] || lost+=($f); done
if (( rc )) || [[ $out != *$'\n'done ]]; then no prune_target "ended early (exit $rc): $out"
elif (( $#left )); then no prune_target "kept old: $left"
elif (( $#lost )); then no prune_target "removed fresh: $lost"
elif [[ $out != 'build folder: '*' MB of old builds removed, '*' left'* ]]; then no prune_target "said: $out"
else ok prune_target; fi
# And with nothing to prune, or no target/ at all.
out=$(zsh -euo pipefail -c "source ${(q)lib}; prune_target $tmp/none; echo done" 2>&1)
[[ $out == *$'\n'done ]] && ok "prune_target no target" || no "prune_target no target" "$out"

exit $failed
