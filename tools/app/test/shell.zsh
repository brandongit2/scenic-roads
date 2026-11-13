#!/bin/zsh
# Tests of tools/app/lib.zsh, the shell steps publish.sh sources, each in a scratch folder (nothing
# of the NAS, app/ or target/ touched); then services.zsh beside it (the services a release ships,
# staged by lib.zsh's services_stage, installed in a sandbox). Run by tools/dev/check.sh when
# tools/app changes:
#   zsh tools/app/test/shell.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
lib=${0:A:h:h}/lib.zsh
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
# (zsh skips the EXIT trap on a signal: a stopped publish removes $tmp too.)
trap 'exit 143' TERM; trap 'exit 130' INT
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
# (target/tmp, which ended the 25 Oct publishes) and a target triple.
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
old $t/aarch64-apple-darwin/release/deps/libold2.rlib
new $t/aarch64-apple-darwin/release/deps/libnew2.rlib
out=$(zsh -euo pipefail -c "source ${(q)lib}; prune_target ${(q)t}; echo done" 2>&1); rc=$?
gone=($t/release/deps/libold.rlib $t/release/build/oldcrate-1 $t/debug/.fingerprint/gone-3 $t/aarch64-apple-darwin/release/deps/libold2.rlib)
kept=($t/release/deps/libnew.rlib $t/release/build/mixed-2/out/a $t/debug/incremental/kept-4 $t/tmp/scratch $t/aarch64-apple-darwin/release/deps/libnew2.rlib)
left=(${^gone}(N)); lost=(); for f in $kept; do [[ -e $f ]] || lost+=($f); done
if (( rc )) || [[ $out != *$'\n'done ]]; then no prune_target "ended early (exit $rc): $out"
elif (( $#left )); then no prune_target "kept old: $left"
elif (( $#lost )); then no prune_target "removed fresh: $lost"
elif [[ $out != 'build folder: '*' MB of old builds removed, '*' left'* ]]; then no prune_target "said: $out"
else ok prune_target; fi
# And with nothing to prune, or no target/ at all.
out=$(zsh -euo pipefail -c "source ${(q)lib}; prune_target $tmp/none; echo done" 2>&1)
[[ $out == *$'\n'done ]] && ok "prune_target no target" || no "prune_target no target" "$out"

r() { zsh -euo pipefail -c "source ${(q)lib}; $1" 2>&1 }

# hold / unhold: a live holder keeps it; a gone one's is taken over; freed only by its owner.
H=$tmp/tree.busy
zsh -c "source ${(q)lib}; hold $H; sleep 30" & hp=$!
for i in {1..200}; do [[ -s $H/owner ]] && break; sleep 0.1; done
out=$(r "hold $H || echo \"held: \$lock_held\"; unhold $H; [[ -d $H ]] && echo kept")
[[ $out == "held: process $hp "*$'\nkept' ]] && ok "hold held by a live process" || no "hold live" "$out"
kill $hp; wait $hp 2>/dev/null
out=$(r "hold $H && echo taken; unhold $H; [[ -d $H ]] || echo freed")
[[ $out == $'taken\nfreed' ]] && ok "hold taken over from a gone process" || no "hold gone" "$out"
# (An owner id naming a live PID with another start time is another process's: not alive.)
out=$(r "id=\$(proc_id); proc_alive \$id && echo live; proc_alive \"\${id%% *} Thu Jan  1 00:00:00 1970\" || echo other")
[[ $out == $'live\nother' ]] && ok "proc_alive: the process, not its PID" || no "proc_alive" "$out"

# stop_program: only the processes running that very program, by PID; one whose arguments merely
# name it (what `pkill -f` would also stop) and another program are left.
# (A program of its own: a copy of a system one, /bin/sleep, is stopped by macOS as it starts. Built
# in a kept folder, never $tmp: Gatekeeper scans a program after it starts, and syspolicyd crashed
# when the file was gone by then, #1131. Each says it runs (its first argument, a ready file) before
# stop_program looks: until its exec ends, which that scan can hold up, ps shows the shell that
# started it, and stop_program found nothing to stop, #1111.)
source ${0:A:h:h:h}/dev/ran.zsh
nap=$(ran_dir stop-program); mkdir -p $nap/a $nap/b
print -r -- '#include <fcntl.h>
#include <unistd.h>
int main(int argc, char **argv) { if (argc > 1) close(open(argv[1], O_CREAT | O_WRONLY, 0644)); sleep(300); return 0; }' > $nap/napper.c
cc -o $nap/a/napper $nap/napper.c && cp $nap/a/napper $nap/b/napper
$nap/a/napper $tmp/napper-a & pa=$!
$nap/b/napper $tmp/napper-b & pb=$!
sh -c 'while sleep 1; do :; done' $nap/a/napper & pn=$!  # (a loop: sh stays, its arguments naming the program; a lone sleep replaced it)
for i in {1..1200}; do [[ -e $tmp/napper-a && -e $tmp/napper-b ]] && break; sleep 0.1; done
if [[ ! -e $tmp/napper-a || ! -e $tmp/napper-b ]]; then no "stop_program" "the programs never ran"
else
  out=$(r "stop_program $nap/a/napper; echo done")
  for i in {1..1200}; do kill -0 $pa 2>/dev/null || break; sleep 0.1; done
  if [[ $out != done ]]; then no "stop_program" "$out"
  elif kill -0 $pa 2>/dev/null; then no "stop_program" "the program's process still runs"
  elif ! kill -0 $pb 2>/dev/null || ! kill -0 $pn 2>/dev/null; then no "stop_program" "stopped another"
  else ok "stop_program: that program's processes alone"; fi
fi
kill $pa $pb $pn 2>/dev/null; wait $pa $pb $pn 2>/dev/null

# fast_forward: in a stand-in repository, live = b; c builds on it, a is older, d diverged.
g=$tmp/repo; git init -q $g
c() { git -C $g -c user.name=t -c user.email=t@t commit -q --allow-empty -m $1; git -C $g rev-parse --short HEAD }
A=$(c a); B=$(c b); C=$(c c); git -C $g checkout -q $A; D=$(c d)
# (The live release's manifest and the one before's, as lead_live writes them.)
rel=$tmp/rel; mkdir -p $rel; cur=$rel/current.json
ff() { (cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $1" 2>&1) }
out=$(ff $A) && ok "fast_forward nothing live" || no "fast_forward nothing live" "$out"
print -r -- "{\"version\": \"20261026-0953-$B\"}" > $cur
out=$(ff $C) && ok "fast_forward newer" || no "fast_forward newer" "$out"
out=$(ff $B) && ok "fast_forward same" || no "fast_forward same" "$out"
out=$(ff $A) && no "fast_forward older" "allowed" || { [[ $out == *'older than the live version'* ]] && ok "fast_forward older refused" || no "fast_forward older" "$out"; }
out=$(ff $D) && no "fast_forward diverged" "allowed" || { [[ $out == *diverged* ]] && ok "fast_forward diverged refused" || no "fast_forward diverged" "$out"; }
print -r -- '{"version": "20261026-0953-0000000"}' > $cur
out=$(ff $C) && no "fast_forward unknown live" "allowed" || ok "fast_forward unknown live refused"

# lead_install / lead_rollback (#1118): against a stand-in `scenic releases` (the lead's releases
# in $LD/live.json, each push and rollback logged; `move-once`: the next push refused, exit 3, as another publish went in meanwhile, moved.json
# live then), in the stand-in repository above: B builds on A, C on B, D diverged.
LD=$tmp/lead; mkdir -p $LD
fake=$tmp/scenic-releases
cat > $fake <<'EOF'
#!/bin/zsh
LD=${0:h}/lead
mf() { python3 -c 'import json, sys; print(json.dumps({"version": sys.argv[1]}))' $1; }
case "$1 $2" in
  "releases --json") cat $LD/live.json ;;
  "releases push")
    [[ ! -f $LD/move-always ]] || { print -r -- moved >> $LD/log; exit 3; }
    [[ ! -f $LD/refuse ]] || { print -r -- refused >> $LD/log; exit 1; }
    print -r -- "push $(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' $4) from $6" >> $LD/log
    if [[ -f $LD/move-once ]]; then rm $LD/move-once; cp $LD/moved.json $LD/live.json; exit 3; fi
    python3 -c 'import json, sys
live = json.load(open(sys.argv[1])); m = json.load(open(sys.argv[2]))
if (live["current"] or {}).get("version", "none") != sys.argv[3]: sys.exit(3)
json.dump({"current": m, "previous": live["current"]}, open(sys.argv[1], "w"))' $LD/live.json $4 $6 || exit
    [[ ! -f $LD/answer-lost ]] || exit 1 ;;
  "releases rollback")
    print -r -- "rollback $3 $4" >> $LD/log
    python3 -c 'import json, sys
live = json.load(open(sys.argv[1]))
json.dump({"current": live["previous"], "previous": live["current"]}, open(sys.argv[1], "w"))' $LD/live.json ;;
  *) exit 2 ;;
esac
EOF
chmod +x $fake
lv1() { python3 -c 'import json, sys; l = json.load(open(sys.argv[1])); print((l["current"] or {}).get("version"), (l["previous"] or {}).get("version"))' $LD/live.json; }
setlive() { python3 -c 'import json, sys; m = lambda v: {"version": v} if v else None; print(json.dumps({"current": m(sys.argv[1]), "previous": m(sys.argv[2])}))' "$1" "${2:-}"; }
print -r -- '{"version": "20261112-0002-'$C'"}' > $tmp/c.manifest
li() { rm -f $LD/log; (cd $g && zsh -c "source ${(q)lib}; lead_install $fake /dev/null $tmp/c.manifest $C '$1' $tmp/lead-read; print -r -- rc=\$?" 2>&1) }
setlive 20261112-0001-$B > $LD/live.json
out=$(li 20261112-0001-$B)
[[ $out == rc=0 && $(<$LD/log) == "push 20261112-0002-$C from 20261112-0001-$B" && $(lv1) == "20261112-0002-$C 20261112-0001-$B" ]] \
  && ok "lead_install: pushed from the live release read" || no "lead_install" "$out / $(<$LD/log)"
# Another publish (B's, over A) went in between the read and the push: read again, pushed from it.
setlive 20261112-0000-$A > $LD/live.json; setlive 20261112-0001-$B 20261112-0000-$A > $LD/moved.json; touch $LD/move-once
out=$(li 20261112-0000-$A)
[[ $out == *'changed since it was read: again'*rc=0 && $(wc -l < $LD/log) == *2 && $(tail -1 $LD/log) == "push 20261112-0002-$C from 20261112-0001-$B" ]] \
  && ok "lead_install: the live release moved meanwhile, read and pushed again" || no "lead_install moved" "$out / $(<$LD/log)"
# The lead's live release diverged from this commit (D's): refused here, nothing pushed.
setlive 20261112-0003-$D > $LD/live.json
out=$(li '')
[[ $out == *diverged*rc=1 && ! -s $LD/log ]] && ok "lead_install: a diverged live release refused, nothing pushed" || no "lead_install diverged" "$out"
# A rollback on the lead since the publish began (live B, C before it, began at C): refused.
setlive 20261112-0001-$B 20261112-0002-$C > $LD/live.json
out=$(li 20261112-0002-$C)
[[ $out == *'rolled back to'*rc=1 && ! -s $LD/log ]] && ok "lead_install: a rollback since the publish began refused" || no "lead_install rolled back" "$out"
# Moving on every push: given up.
setlive 20261112-0000-$A > $LD/live.json; cp $LD/live.json $LD/moved.json
out=$(cd $g && zsh -c "source ${(q)lib}; lead_tries=2; touch $LD/move-always; lead_install $fake /dev/null $tmp/c.manifest $C '' $tmp/lead-read; print -r -- rc=\$?" 2>&1)
[[ $out == *'kept changing'*rc=1 ]] && ok "lead_install: a live release moving at every try, given up" || no "lead_install kept moving" "$out"
# Taken, its answer lost (exit 1): live on the lead all the same. Refused (exit 1, not live): 1.
rm -f $LD/move-once $LD/move-always
setlive 20261112-0001-$B > $LD/live.json; touch $LD/answer-lost
out=$(li 20261112-0001-$B)
[[ $out == *'answer was lost'*rc=0 && $(lv1) == "20261112-0002-$C 20261112-0001-$B" ]] \
  && ok "lead_install: the answer lost, live on the lead" || no "lead_install answer lost" "$out"
rm $LD/answer-lost
setlive 20261112-0001-$B > $LD/live.json; touch $LD/refuse
out=$(li 20261112-0001-$B)
[[ $out == *'refused the release'*rc=1 && $(lv1) == "20261112-0001-$B None" ]] \
  && ok "lead_install: refused, 1" || no "lead_install refused" "$out"
rm $LD/refuse
# lead_rollback: the one before made live from the two read; none before it, refused.
setlive 20261112-0002-$C 20261112-0001-$B > $LD/live.json; rm -f $LD/log
out=$(cd $g && zsh -c "source ${(q)lib}; lead_rollback $fake $tmp/lead-read; print -r -- rc=\$?" 2>&1)
[[ $out == *"rolled back to 20261112-0001-$B on the lead"*rc=0 && $(<$LD/log) == "rollback 20261112-0002-$C 20261112-0001-$B" && $(lv1) == "20261112-0001-$B 20261112-0002-$C" ]] \
  && ok "lead_rollback" || no "lead_rollback" "$out / $(<$LD/log)"
setlive 20261112-0002-$C > $LD/live.json; rm -f $LD/log
out=$(cd $g && zsh -c "source ${(q)lib}; lead_rollback $fake $tmp/lead-read; print -r -- rc=\$?" 2>&1)
[[ $out == *'no release before'*rc=1 && ! -s $LD/log ]] && ok "lead_rollback: none before, refused" || no "lead_rollback none before" "$out"

# Two publishes at once, as publish.sh composes these steps (fake steps, the stand-in lead and
# repository): each takes a tree, the tests one at a time, then its install on the lead
# (lead_install: the lead's live release read, the order checked). The second (c, newer) builds
# beside the first (b) and goes in right after it, over it; a third, of an older commit (a), is
# refused at its install.
setlive '' > $LD/live.json; rm -f $LD/log; cache=$tmp/cache; mkdir -p $cache; ev=$tmp/events; : > $ev
for x in a:$A b:$B c:$C; do print -r -- "{\"version\": \"20261026-0953-${x#*:}\"}" > $tmp/${x%%:*}.manifest; done
# No timing: each waits on the other's events (w, at most 60 s), so the order checked below is
# forced by the locks, not by sleeps: c claims while b holds a tree, b's tests before c asks for
# them, c installs once b has.
pub() {  # <name> <commit> <event before built> <event before tested> <event before installing>
  (cd $g && zsh -c "set -euo pipefail; source ${(q)lib}
    e() { print -r -- \"$1 \$*\" >> $ev; }
    w() { [[ -z \$1 ]] && return; local t=\$(( SECONDS + 60 )); while ! grep -qF -- \"\$1\" $ev; do (( SECONDS < t )) || { e timeout \$1; exit 1; }; sleep 0.05; done; }
    cleanup=(); claim_tree $cache; e build \${snap:t}; w '$3'; e built
    until hold $cache/tests.busy; do sleep 0.1; done; e tests; w '$4'; e tested; unhold $cache/tests.busy
    w '$5'
    if ! lead_install $fake /dev/null $tmp/$1.manifest $2 '' $tmp/lead-$1 > $tmp/why-$1; then e refused; unhold \$snap.busy; exit 1; fi
    e installed; unhold \$snap.busy" 2>&1)
}
# b: built once c took the other tree, tested once c is built (so c asks for the tests b holds).
# c: starts once b holds a tree, built once b holds the tests, installs once b has.
pub b $B 'c build' 'c built' '' & p1=$!
t=$(( SECONDS + 60 )); while ! grep -qF 'b build' $ev && (( SECONDS < t )); do sleep 0.05; done
pub c $C 'b tests' '' 'b installed' & p2=$!
wait $p1; r1=$?; wait $p2; r2=$?
order=$(tr '\n' ',' < $ev)
seq() { local s=$order w; for w in "$@"; do [[ $s == *"$w"* ]] || return 1; s=${s#*"$w"}; done }
if (( r1 || r2 )); then no "two publishes" "exit $r1/$r2: $order"
elif ! seq 'b build tree,' 'c build tree-2,' 'b built'; then no "two publishes build beside each other" "$order"
elif ! seq 'b tests' 'b tested' 'c tests' 'c tested'; then no "two publishes' tests one at a time" "$order"
elif [[ $(lv1) != "20261026-0953-$C 20261026-0953-$B" || $(<$LD/log) != "push 20261026-0953-$B from none"$'\n'"push 20261026-0953-$C from 20261026-0953-$B" ]]; then
  no "two publishes" "live: $(lv1); $(<$LD/log)"
else ok "two publishes: built side by side, tests queued, the second put live over the first"; fi
: > $ev; rm -f $LD/log
pub a $A '' '' '' && no "older publish" "installed: $(<$ev)" || {
  [[ $(<$ev) == *refused* && $(<$tmp/why-a) == *'older than the live version'* && ! -s $LD/log && ! -e $cache/tree.busy ]] \
    && ok "older publish refused, nothing pushed, its tree freed" || no "older publish" "$(<$ev) $(<$tmp/why-a)"; }

# claim_tree: with both trees held a third publish waits (saying so once, naming the holders),
# claims the first to free, takes over a dead holder's, and a TERM while waiting leaves nothing held.
ct=$tmp/ct; mkdir -p $ct
fake() {  # <tree> <seconds>: a holder process
  zsh -c "source ${(q)lib}; hold $ct/$1.busy; sleep $2" & fk=$!
  for i in {1..200}; do [[ -s $ct/$1.busy/owner ]] && break; sleep 0.1; done
}
waiter() {  # <out file>: a third publish
  zsh -c "source ${(q)lib}; cleanup=(); tree_poll=0.2; done_() { for c in \$cleanup; do eval \$c; done; }; at_end done_
    claim_tree $ct; echo claimed \${snap:t}; sleep 30 & wait \$!" > $1 2>&1 & wp=$!
}
upon() {  # <text> <file>: until the file says it (at most 20 s; the check after says if it never did)
  local t=$(( SECONDS + 20 )); while ! grep -qF -- $1 $2 2>/dev/null && (( SECONDS < t )); do sleep 0.05; done
}
fake tree 30; f1=$fk; fake tree-2 30; f2=$fk
waiter $tmp/w1; upon waiting $tmp/w1
[[ $(grep -c waiting $tmp/w1) == 1 && $(<$tmp/w1) == *"tree: process $f1 "*"tree-2: process $f2 "* ]] && ! grep -q claimed $tmp/w1 \
  && ok "claim_tree waits, saying so once, naming the holders" || no "claim_tree waits" "$(<$tmp/w1)"
kill $f1; wait $f1 2>/dev/null
for i in {1..200}; do grep -q claimed $tmp/w1 && break; sleep 0.1; done
[[ $(grep -c waiting $tmp/w1) == 1 && $(<$tmp/w1) == *'claimed tree' && $(<$ct/tree.busy/owner) == "$wp "* ]] \
  && ok "claim_tree claims the tree a holder freed (a dead holder's)" || no "claim_tree freed" "$(<$tmp/w1)"
kill $wp; wait $wp 2>/dev/null
[[ ! -e $ct/tree.busy ]] && ok "claim_tree's tree freed when its publish is stopped" || no "claim_tree holder stopped" "left"
fake tree 30; f1=$fk
waiter $tmp/w2; upon waiting $tmp/w2; kill $wp; wait $wp 2>/dev/null
[[ ! -e $ct/tree.busy.aside-$wp && -d $ct/tree.busy && $(<$ct/tree.busy/owner) == "$f1 "* && $(<$ct/tree-2.busy/owner) == "$f2 "* && -z $(ls $ct | grep -v '^tree')  ]] \
  && ok "claim_tree: TERM while waiting leaves no lock of its own, others' kept" || no "claim_tree TERM" "$(ls $ct)"
kill $f1 $f2; wait $f1 $f2 2>/dev/null

# hold: a lock taken by another while this one moved the stale one aside is put back.
zsh -c 'exit 0' & dp=$!; wait $dp
H2=$tmp/race.busy; mkdir $H2; print -r -- "$dp gone" > $H2/owner
# (Another takes it just before this one's move: its owner then in the folder moved aside.)
out=$(r "mv() { [[ \$2 == *aside* ]] && print -r -- 'x other' > \$1/owner; command mv \$@ }
  hold $H2 || echo \"held: \$lock_held\"")
[[ $out == 'held: taken just now' && -d $H2 && $(<$H2/owner 2>/dev/null) == 'x other' ]] && ok "hold puts back one taken meanwhile" || no "hold put back" "$out; $(ls $tmp | grep race)"

# fast_forward: a publish begun before a rollback is refused; one begun after it goes in.
print -r -- "{\"version\": \"20261026-1000-$C\"}" > $rel/previous.json
print -r -- "{\"version\": \"20261026-0953-$B\"}" > $cur
out=$(cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $C 20261026-1000-$C" 2>&1) && no "fast_forward after a rollback" "allowed" || {
  [[ $out == *'rolled back'* ]] && ok "fast_forward begun before a rollback refused" || no "fast_forward rollback" "$out"; }
out=$(cd $g && zsh -c "source ${(q)lib}; fast_forward $cur $C 20261026-0953-$B" 2>&1) && ok "fast_forward begun after a rollback" || no "fast_forward after rollback" "$out"
rm -f $rel/previous.json

# rollback_ok: back to a version with the live one's rollback floor, or both without one: allowed;
# to one before the live one's floor (none, or another): refused, saying why; an unknown commit: refused.
rf() { mkdir -p $g/tools/app; print -r -- "$1"$'\n'"# why $1" > $g/tools/app/rollback-floor; git -C $g add -A; c $2; }
E=$(c e); F=$(rf one f); G=$(c g); H=$(rf two h)
prev=$rel/previous.json
ro() { print -r -- "{\"version\": \"20261026-0953-$1\"}" > $cur; print -r -- "{\"version\": \"20261026-0900-$2\"}" > $prev
  (cd $g && zsh -c "source ${(q)lib}; rollback_ok $cur $prev" 2>&1) }
out=$(ro $E $C) && ok "rollback_ok neither has a floor" || no "rollback_ok no floors" "$out"
out=$(ro $G $F) && ok "rollback_ok same floor" || no "rollback_ok same floor" "$out"
out=$(ro $F $E) && no "rollback_ok past a floor" "allowed" || { [[ $out == *'refused'*'why one'* ]] && ok "rollback_ok past a floor refused" || no "rollback_ok past a floor" "$out"; }
out=$(ro $H $G) && no "rollback_ok past a new floor" "allowed" || { [[ $out == *'refused'* ]] && ok "rollback_ok past a new floor refused" || no "rollback_ok past a new floor" "$out"; }
out=$(ro $G 0000000) && no "rollback_ok unknown commit" "allowed" || ok "rollback_ok unknown commit refused"
rm -f $prev

# The services (tools/app/services.zsh), in a sandbox of their own.
zsh ${0:A:h}/services.zsh || failed=1

exit $failed
