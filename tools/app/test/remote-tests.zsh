#!/bin/zsh
# Tests of tools/app/remote-tests.sh, in a scratch folder: the copies (send_tree, #983; into_build,
# #969) build exactly the tree sent, a toy crate built by cargo before and after an edit dated back,
# and another key's tree in the same build folder; two trees taking turns in a folder under a real
# `cargo test`; a run itself in a fake home (its slot the folder's lock); the slot a key takes; and the disk room on
# the test Mac (make_room, #912): a fake ~/scenic-check, a fake agent folder with its room target, a
# fake free space (the fake disk's free space rises by what is removed), locks held, stale and free. Run by
# tools/dev/check.sh when tools/app changes:
#   zsh tools/app/test/remote-tests.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
source ${0:A:h:h}/remote-tests.sh
# (The repo's toolchain on the PATH, as check.sh has it: the toy crate is built outside the repo,
# where mise's cargo shim finds no version and fails.)
eval "$(cd ${0:A:h:h:h:h} && mise env -s zsh 2>/dev/null)" 2>/dev/null
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
failed=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1: $2"; failed=1 }

# The copy: an edit made during a run (dated before that run's build, as rsync -a kept it) is built
# by the next; a file unchanged isn't rewritten (its crate not rebuilt).
toy() {  # <dir> <version>: a one-file crate printing <version>, its source dated back an hour
  mkdir -p $1/src
  print -r -- $'[package]\nname = "toy"\nversion = "0.1.0"\nedition = "2021"\n[workspace]' > $1/Cargo.toml
  print -r -- "fn main() { println!(\"$2\"); }" > $1/src/main.rs
  touch -t $(date -v-1H +%Y%m%d%H%M.%S) $1/Cargo.toml $1/src/main.rs
}
built() {  # <copy>: what its build prints
  ( cd $1 && CARGO_TARGET_DIR=$1/../target cargo build -q --offline 2>&1 && $1/../target/debug/toy )
}
copy_run() {  # <copier> <into its build> <key folder>: the toy sent, built, edited (same size, dated back), sent, built
  local src=$3/../tree-${3:t}
  rm -rf $src $3; mkdir -p $3/src; toy $src v1
  print -rn -- $'Cargo.toml\0src/main.rs\0' > $3/files.list
  $1 $src $3/files.list $3/src/ && $2 $3/src $3/build/src && built $3/build/src >/dev/null || return 1
  toy $src v2  # (the edit: dated an hour back, before the build above)
  stat -f %m $3/build/src/Cargo.toml > $3/cargo-toml-date
  $1 $src $3/files.list $3/src/ && $2 $3/src $3/build/src && built $3/build/src
}
kept_times() { rsync -a --from0 --files-from=$2 $1/ $3 }  # (times kept, for contrast)
kept_into() { mkdir -p $2 && rsync -a --delete $1/ $2/ }
if command -v cargo >/dev/null; then
  out=$(copy_run send_tree into_build $tmp/k1)
  [[ $out == v2 ]] && ok "an edit dated back is built" || no "an edit dated back is built" "ran: $out"
  [[ $(stat -f %m $tmp/k1/build/src/Cargo.toml) == $(<$tmp/k1/cargo-toml-date) ]] \
    && ok "an unchanged file not rewritten" || no "an unchanged file not rewritten" "Cargo.toml re-dated"
  # (The case is real: kept times built the old source.)
  out=$(copy_run kept_times kept_into $tmp/k2)
  [[ $out == v1 ]] && ok "kept times would test the stale build" || no "kept times would test the stale build" "ran: $out"
  # Keys taking turns in one build folder (#969): each run builds its own tree, back again too,
  # and a file of the other key's, or one a test wrote there, doesn't stay.
  k=$tmp/k3; mkdir -p $k/a/src $k/b/src; toy $k/a/src va; toy $k/b/src vb; print extra > $k/b/src/src/other.rs
  into_build $k/a/src $k/build/src && [[ $(built $k/build/src) == va ]] \
    && ok "a key's tree built" || no "a key's tree built" "not va"
  print written > $k/build/src/src/by-a-test
  into_build $k/b/src $k/build/src && [[ $(built $k/build/src) == vb && ! -e $k/build/src/src/by-a-test ]] \
    && ok "another key's tree built in its folder" || no "another key's tree built in its folder" "not vb, or a file left"
  into_build $k/a/src $k/build/src && [[ $(built $k/build/src) == va && ! -e $k/build/src/src/other.rs ]] \
    && ok "the first tree again, dated back as it is" || no "the first tree again, dated back as it is" "not va, or a file left"
else no "an edit dated back is built" "no cargo on the PATH"; fi

# Two trees taking turns in one build folder, a real workspace (a test crate over a dependency) and a
# real `cargo test`: a test passes only on its own tree's value, so a stale build fails it (#969).
wtree() {  # <dir> <value>: the workspace, the value in the dep's v.rs and in a file it includes
  mkdir -p $1/app/src $1/dep/src
  print -r -- $'[workspace]\nmembers = ["app", "dep"]\nresolver = "2"' > $1/Cargo.toml
  print -r -- $'[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n[dependencies]\ndep = { path = "../dep" }' > $1/app/Cargo.toml
  print -r -- $'[package]\nname = "dep"\nversion = "0.1.0"\nedition = "2021"' > $1/dep/Cargo.toml
  print -r -- '#[test] fn its_own_tree() { let w = std::env::var("WANT").unwrap(); assert_eq!(dep::V, w); assert_eq!(dep::D.trim(), w); }' > $1/app/src/lib.rs
  print -r -- $'mod v;\npub use v::V;\npub const D: &str = include_str!("d.txt");' > $1/dep/src/lib.rs
  print -r -- "pub const V: &str = \"$2\";" > $1/dep/src/v.rs
  print -r -- $2 > $1/dep/src/d.txt
}
wlist() { ( cd $1 && find . -type f ! -name Cargo.lock | sed 's|^\./||' | tr '\n' '\0' ) }
dated_back() { find $1 -type f -exec touch -t $(date -v-$2H +%Y%m%d%H%M.%S) {} + }  # (<hours> before every build)
turn() {  # <copier> <into its build> <source tree> <key copy> <build> <value>: sent, into the folder, tested for <value>
  wlist $3 > $4.list; mkdir -p $4
  # (The copy's prune, remote_main's.)
  ( cd $4 && comm -23 <(find . -type f | sed 's|^\./||' | LC_ALL=C sort) <(tr '\0' '\n' < $4.list | LC_ALL=C sort) | while IFS= read -r f; do rm -f -- "$f"; done )
  $1 $3 $4.list $4/ && $2 $4 $5/src || return 2
  ( cd $5/src && WANT=$6 CARGO_TARGET_DIR=$5/target cargo test -q --offline -p app >/dev/null 2>&1 )
}
if command -v cargo >/dev/null; then
  k=$tmp/w; a=$k/tree-a; b=$k/tree-b; f=$k/slot; mkdir -p $k
  wtree $a aaa; wtree $b bbb; dated_back $a 3; dated_back $b 3
  t() { turn send_tree into_build $2 $k/key-${3} $f $1 }
  t aaa $a a && ok "turns: the first tree" || no "turns: the first tree" "stale or failed"
  t bbb $b b && ok "turns: another branch's tree, dated before the build" || no "turns: another branch's tree, dated before the build" "stale or failed"
  t aaa $a a && ok "turns: the first again, its files unsent and old" || no "turns: the first again, its files unsent and old" "stale or failed"
  t bbb $b b && ok "turns: back again" || no "turns: back again" "stale or failed"
  # A dependency's file edited, its size kept, dated before every build.
  print -r -- 'pub const V: &str = "ccc";' > $b/dep/src/v.rs; print -r -- ccc > $b/dep/src/d.txt; dated_back $b 2
  t ccc $b b && ok "a dependency edited, the same size, dated back" || no "a dependency edited, the same size, dated back" "stale or failed"
  # Edited again with its very date and size kept: only the content says.
  cp -p $b/dep/src/v.rs $k/date; cp -p $b/dep/src/d.txt $k/date2
  print -r -- 'pub const V: &str = "ddd";' > $b/dep/src/v.rs; print -r -- ddd > $b/dep/src/d.txt
  touch -r $k/date $b/dep/src/v.rs; touch -r $k/date2 $b/dep/src/d.txt
  t ddd $b b && ok "an edit keeping date and size" || no "an edit keeping date and size" "stale or failed"
  # Changed back to an earlier version, the earlier date with it.
  print -r -- 'pub const V: &str = "bbb";' > $b/dep/src/v.rs; print -r -- bbb > $b/dep/src/d.txt; dated_back $b 5
  t bbb $b b && ok "back to an earlier version with an older date" || no "back to an earlier version with an older date" "stale or failed"
  # A module moved (v.rs to v/mod.rs): the other tree's file doesn't stay, and the move back builds.
  mkdir -p $b/dep/src/v; mv $b/dep/src/v.rs $b/dep/src/v/mod.rs; dated_back $b 5
  t bbb $b b && [[ ! -e $f/src/dep/src/v.rs ]] && ok "a module moved" || no "a module moved" "stale, failed or left"
  t aaa $a a && [[ ! -e $f/src/dep/src/v ]] && ok "and the tree before it" || no "and the tree before it" "stale, failed or left"
  # A run's test wrote into the tree, a source dated back: put right, and what's run is the tree's.
  print -r -- 'pub const V: &str = "zzz";' > $f/src/dep/src/v.rs; dated_back $f/src/dep/src/v.rs 5
  t aaa $a a && ok "a source a test overwrote" || no "a source a test overwrote" "stale or failed"
  # (The case is real: the same turns with times kept test the stale build, #983's bug.)
  f2=$k/slot2; wtree $a aaa; wtree $b bbb; dated_back $a 3; dated_back $b 3
  turn kept_times kept_into $a $k/kept-a $f2 aaa || no "kept times: the first tree" "failed"
  turn kept_times kept_into $b $k/kept-b $f2 bbb \
    && no "kept times would fail another tree's test" "not stale" || ok "kept times would fail another tree's test"
fi

# A run itself, remote_main in a fake home: its own process group (as ssh gives it), slots 2 and 3
# held, so every run here is in build folder 1.
fh=$tmp/home; fb=$fh/scenic-check
holdme="$$ $(ps -o lstart= -p $$) test"
own_group() { perl -e 'setpgrp(0,0); exec @ARGV' "$@" }
run_setup() {  # <key>: a tree and its lock
  mkdir -p $fb/$1/src $fb/$1/.busy; print -r -- tree > $fb/$1/src/f; print -rn -- $'f\0' > $fb/$1/files.list; print -r -- o > $fb/$1/.busy/owner
}
run_remote() {  # <key> <command>: a run, the group's leader
  HOME=$fh SCENIC_TESTS_TERM_WAIT=2 own_group zsh $self --remote $1 o $2 "" ""
}
wait_for() { local i; for i in {1..300}; do [[ -e $1 ]] && return 0; sleep 0.1; done; return 1 }
hold_two() { mkdir -p $fb/.slots/{2,3}; print -r -- $holdme > $fb/.slots/2/owner; print -r -- $holdme > $fb/.slots/3/owner }

# Ended as a run ends, with a test's server left: it goes first, then the slot.
rm -rf $fh; hold_two; run_setup e1
out=$(run_remote e1 "print -r -- \$PWD > $tmp/where; sleep 300 & print \$! > $tmp/left")
left=$(<$tmp/left)
[[ $(<$tmp/where) == *"/.build/1/src" && $out == *"@@remote-tests-status 0"* ]] && ok "a run in its slot's folder" || no "a run in its slot's folder" "$(<$tmp/where): $out"
! kill -0 $left 2>/dev/null && [[ ! -e $fb/.slots/1 ]] && ok "what a run left goes, then its slot" || no "what a run left goes, then its slot" "$left, $(ls $fb/.slots)"

main_of() { print -r -- ${$(<$fb/.slots/1/owner)%% *} }  # (the run itself, the group's leader: the owner its slot names)
gone() { local i; for i in {1..100}; do kill -0 $1 2>/dev/null || return 0; sleep 0.1; done; return 1 }

# Stopped by a signal, a child ignoring it: killed, nothing left, and the slot usable by the next.
rm -f $tmp/up; run_setup e2
run_remote e2 "( sh -c 'echo \$\$ > $tmp/stubborn; trap \"\" TERM; touch $tmp/up; while sleep 1; do :; done' ) & wait" >/dev/null &
rp=$!
if wait_for $tmp/up; then
  kill -TERM $(main_of); wait $rp 2>/dev/null
  gone $(<$tmp/stubborn) && ok "a TERM stops all of a run, a child ignoring it too" || no "a TERM stops all of a run, a child ignoring it too" "still there"
else no "a TERM stops all of a run, a child ignoring it too" "never started"; fi

# Killed outright, a child ignoring TERM still running: the next run doesn't share the folder with it.
rm -f $tmp/up $tmp/next; run_setup e3; run_setup e4
run_remote e3 "( sh -c 'echo \$\$ > $tmp/orphan; trap \"\" TERM; touch $tmp/up; while sleep 1; do :; done' ) & wait" >/dev/null &
rp=$!
if wait_for $tmp/up; then
  kill -KILL $(main_of); wait $rp 2>/dev/null; orphan=$(<$tmp/orphan)
  out=$(run_remote e4 "kill -0 $orphan 2>/dev/null && print shared > $tmp/next || print alone > $tmp/next; print -r -- \$PWD >> $tmp/next")
  [[ $(<$tmp/next) == alone$'\n'*/.build/1/src && $out == *"@@remote-tests-status 0"* ]] \
    && ok "a run killed outright: its folder free once nothing of it is left" || no "a run killed outright: its folder free once nothing of it is left" "$(<$tmp/next): $out"
  kill -KILL $orphan 2>/dev/null
else no "a run killed outright: its folder free once nothing of it is left" "never started"; fi

# Its slot lost during a run: no result, whatever the command said.
rm -rf $fb/.slots/1; run_setup e5
out=$(run_remote e5 "print other > $fb/.slots/1/owner")
[[ $out == *"@@remote-tests-status none"* && $out != *"@@remote-tests-status 0"* ]] && ok "a run whose slot was taken gives no result" || no "a run whose slot was taken gives no result" "$out"
rm -rf $fb/.slots/1

# Five at once in three slots: a folder is never two runs'.
rm -rf $fb/.slots; for k in c1 c2 c3 c4 c5; do run_setup $k; done; pids=()
for k in c1 c2 c3 c4 c5; do
  run_remote $k "mkdir ../inuse 2>/dev/null || print \$PWD >> $tmp/overlap; print \$PWD >> $tmp/folders; sleep 1; rmdir ../inuse" >/dev/null & pids+=($!)
done
wait $pids
[[ ! -e $tmp/overlap && $(wc -l < $tmp/folders) -eq 5 && $(sort -u $tmp/folders | wc -l) -le 3 ]] \
  && ok "runs at once: never two in a folder" || no "runs at once: never two in a folder" "$(cat $tmp/overlap $tmp/folders 2>&1)"

base=$tmp/scenic-check
# The slot a key tries first: its last; then the least recently used, those never used before all.
k=$tmp/k4; mkdir -p $k/.build/{1,2}; touch -t 202601010100 $k/.build/1/stamp; touch -t 202601010000 $k/.build/2/stamp
[[ ${(j: :)${(f)"$(slot_order $k 3 $k)"}} == "3 2 1" ]] && ok "a new key: the least recently used" \
  || no "a new key: the least recently used" "$(slot_order $k 3 $k)"
print 1 > $k/slot
[[ ${(j: :)${(f)"$(slot_order $k 3 $k)"}} == "1 3 2" ]] && ok "a key's own slot first" \
  || no "a key's own slot first" "$(slot_order $k 3 $k)"
print 9 > $k/slot
[[ ${(j: :)${(f)"$(slot_order $k 3 $k)"}} == "3 2 1" ]] && ok "a slot no longer there not tried" \
  || no "a slot no longer there not tried" "$(slot_order $k 3 $k)"

agent_home=$tmp/agent
# The fake disk: a fixed total less what the fake ~/scenic-check holds (KB).
disk_kb=0
free_kb() { print $(( disk_kb - $(du -sk $base | cut -f1) )) }
copy() {  # <key> <copy KB> <age in minutes> [origin]
  mkdir -p $base/$1/src
  mkfile $(( $2 ))k $base/$1/src/blob
  touch -t $(date -v-$3M +%Y%m%d%H%M) $base/$1/stamp
  [[ -n ${4:-} ]] && print -r -- "$4" > $base/$1/origin
  return 0
}
buildf() {  # <slot> <target KB> <age in minutes>
  mkdir -p $base/.build/$1/target
  mkfile $(( $2 ))k $base/.build/$1/target/blob
  touch -t $(date -v-$3M +%Y%m%d%H%M) $base/.build/$1/stamp
}
builds() { local k r=(); for k in $base/.build/*(/N); do r+=${k:t}; done; print -r -- "${(j: :)r}" }
target() { mkdir -p $agent_home; print -r -- "{\"bytes\":$(( $1 * 1024 )),\"by\":\"test\",\"at\":1}" > $agent_home/room-target.json }
fresh() { rm -rf $base $agent_home; mkdir -p $base; }
have() { local k r=(); for k in $base/*(/N); do r+=${k:t}; done; print -r -- "${(j: :)r}" }

# The least recently used go until the room target plus this run's build folder is free; the
# newest kept.
fresh; target 1000
copy me 10 1; buildf 1 200 1; copy old 300 50; copy mid 300 30; copy new 300 10
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 200 - 250 ))  # 250 KB short: the oldest alone
out=$(make_room $base me "" "" $base/.build/1)
[[ $(have) == "me mid new" ]] && ok "least recently used first" || no "least recently used first" "left $(have): $out"

# A copy whose lock is held stays, even the oldest; a stale lock's (older than 6 h) goes.
fresh; target 1000
copy me 10 1; buildf 1 100 1; copy held 300 90; copy stale 300 80; copy other 300 5
mkdir $base/held/.busy; mkdir $base/stale/.busy; touch -t $(date -v-7H +%Y%m%d%H%M) $base/stale/.busy
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 100 - 500 ))
out=$(make_room $base me "" "" $base/.build/1)
[[ $(have) == "held me" ]] && ok "held lock kept, stale one not" || no "held lock kept, stale one not" "left $(have): $out"

# Every other copy held: nothing removed, said so, and the run goes on (status 0).
fresh; target 1000
copy me 10 1; buildf 1 100 1; copy a 300 9; mkdir $base/a/.busy
disk_kb=$(( $(du -sk $base | cut -f1) ))
out=$(make_room $base me "" "" $base/.build/1); rc=$?
[[ $rc == 0 && $(have) == "a me" && $out == *"other copies in use"* ]] && ok "all held: nothing removed" || no "all held: nothing removed" "rc $rc, left $(have): $out"

# Room enough: nothing removed.
fresh; target 1000
copy me 10 1; buildf 1 100 1; copy a 300 900
disk_kb=$(( $(du -sk $base | cut -f1) + 5000 ))
make_room $base me "" "" $base/.build/1 >/dev/null
[[ $(have) == "a me" ]] && ok "room enough: nothing removed" || no "room enough: nothing removed" "left $(have)"

# An empty build folder needs the largest build folder there (a full build).
fresh; target 1000
mkdir -p $base/me/src; buildf 2 600 1; copy big 300 50; copy small 100 40
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 300 ))  # room for a 100 KB need, short of 600
make_room $base me "" "" $base/.build/1 >/dev/null
[[ $(have) == "me small" ]] && ok "an empty build folder needs the largest build" || no "an empty build folder needs the largest build" "left $(have)"

# The keys' copies gone and still short: the other slots' build folders, least recently used first,
# never this run's nor one whose slot is taken.
fresh; target 1000
copy me 10 1; buildf 1 100 1; buildf 2 300 90; buildf 3 300 50; buildf 4 300 10; mkdir -p $base/.slots/2
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 100 - 200 ))  # 200 KB short
out=$(make_room $base me "" "" $base/.build/1)
[[ $(builds) == "1 2 4" && ! -e $base/.slots/3 && -d $base/.slots/2 ]] \
  && ok "the other slots' build folders last" || no "the other slots' build folders last" "left $(builds): $out"

# No room target (no file): this run's need alone.
fresh
copy me 10 1; buildf 1 100 1; copy a 300 9
disk_kb=$(( $(du -sk $base | cut -f1) + 200 ))
make_room $base me "" "" $base/.build/1 >/dev/null
[[ $(have) == "a me" ]] && ok "no room target" || no "no room target" "left $(have)"

# A gone worktree's copy (same origin, not among the live worktrees) goes, room or not; another
# origin's, a live one's and a held gone one's stay; this key's origin is recorded.
fresh; target 1
copy me 10 1; copy gone 10 5 "m1:/repo/.git"; copy live 10 5 "m1:/repo/.git"
copy elsewhere 10 5 "m4:/repo/.git"; copy plain 10 5; copy heldgone 10 5 "m1:/repo/.git"; mkdir $base/heldgone/.busy
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
out=$(make_room $base me "m1:/repo/.git" "me live other" $base/.build/1)
[[ $(have) == "elsewhere heldgone live me plain" && $(<$base/me/origin) == "m1:/repo/.git" ]] \
  && ok "a gone worktree's copy goes" || no "a gone worktree's copy goes" "left $(have): $out"

# A removal cut short (.gone-*) is finished.
fresh; copy me 10 1; mkdir -p $base/.gone-x-1/target
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
make_room $base me "" "" $base/.build/1 >/dev/null
[[ ! -e $base/.gone-x-1 ]] && ok "a cut-short removal finished" || no "a cut-short removal finished" "left"

# The real room target's file read as the agent writes it.
fresh; target 2048
[[ $(room_target_kb) == 2048 ]] && ok "room target read" || no "room target read" "$(room_target_kb)"

exit $failed
