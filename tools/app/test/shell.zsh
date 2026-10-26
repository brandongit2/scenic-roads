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
ending tolerated-lists 0 'f() { false; }; f && echo no; false || :; while f; do :; done; until ! f; do :; done; ! f; g() { f || return 0; }; g; echo fine'
ending tolerated-then-failed 6 'f() { false; }; if f; then :; fi; f || :; g() { return 6; }; g; echo no'

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

# nas_lock / nas_unlock: create-new, held by another, freed only by its holder, a stale one taken over.
nas=$tmp/nas; mkdir -p $nas/app; L=$nas/app/publish.lock
r() { zsh -euo pipefail -c "source ${(q)lib}; $1" 2>&1 }
out=$(r "nas_lock $L 'a 1'; echo rc=\$?; nas_lock $L 'b 2' || echo \"held: \$lock_held\"; nas_unlock $L 'b 2'; cat $L")
[[ $out == $'rc=0\nheld: a 1, 0 min ago\na 1' ]] && ok "nas_lock held, not freed by another" || no "nas_lock held" "$out"
out=$(r "nas_unlock $L 'a 1'; [[ ! -e $L ]] && echo freed")
[[ $out == freed ]] && ok "nas_unlock by its holder" || no "nas_unlock by its holder" "$out"
print 'old 9' > $L; touch -t 202601010000 $L
out=$(r "nas_lock $L 'c 3'; cat $L")
[[ $out == *'took over a lock'*'(old 9)'*$'\nc 3' ]] && ok "nas_lock stale taken over" || no "nas_lock stale" "$out"
rm -f $L

# hold / unhold: a live holder keeps it; a gone one's is taken over; freed only by its owner.
H=$tmp/tree.busy
zsh -c "source ${(q)lib}; hold $H; sleep 30" & hp=$!
for i in {1..50}; do [[ -s $H/owner ]] && break; sleep 0.1; done
out=$(r "hold $H || echo \"held: \$lock_held\"; unhold $H; [[ -d $H ]] && echo kept")
[[ $out == "held: process $hp "*$'\nkept' ]] && ok "hold held by a live process" || no "hold live" "$out"
kill $hp; wait $hp 2>/dev/null
out=$(r "hold $H && echo taken; unhold $H; [[ -d $H ]] || echo freed")
[[ $out == $'taken\nfreed' ]] && ok "hold taken over from a gone process" || no "hold gone" "$out"

# fast_forward: in a stand-in repository, live = b; c builds on it, a is older, d diverged.
g=$tmp/repo; git init -q $g
c() { git -C $g -c user.name=t -c user.email=t@t commit -q --allow-empty -m $1; git -C $g rev-parse --short HEAD }
A=$(c a); B=$(c b); C=$(c c); git -C $g checkout -q $A; D=$(c d)
cur=$nas/app/current.json
ff() { (cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $1" 2>&1) }
out=$(ff $A) && ok "fast_forward nothing live" || no "fast_forward nothing live" "$out"
print -r -- "{\"version\": \"20261026-0953-$B\"}" > $cur
out=$(ff $C) && ok "fast_forward newer" || no "fast_forward newer" "$out"
out=$(ff $B) && ok "fast_forward same" || no "fast_forward same" "$out"
out=$(ff $A) && no "fast_forward older" "allowed" || { [[ $out == *'older than the live version'* ]] && ok "fast_forward older refused" || no "fast_forward older" "$out"; }
out=$(ff $D) && no "fast_forward diverged" "allowed" || { [[ $out == *diverged* ]] && ok "fast_forward diverged refused" || no "fast_forward diverged" "$out"; }
print -r -- '{"version": "20261026-0953-0000000"}' > $cur
out=$(ff $C) && no "fast_forward unknown live" "allowed" || ok "fast_forward unknown live refused"

# Two publishes at once, as publish.sh composes these steps (fake steps, the stand-in NAS and
# repository): each takes a tree, the tests one at a time, the NAS lock only around its install
# with the ordering checked under it. The second (c, newer) builds beside the first (b) and goes in
# right after it; a third, of an older commit (a), is refused at its install.
rm -f $cur; cache=$tmp/cache; mkdir -p $cache; ev=$tmp/events; : > $ev
pub() {  # <name> <commit> <build seconds>
  (cd $g && zsh -c "set -euo pipefail; source ${(q)lib}
    e() { print -r -- \"$1 \$*\" >> $ev; }
    who='$1 '\$\$
    snap=; for t in tree tree-2; do hold $cache/\$t.busy && { snap=\$t; break; }; done
    [[ -n \$snap ]] || { e no-tree; exit 1; }
    e build \$snap; sleep $3; e built
    until hold $cache/tests.busy; do sleep 0.1; done; e tests; sleep 0.5; e tested; unhold $cache/tests.busy
    until nas_lock $L \"\$who\"; do sleep 0.1; done; e locked
    if ! fast_forward $cur $2 > $tmp/why-$1; then e refused; nas_unlock $L \"\$who\"; unhold $cache/\$snap.busy; exit 1; fi
    sleep 0.5; print -r -- '{\"version\": \"20261026-0953-$2\"}' > $cur; e installed
    nas_unlock $L \"\$who\"; e unlocked; unhold $cache/\$snap.busy" 2>&1)
}
pub b $B 0.3 & p1=$!
sleep 0.1; pub c $C 0.6 & p2=$!
wait $p1; r1=$?; wait $p2; r2=$?
order=$(tr '\n' ',' < $ev)
seq() { local s=$order w; for w in "$@"; do [[ $s == *"$w"* ]] || return 1; s=${s#*"$w"}; done }
if (( r1 || r2 )); then no "two publishes" "exit $r1/$r2: $order"
elif ! seq 'b build tree,' 'c build tree-2,' 'b built'; then no "two publishes build beside each other" "$order"
elif ! seq 'b tests' 'b tested' 'c tests' 'c tested'; then no "two publishes' tests one at a time" "$order"
elif ! seq 'b locked' 'b installed' 'b unlocked' 'c locked' 'c installed' 'c unlocked'; then no "two publishes install one after the other" "$order"
elif [[ $(<$cur) != *$C* ]]; then no "two publishes" "live: $(<$cur)"
else ok "two publishes: built side by side, tests queued, installed in turn"; fi
: > $ev
pub a $A 0 && no "older publish" "installed: $(<$ev)" || {
  [[ $(<$ev) == *refused* && $(<$cur) == *$C* && ! -e $L && ! -e $cache/tree.busy ]] && ok "older publish refused, lock freed" || no "older publish" "$(<$ev) $(<$tmp/why-a)"; }

# nas_lock: a lock of this Mac's whose process is gone is taken over at once; a live one's kept.
zsh -c 'exit 0' & dp=$!; wait $dp
print -r -- "mac $dp 2026-10-26T00:00:00Z" > $L
out=$(r "nas_lock $L 'mac 1 now' && cat $L")
[[ $out == *'took over'*$'\nmac 1 now' ]] && ok "nas_lock this Mac's gone holder taken over" || no "nas_lock gone holder" "$out"
print -r -- "mac $$ 2026-10-26T00:00:00Z" > $L
out=$(r "nas_lock $L 'mac 1 now' || echo held")
[[ $out == held ]] && ok "nas_lock this Mac's live holder kept" || no "nas_lock live holder" "$out"
print -r -- "other $dp 2026-10-26T00:00:00Z" > $L
out=$(r "nas_lock $L 'mac 1 now' || echo held")
[[ $out == held ]] && ok "nas_lock another Mac's holder kept" || no "nas_lock other Mac" "$out"
rm -f $L

# hold: a lock taken by another while this one moved the stale one aside is put back.
H2=$tmp/race.busy; mkdir $H2; print -r -- "$dp gone" > $H2/owner
# (Another takes it just before this one's move: its owner then in the folder moved aside.)
out=$(r "mv() { [[ \$2 == *aside* ]] && print -r -- 'x other' > \$1/owner; command mv \$@ }
  hold $H2 || echo \"held: \$lock_held\"")
[[ $out == 'held: taken just now' && -d $H2 && $(<$H2/owner 2>/dev/null) == 'x other' ]] && ok "hold puts back one taken meanwhile" || no "hold put back" "$out; $(ls $tmp | grep race)"

# fast_forward: a publish begun before a rollback is refused; one begun after it goes in.
print -r -- "{\"version\": \"20261026-1000-$C\"}" > $nas/app/previous.json
print -r -- "{\"version\": \"20261026-0953-$B\"}" > $cur
out=$(cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $C 20261026-1000-$C" 2>&1) && no "fast_forward after a rollback" "allowed" || {
  [[ $out == *'rolled back'* ]] && ok "fast_forward begun before a rollback refused" || no "fast_forward rollback" "$out"; }
out=$(cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $C 20261026-0953-$B" 2>&1) && ok "fast_forward begun after a rollback" || no "fast_forward after rollback" "$out"
rm -f $nas/app/previous.json

# smoke: two publishes' smoke tests at once (task #698), each its own port and fake server (a
# Python stand-in answering 200 after a delay, as the server answers once its catalog is read).
# The first's server is slow to start, as the mini's was beside the other publish's build: it is
# waited for, not reported as six 000s. One still starting past the wait, and one that exits, are
# each said so, with whether it printed anything.
fake=$tmp/fake.py
print -r -- 'import sys, time, http.server
time.sleep(float(sys.argv[2]))
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self): self.send_response(200); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()' > $fake
sm() {  # <name> <port> <delay> <wait>: smoke against a fake server, its output in $tmp/smoke-<name>
  python3 -I $fake $2 $3 > $tmp/log-$1 2>&1 &
  local p=$! rc
  zsh -c "source ${(q)lib}; smoke $2 $p $tmp/log-$1 $4 /api/ping /api/meta /" > $tmp/smoke-$1 2>&1; rc=$?
  kill $p 2>/dev/null; wait $p 2>/dev/null
  return $rc
}
p1=$(( 20000 + RANDOM % 20000 )); p2=$(( p1 + 1 ))
sm first $p1 3 20 & s1=$!; sm second $p2 0 20 & s2=$!
wait $s1; r1=$?; wait $s2; r2=$?
(( r1 == 0 && r2 == 0 )) && ok "smoke: two at once, the first's server slow to start, both pass" || no "smoke two at once" "$r1/$r2: $(<$tmp/smoke-first) $(<$tmp/smoke-second)"
sm slow $p1 30 1 && no "smoke still starting" "passed" || {
  [[ $(<$tmp/smoke-slow) == *'still running'*'printed nothing'* && $(<$tmp/smoke-slow) != *000* ]] && ok "smoke: a server still starting past the wait said so" || no "smoke still starting" "$(<$tmp/smoke-slow)"; }
print -r -- 'import sys; print("boom", file=sys.stderr); sys.exit(3)' > $tmp/die.py
# (Started by the shell that runs smoke, as publish.sh does: its status is that shell's to wait for.)
out=$(zsh -c "source ${(q)lib}; python3 -I $tmp/die.py > $tmp/log-die 2>&1 & smoke $p1 \$! $tmp/log-die 20 /api/ping") && no "smoke exited" "passed" || {
  [[ $out == *'exited (status 3)'*boom* ]] && ok "smoke: a server that exited said so, with its log" || no "smoke exited" "$out"; }

exit $failed
