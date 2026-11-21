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
# (A kept folder, never removed here: the toy crates' programs and tests run from it,
# tools/dev/ran.zsh, #1131.)
source ${0:A:h:h:h}/dev/ran.zsh
tmp=$(ran_dir remote-tests)
failed=0
# (A fake agent folder for the functions run here: never this Mac's own, #1570's hold.)
agent_home=$tmp/agent
# (No pool here, whatever this Mac's agent runs: the runs' shares the same on any Mac.)
print -r -- "  9 1 30.0 /usr/bin/true" > $tmp/ps0
export SCENIC_TESTS_PS=$tmp/ps0
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
holdme="$(proc_id) test"
own_group() { perl -e 'setpgrp(0,0); exec @ARGV' "$@" }
run_setup() {  # <key>: a tree and its lock
  mkdir -p $fb/$1/src $fb/$1/.busy; print -r -- tree > $fb/$1/src/f; print -rn -- $'f\0' > $fb/$1/files.list; print -r -- o > $fb/$1/.busy/owner
}
run_remote() {  # <key> <command>: a run, the group's leader
  HOME=$fh SCENIC_TESTS_TERM_WAIT=2 own_group zsh $self --remote $1 o $2 "" ""
}
wait_for() { local i; for i in {1..300}; do [[ -e $1 ]] && return 0; sleep 0.1; done; return 1 }
hold_two() { mkdir -p $fb/.slots/{2,3}; print -r -- $holdme > $fb/.slots/2/owner; print -r -- $holdme > $fb/.slots/3/owner }

# The capacity (#1539): a slot per 4 cores and per 12 GB, the fewer, at least one; each run's test
# threads its share of the cores, rounded up; the capacity file's slots and threads, either.
cap() { capacity $tmp/capacity $1 $2; print -r -- "$reply[1] $reply[2]" }
rm -f $tmp/capacity
[[ $(cap 14 48) == "3 5" && $(cap 10 16) == "1 10" && $(cap 2 4) == "1 2" && $(cap 16 24) == "2 8" ]] \
  && ok "the capacity, from the cores and the memory" || no "the capacity, from the cores and the memory" "$(cap 14 48), $(cap 10 16), $(cap 2 4), $(cap 16 24)"
print 'slots=2' > $tmp/capacity; a=$(cap 14 48)
print -n 'threads=3' > $tmp/capacity; b=$(cap 14 48)
[[ $a == "2 7" && $b == "3 3" ]] && ok "the capacity, as its file sets it, a last line without its newline too" || no "the capacity, as its file sets it, a last line without its newline too" "$a, $b"
print -r -- $'  slots = 2  # two runs\r\n# a comment\n\n threads=4\r' > $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn
[[ "$reply[1] $reply[2]" == "2 4" && ! -s $tmp/warn ]] && ok "the capacity file: spaces, CRs and comments" || no "the capacity file: spaces, CRs and comments" "$reply[1] $reply[2]: $(<$tmp/warn)"
print -r -- $'slots=0\nthreads=x\njobs=2\nslots' > $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn
w=$(<$tmp/warn)
[[ "$reply[1] $reply[2]" == "3 5" && $w == *"line 1: slots=0"* && $w == *"line 2: threads=x"* && $w == *"line 3: jobs=2"*unknown* && $w == *"line 4: slots"* && $(wc -l < $tmp/warn) -eq 4 ]] \
  && ok "the capacity file: each line not read says so, by its value" || no "the capacity file: each line not read says so, by its value" "$reply[1] $reply[2]: $w"
print -r -- $'slots=1000\nthreads=0005' > $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn
w=$(<$tmp/warn)
[[ "$reply[1] $reply[2]" == "3 5" && $w == *"slots=1000 isn't a whole number from 1 to 999"* && $w == *"threads=0005 isn't"* ]] \
  && ok "the capacity file: a value of more than 3 digits not read" || no "the capacity file: a value of more than 3 digits not read" "$reply[1] $reply[2]: $w"
print -r -- $'slots=2\nthreads=4\nslots=1' > $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn
w=$(<$tmp/warn)
[[ "$reply[1] $reply[2]" == "1 4" && $w == *"line 3: slots given again (line 1): this line's 1 read"* && $(wc -l < $tmp/warn) -eq 1 ]] \
  && ok "the capacity file: a key given twice says so, the last read" || no "the capacity file: a key given twice says so, the last read" "$reply[1] $reply[2]: $w"
rm -f $tmp/capacity; mkdir $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn; w=$(<$tmp/warn); rmdir $tmp/capacity
print 'slots=2' > $tmp/capacity; chmod 000 $tmp/capacity
capacity $tmp/capacity 14 48 > $tmp/warn; w2=$(<$tmp/warn); chmod 644 $tmp/capacity
[[ "$reply[1] $reply[2]" == "3 5" && $w == *"is a folder or can't be read"* && $w2 == *"is a folder or can't be read"* ]] \
  && ok "the capacity file a folder, or unreadable: said, the capacity derived" || no "the capacity file a folder, or unreadable: said, the capacity derived" "$reply[1] $reply[2]: $w / $w2"
rm -f $tmp/warn
print 'threads=3' > $tmp/capacity; capacity $tmp/capacity 14 48
[[ $reply[3] == "slots derived: 14 cores / 4, 48 GB / 12; threads set in $tmp/capacity" ]] && ok "the capacity: the slots' and the threads' sources, each" || no "the capacity: the slots' and the threads' sources, each" "$reply[3]"
print 'slots=2' > $tmp/capacity; capacity $tmp/capacity 14 48
[[ $reply[3] == "slots set in $tmp/capacity; threads derived: 14 cores / 2 slots" ]] && ok "the capacity: threads derived from the slots set" || no "the capacity: threads derived from the slots set" "$reply[3]"

# Ended as a run ends, with a test's server left: it goes first, then the slot.
# (publish_wait 1 min: a publish run waiting for slots never freed ends its wait within the test.)
rm -rf $fh; mkdir -p $fb; print $'slots=3\npublish_wait=1' > $fb/capacity; hold_two; run_setup e1
out=$(run_remote e1 "print -r -- \$PWD > $tmp/where; sleep 300 & print \$! > $tmp/left")
left=$(<$tmp/left)
[[ $(<$tmp/where) == *"/.build/1/src" && $out == *"@@remote-tests-status 0"* ]] && ok "a run in its slot's folder" || no "a run in its slot's folder" "$(<$tmp/where): $out"
! kill -0 $left 2>/dev/null && [[ ! -e $fb/.slots/1 ]] && ok "what a run left goes, then its slot" || no "what a run left goes, then its slot" "$left, $(ls $fb/.slots)"

# Its builds below its tests (#1583): rustc through the wrapper, clamped to utility, as cargo runs it.
run_setup e0
out=$(run_remote e0 "print -r -- \$RUSTC_WRAPPER > $tmp/wrap; \$RUSTC_WRAPPER rustc -vV > $tmp/vv 2>&1")
w=$(<$tmp/wrap)
[[ $w == $fb/.rustc-utility && -x $w && $(<$w) == *'exec /usr/sbin/taskpolicy -c utility '*'"$@"' && $(<$tmp/vv) == *$'\n'release:* && $out == *"@@remote-tests-status 0"* ]] \
  && ok "a run's rustc through its utility wrapper" || no "a run's rustc through its utility wrapper" "$w: $(cat $w $tmp/vv 2>&1): $out"

main_of() { local o=$(<$fb/.slots/1/owner); print -r -- ${o%% *} }  # (the run itself, the group's leader: the owner its slot names)
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
[[ ! -e $tmp/overlap && $(wc -l < $tmp/folders) -eq 5 && $(sort -u $tmp/folders | wc -l) -le 3 && $(<$tmp/folders) != */.build/0/* ]] \
  && ok "runs at once: never two in a folder, none in publish's" || no "runs at once: never two in a folder, none in publish's" "$(cat $tmp/overlap $tmp/folders 2>&1)"

# Publish's run: its own slot, 0, with every core as its test threads; a check's, its share.
out=$(unset SCENIC_PUBLISH; local_main publish $tmp "touch $tmp/pubran" 2>&1); rc=$?
[[ $rc == 2 && ! -e $tmp/pubran && $out == *"SCENIC_PUBLISH isn't exported"* ]] && ok "key publish without SCENIC_PUBLISH: refused here" || no "key publish without SCENIC_PUBLISH: refused here" "$rc: $out"
run_setup publish
rm -f $tmp/pub
out=$(run_remote publish "print -r -- ran > $tmp/pub")
[[ ! -e $tmp/pub && $out == *"@@remote-tests-status none"*SCENIC_PUBLISH* ]] && ok "key publish without SCENIC_PUBLISH: not run" || no "key publish without SCENIC_PUBLISH: not run" "$(cat $tmp/pub 2>&1): $out"
rm -rf $fb/.slots/{2,3}
out=$(run_remote publish "export SCENIC_PUBLISH=1; print -r -- \$PWD \$NEXTEST_TEST_THREADS > $tmp/pub; ls ../../../.slots > $tmp/pubslots")
[[ $(<$tmp/pub) == */.build/0/src\ $(sysctl -n hw.ncpu) && $out == *"slot 0 of 3, $(sysctl -n hw.ncpu) test threads"* ]] \
  && ok "publish's run: its own slot, every core as it builds" || no "publish's run: its own slot, every core as it builds" "$(<$tmp/pub): $out"
out=$(run_remote c1 "print -r -- \$NEXTEST_TEST_THREADS \$RAYON_NUM_THREADS > $tmp/chk")
n=$(( ($(sysctl -n hw.ncpu) + 2) / 3 ))
[[ $(<$tmp/chk) == "$n 2" && $out == *" of 3, $n test threads"*"2 threads a test"* ]] && ok "a check's run: its share of the cores, a test's own threads its share of those" || no "a check's run: its share of the cores" "$(<$tmp/chk): $out"

# The pool's jobs (#1563): the CPU of the agent's process and every process under it, nothing else;
# taken off a check's share, and off publish's tests' threads, half the cores left them at least.
print -r -- "    1     0   0.0 /sbin/launchd
  610     1   1.0 scenic-launcher agent
  611   610  55.0 /Users/x/Library/Application Support/scenic/app/current/scenic agent
  700   611   0.0 /bin/sh -c s=\$1; shift
  701   700 310.0 /Users/x/Library/Application Support/scenic/app/v1/scenic-build unit --root /n
  702   701  40.0 /Users/x/Library/Application Support/scenic/app/v1/scenic-build tool scenic-metrics
  800     1 900.0 rustc --crate-name scenic agentx
  801     1 200.0 awk /(^|\\/)scenic agent( |\$)/" > $tmp/ps
[[ $(SCENIC_TESTS_PS=$tmp/ps pool_busy) == "4 4.0 4" ]] && ok "the pool's jobs: its agent's tree alone" || no "the pool's jobs: its agent's tree alone" "$(SCENIC_TESTS_PS=$tmp/ps pool_busy)"
# A job its agent's restart left (adopted by the new agent, its wrapper's parent launchd): counted,
# with what is under it; the map server and others of the app's folder not.
cp $tmp/ps $tmp/ps-orphan
print -r -- "91014     1   0.0 /bin/sh -c s=\$1; shift; \"\$@\"
91018 91014 199.0 /Users/x/Library/Application Support/scenic/app/v0/scenic-build depth --root /n
91019 91018  50.0 /usr/bin/python3 -I x.py
92000     1 120.0 /Users/x/Library/Application Support/scenic/app/v1/server --port 0" >> $tmp/ps-orphan
[[ $(SCENIC_TESTS_PS=$tmp/ps-orphan pool_busy) == "7 6.5 6" ]] && ok "the pool's jobs: one left by an agent's restart too" || no "the pool's jobs: one left by an agent's restart too" "$(SCENIC_TESTS_PS=$tmp/ps-orphan pool_busy)"
[[ $(SCENIC_TESTS_PS=$tmp/ps0 pool_busy) == "0 0.0 0" ]] && ok "the pool's jobs: no agent, none" || no "the pool's jobs: no agent, none" "$(SCENIC_TESTS_PS=$tmp/ps0 pool_busy)"
rm -f $tmp/capacity; capacity $tmp/capacity 14 48 5
[[ "$reply[1] $reply[2]" == "3 3" && $reply[3] == *"threads derived: 14 cores less 5 the pool's jobs keep busy / 3 slots" ]] \
  && ok "the capacity: a check's share of what the pool leaves" || no "the capacity: a check's share of what the pool leaves" "$reply"
capacity $tmp/capacity 14 48 20
[[ $reply[2] == 1 ]] && ok "the capacity: a test thread at least" || no "the capacity: a test thread at least" "$reply[2]"
a=$(publish_share 14 4 ""; print $REPLY); b=$(publish_share 14 11 ""; print $REPLY); c=$(publish_share 14 11 6; print $REPLY $reply[1])
[[ $a == 10 && $b == 7 && $c == "6 publish_threads set in the capacity file" ]] && ok "publish's threads: what the pool leaves, half at least, or as set" || no "publish's threads: what the pool leaves, half at least, or as set" "$a, $b, $c"
print -r -- $'publish_threads=9\npublish_wait=2' > $tmp/capacity; capacity $tmp/capacity 14 48
[[ $reply[4] == 9 && $reply[5] == 120 && $reply[6] == 1 ]] && ok "the capacity file: publish's threads and wait" || no "the capacity file: publish's threads and wait" "$reply"
print -r -- 'measures=2' > $tmp/capacity; capacity $tmp/capacity 14 48
[[ $reply[6] == 2 ]] && ok "the capacity file: the measurement runs at once" || no "the capacity file: the measurement runs at once" "$reply"
rm -f $tmp/capacity

# While publish's run holds slot 0, every other run waits at start, in no slot (a run outside
# check.sh's too: any key); publish's tests wait for the runs under way, each check slot taken as it
# frees; then every slot is publish's, and all are freed as it ends.
rm -rf $fb/.slots; mkdir -p $fb/.slots/2; print -r -- $holdme > $fb/.slots/2/owner  # (a run under way, in slot 2)
run_setup publish; run_setup t1560-base; rm -f $tmp/{pubgo,pubtests,pubend,chkran,pubthreads,slotsheld}
run_remote publish "export SCENIC_PUBLISH=1; touch $tmp/pubgo; t=\$(\$SCENIC_TEST_MAC_ALONE); print \$t > $tmp/pubthreads; ls $fb/.slots > $tmp/slotsheld; date +%s > $tmp/pubtests; until [[ -e $tmp/pubend ]]; do sleep 0.2; done" > $tmp/pubout 2>&1 &
pp=$!
wait_for $tmp/pubgo
run_remote t1560-base "date +%s > $tmp/chkran; ls $fb/.slots >> $tmp/chkran" > $tmp/chkout 2>&1 &
cp=$!
sleep 3
[[ ! -e $tmp/pubtests && ! -e $tmp/chkran && $(<$tmp/chkout) == *"is publish's run's (slot 0 since"* && -d $fb/.slots/1 && -d $fb/.slots/3 ]] \
  && ok "behind publish: another run waits at start, publish's tests wait for the run under way" || no "behind publish: another run waits at start, publish's tests wait for the run under way" "$(cat $tmp/chkout $tmp/pubout)"
rm -rf $fb/.slots/2  # (the run under way ends)
wait_for $tmp/pubtests
[[ -e $tmp/pubtests && ! -e $tmp/chkran && $(<$tmp/slotsheld) == $'0\n1\n2\n3' && $(<$tmp/pubthreads) == "NEXTEST_TEST_THREADS=$(sysctl -n hw.ncpu) RUST_TEST_THREADS=$(sysctl -n hw.ncpu) RAYON_NUM_THREADS=2" \
   && $(<$fb/.slots/2/owner) == $(<$fb/.slots/0/owner) ]] \
  && ok "publish's tests: once every slot is its own" || no "publish's tests: once every slot is its own" "$(cat $tmp/slotsheld $tmp/pubthreads $tmp/pubout)"
[[ $(<$tmp/pubout) == *"wait for the test Mac's other runs to end"*"slot 2 test (since"*"the test Mac is publish's after"* ]] \
  && ok "publish's wait said: what for, and how it went" || no "publish's wait said: what for, and how it went" "$(<$tmp/pubout)"
touch $tmp/pubend; wait $pp $cp
[[ -e $tmp/chkran && $(<$tmp/chkout) == *"got a test-Mac slot after"* && $(<$tmp/chkout) == *"@@remote-tests-status 0"* && ! -e $fb/.slots/0 ]] \
  && ok "the run behind publish goes once it ends, its slots all freed" || no "the run behind publish goes once it ends, its slots all freed" "$(cat $tmp/chkout; ls $fb/.slots)"
[[ -z $(ls $fb/.slots) ]] && ok "nothing held after" || no "nothing held after" "$(ls $fb/.slots)"

# (Alone, nothing behind it to free a gone run's slots: its own end frees them all.)
out=$(run_remote publish "export SCENIC_PUBLISH=1; \$SCENIC_TEST_MAC_ALONE >/dev/null; ls $fb/.slots > $tmp/slotsheld")
[[ $(<$tmp/slotsheld) == $'0\n1\n2\n3' && -z $(ls $fb/.slots) ]] && ok "publish's run ended: every slot it held freed" || no "publish's run ended: every slot it held freed" "$(<$tmp/slotsheld); $(ls $fb/.slots): $out"
# (#1570) Its tests' hold, in the agent's folder while its tests run, names its run's process and
# its tests' threads, and goes as the run ends; by a TERM too.
ah="$fh/Library/Application Support/scenic/agent"; mkdir -p $ah; rm -f $tmp/{held,pubowner,pubgo}
out=$(run_remote publish "export SCENIC_PUBLISH=1; t=\$(\$SCENIC_TEST_MAC_ALONE); print \${\${t#*=}%% *} > $tmp/pubthreads; cat \"$ah/test-hold.json\" > $tmp/held; cat $fb/.slots/0/owner > $tmp/pubowner")
o=$(<$tmp/pubowner)
[[ $(plutil -extract pid raw -o - $tmp/held 2>/dev/null) == ${o%% *} && $(plutil -extract cores raw -o - $tmp/held 2>/dev/null) == $(<$tmp/pubthreads) && ! -e $ah/test-hold.json ]] \
  && ok "publish's tests' hold: its run's, its threads, freed at its end" || no "publish's tests' hold: its run's, its threads, freed at its end" "$(cat $tmp/held $tmp/pubthreads 2>&1); $(ls $ah): $out"
run_remote publish "export SCENIC_PUBLISH=1; \$SCENIC_TEST_MAC_ALONE >/dev/null; touch $tmp/pubgo; sleep 30" >/dev/null 2>&1 &
pp=$!
wait_for $tmp/pubgo; held=$([[ -e $ah/test-hold.json ]] && print yes)
o=$(<$fb/.slots/0/owner); kill -TERM ${o%% *}; wait $pp 2>/dev/null
[[ $held == yes && ! -e $ah/test-hold.json && -z $(ls $fb/.slots) ]] && ok "publish's tests' hold freed on a TERM" || no "publish's tests' hold freed on a TERM" "${held:-not held}; $(ls $ah) $(ls $fb/.slots)"
rm -rf $fh/Library
# Publish's run killed outright mid-wait: its slots are a gone run's, taken by the next run.
mkdir -p $fb/.slots/3; print -r -- $holdme > $fb/.slots/3/owner; rm -f $tmp/pubgo
run_remote publish "export SCENIC_PUBLISH=1; touch $tmp/pubgo; \$SCENIC_TEST_MAC_ALONE" >/dev/null 2>&1 &
pp=$!
wait_for $tmp/pubgo; for i in {1..50}; do [[ -d $fb/.slots/1 && -d $fb/.slots/2 ]] && break; sleep 0.1; done
pm=$(o=$(<$fb/.slots/0/owner); print ${o%% *})
kill -KILL $pm; wait $pp 2>/dev/null; rm -rf $fb/.slots/3
out=$(run_remote t1560-base "true")
[[ $out == *"@@remote-tests-status 0"* ]] && ! publish_holds $fb && ok "publish killed outright: its slots freed for the next run" || no "publish killed outright: its slots freed for the next run" "$out; $(ls $fb/.slots)"
rm -rf $fb/.slots

# The reservation stops once its run is gone (killed outright, no trap: a run on the test Mac itself).
mkdir -p $fb/.slots/2; print -r -- $holdme > $fb/.slots/2/owner
true & dead=$!; wait $dead
( reserve_all $fb 2 "$dead Fri Nov 20 20:00:00 2026 publish" ) & rp=$!
for i in {1..50}; do kill -0 $rp 2>/dev/null || break; sleep 0.1; done
if kill -0 $rp 2>/dev/null; then kill $rp; no "the reservation stops once its run is gone" "still running"
else ok "the reservation stops once its run is gone"; fi
rm -rf $fb/.slots

# Publish's wait is bounded (publish_wait): past it, its tests go on, saying what is left.
mkdir -p $fb/.slots/{0,1}; print -r -- $holdme > $fb/.slots/1/owner
out=$(wait_alone $fb 1 1 14 "" "me" "" 2>&1)
[[ $out == *"went on after"*"slot 1 test"* && $out == *"no agent here: no hold"* && ${out##*$'\n'} == "NEXTEST_TEST_THREADS=14 RUST_TEST_THREADS=14 RAYON_NUM_THREADS=2" ]] && ok "publish's wait bounded, said" || no "publish's wait bounded, said" "$out"
rm -rf $fb/.slots
# (#1570) Its tests' threads held from the pool's jobs in the agent's folder: its run's process, the cores.
mkdir -p $fb/.slots/0 $tmp/agent-w; rm -f $tmp/agent-w/test-hold.json; touch $fb/.slots/0/alone
out=$(wait_alone $fb 1 1 14 "" "$$ Fri Nov 20 20:00:00 2026 publish" $tmp/agent-w 2>&1)
[[ $(plutil -extract pid raw -o - $tmp/agent-w/test-hold.json 2>/dev/null) == $$ && $(plutil -extract cores raw -o - $tmp/agent-w/test-hold.json 2>/dev/null) == 14 \
   && $out == *"14 test threads"*"held from the pool's jobs ($tmp/agent-w/test-hold.json)"* ]] \
  && ok "publish's tests' threads held from the pool's jobs" || no "publish's tests' threads held from the pool's jobs" "$out; $(cat $tmp/agent-w/test-hold.json 2>&1)"
(agent_home=$tmp/agent-w; free_mine $fb "1 x publish" $fb/.slots/0)
[[ -e $tmp/agent-w/test-hold.json ]] && ok "another run's hold left as it is" || no "another run's hold left as it is" "removed"
(agent_home=$tmp/agent-w; free_mine $fb "$$ x publish" $fb/.slots/0)
[[ ! -e $tmp/agent-w/test-hold.json ]] && ok "a run's own hold freed" || no "a run's own hold freed" "left"
rm -rf $fb/.slots $tmp/agent-w

# A run on the test Mac itself (no copy): a slot taken, behind publish's run too, freed as it ends.
mkdir -p $tmp/here; hold_two
out=$(HOME=$fh here_on_test_mac h1 $tmp/here "cat $fb/.slots/1/owner; print \$NEXTEST_TEST_THREADS" 2>&1)
[[ $out == *"running here (this is the test Mac;"*"slot 1 of 3, $n test threads"* && $out == *" h1"$'\n'$n && ! -e $fb/.slots/1 ]] \
  && ok "a run on the test Mac itself: counted in a slot, its share, freed" || no "a run on the test Mac itself: counted in a slot, its share, freed" "$out; $(ls $fb/.slots)"
rm -f $tmp/up $tmp/sl
# (#1570: its tests' hold too, by the trap as a TERM ends it: the run's process is this shell's.)
mkdir -p $agent_home; print -r -- "{\"cores\":3,\"pid\":$$,\"at\":1}" > $agent_home/test-hold.json
( HOME=$fh here_on_test_mac h2 $tmp/here "touch $tmp/up; sleep 30 & print \$! > $tmp/sl; wait" >/dev/null 2>&1 ) &
hp=$!
wait_for $tmp/up && [[ -d $fb/.slots/1 ]]; held=$?
kill -TERM $hp; wait $hp 2>/dev/null
for i in {1..50}; do [[ -e $fb/.slots/1 ]] || break; sleep 0.1; done
(( held == 0 )) && [[ ! -e $fb/.slots/1 && ! -e $agent_home/test-hold.json ]] && ok "a run on the test Mac itself stopped: its slot freed, its hold too" || no "a run on the test Mac itself stopped: its slot freed, its hold too" "$held; $(ls $fb/.slots $agent_home)"
[[ -s $tmp/sl ]] && kill $(<$tmp/sl) 2>/dev/null
# (remote-tests.sh itself on the test Mac: that way, not run outside the capacity.)
out=$(SCENIC_THIS_MAC=tm SCENIC_TEST_MAC=tm SCENIC_TESTS_LOCAL_FILE=$tmp/no-marker HOME=$fh local_main h4 $tmp/here "cat $fb/.slots/1/owner" 2>&1)
[[ $out == *"slot 1 of 3"*" h4"* ]] && ok "remote-tests.sh on the test Mac: its run in a slot" || no "remote-tests.sh on the test Mac: its run in a slot" "$out"
# Behind publish's run, it waits too.
rm -rf $fb/.slots/1; mkdir -p $fb/.slots/0; print -r -- "$holdme" > $fb/.slots/0/owner; rm -f $tmp/hran
( HOME=$fh here_on_test_mac h3 $tmp/here "touch $tmp/hran" > $tmp/hout 2>&1 ) &
hp=$!
sleep 2
[[ ! -e $tmp/hran && $(<$tmp/hout) == *"is publish's run's"* ]] && ok "a run on the test Mac itself waits behind publish's" || no "a run on the test Mac itself waits behind publish's" "$(<$tmp/hout)"
rm -rf $fb/.slots/0; wait $hp
[[ -e $tmp/hran ]] && ok "and goes once publish's run ends" || no "and goes once publish's run ends" "$(<$tmp/hout)"
rm -rf $fb/.slots

# Measurement runs (#1575): marked #perf or #repro in their command, or setting SCENIC_PUBLISH under
# another key; one at a time (a turn, before its slot), beside the checks in the slots' capacity.
mk() { is_measure $1 "$2" && print m || print n }
[[ $(mk k 'cargo test --release -p x #perf')$(mk k 'cargo nextest run --workspace #repro')$(mk k 'export SCENIC_PUBLISH=1; cargo nextest run -p pipeline')$(mk k 'a;#perf') == mmmm \
   && $(mk k 'cargo test -p x')$(mk k 'cargo test -p x #performance')$(mk k 'echo a#perf')$(mk publish 'export SCENIC_PUBLISH=1; cargo nextest run') == nnnn ]] \
  && ok "a measurement run: by its marks" || no "a measurement run: by its marks" "$(mk k 'cargo test --release -p x #perf')…"
# (Waits on what a run says or writes, polled under the watchdog, 2 min: never a fixed head start,
# testing.md #1157. A run held up past it, a broken admission's, is stopped, not waited on for 30 min.)
said_in() { local i; for i in {1..1200}; do [[ -e $1 && $(<$1) == *$2* ]] && return 0; sleep 0.1; done; return 1 }  # <file> <text>
bounded_run() {  # <key> <command> <file it writes> <output file>: the run, stopped if <file> isn't written within 2 min
  run_remote $1 $2 > $4 2>&1 &
  local p=$!
  said_in $3 '' || kill -TERM $p 2>/dev/null  # (the run itself, its group's leader: its trap stops the rest)
  wait $p 2>/dev/null
}
run_setup m1; run_setup m2; run_setup c6; rm -f $tmp/{m1go,m1end,m2ran,c6ran}
run_remote m1 "touch $tmp/m1go; until [[ -e $tmp/m1end ]]; do sleep 0.2; done #perf" > $tmp/m1out 2>&1 &
m1p=$!
said_in $tmp/m1go ''
run_remote m2 "#perf
date +%s > $tmp/m2ran" > $tmp/m2out 2>&1 &
m2p=$!
said_in $tmp/m2out "measurement turns are taken"
[[ ! -e $tmp/m2ran && $(<$tmp/m2out) == *"measurement turns are taken"*m1* ]] \
  && ok "a second measurement run waits while one runs" || no "a second measurement run waits while one runs" "$(<$tmp/m2out)"
[[ $(ls $fb/.slots | tr '\n' ' ') == [1-3]\ m1\  ]] && ok "a measurement run waiting for its turn holds no slot" || no "a measurement run waiting for its turn holds no slot" "$(ls $fb/.slots)"
bounded_run c6 "date +%s > $tmp/c6ran" $tmp/c6ran $tmp/c6out; out=$(<$tmp/c6out)
[[ -e $tmp/c6ran && ! -e $tmp/m2ran && $out == *"@@remote-tests-status 0"* && $out != *measurement* ]] \
  && ok "a check goes beside a measurement run" || no "a check goes beside a measurement run" "$out"
touch $tmp/m1end; wait $m1p $m2p
[[ -e $tmp/m2ran && $(<$tmp/m2out) == *"got a measurement turn after"*"a measurement run, turn 1 of 1)"* && $(<$tmp/m1out) == *"a measurement run, turn 1 of 1)"* && -z $(ls $fb/.slots) ]] \
  && ok "the next measurement run goes once the first ends, its turn and slot freed" || no "the next measurement run goes once the first ends, its turn and slot freed" "$(<$tmp/m2out); $(ls $fb/.slots)"
# Behind publish's run, a measurement run takes neither its turn nor a slot.
mkdir -p $fb/.slots/0; print -r -- "$holdme" > $fb/.slots/0/owner; rm -f $tmp/m2ran
run_remote m2 "date +%s > $tmp/m2ran #perf" > $tmp/m2out 2>&1 &
m2p=$!
said_in $tmp/m2out "is publish's run's (slot 0 since"
[[ ! -e $tmp/m2ran && $(ls $fb/.slots) == 0 && $(<$tmp/m2out) == *"a measurement run"*"is publish's run's (slot 0 since"* ]] \
  && ok "behind publish's run, a measurement run takes no turn" || no "behind publish's run, a measurement run takes no turn" "$(ls $fb/.slots): $(<$tmp/m2out)"
rm -rf $fb/.slots/0; wait $m2p
[[ -e $tmp/m2ran && $(<$tmp/m2out) == *"got a measurement turn after"* ]] && ok "a measurement run goes once publish's run ends" || no "a measurement run goes once publish's run ends" "$(<$tmp/m2out)"
# A gone run's turn is freed: its process gone, or its PID another process's (its start time differs).
true & dead=$!; wait $dead
sleep 300 & live=$!
for o in "$dead Fri Nov 20 20:00:00 2026 gone" "$live Fri Nov 20 20:00:00 2026 reused"; do
  rm -rf $fb/.slots; mkdir -p $fb/.slots/m1; print -r -- "$o" > $fb/.slots/m1/owner; rm -f $tmp/m2ran
  bounded_run m2 "date +%s > $tmp/m2ran #perf" $tmp/m2ran $tmp/m2out; out=$(<$tmp/m2out)
  [[ -e $tmp/m2ran && $out == *"a measurement run, turn 1 of 1)"* && -z $(ls $fb/.slots) ]] \
    && ok "a gone run's turn freed (${o##* })" || no "a gone run's turn freed (${o##* })" "$out; $(ls $fb/.slots)"
done
kill $live 2>/dev/null; wait $live 2>/dev/null
# No slot within its wait: its turn freed too.
hold_two; mkdir -p $fb/.slots/1; print -r -- "$holdme" > $fb/.slots/1/owner; rm -f $tmp/m2ran
out=$(SCENIC_TESTS_SLOT_WAIT=5 run_remote m2 "date +%s > $tmp/m2ran #perf")
[[ ! -e $tmp/m2ran && $out == *"@@remote-tests-status none no test-Mac slot"* && ! -e $fb/.slots/m1 ]] \
  && ok "no slot for a measurement run: its turn freed" || no "no slot for a measurement run: its turn freed" "$out; $(ls $fb/.slots)"
rm -rf $fb/.slots
# measures= of the capacity file: that many at once.
print $'slots=3\npublish_wait=1\nmeasures=2' > $fb/capacity; rm -f $tmp/{m1go,m1end,m2ran}
run_remote m1 "touch $tmp/m1go; until [[ -e $tmp/m1end ]]; do sleep 0.2; done #perf" > $tmp/m1out 2>&1 &
m1p=$!
said_in $tmp/m1go ''
bounded_run m2 "date +%s > $tmp/m2ran #perf" $tmp/m2ran $tmp/m2out; out=$(<$tmp/m2out)
touch $tmp/m1end; wait $m1p
[[ -e $tmp/m2ran && $out == *"a measurement run, turn 2 of 2)"* && $out != *"turns are taken"* ]] \
  && ok "the capacity file's measures=: that many at once" || no "the capacity file's measures=: that many at once" "$out"
print $'slots=3\npublish_wait=1' > $fb/capacity; rm -rf $fb/.slots
# A measurement run on the test Mac itself: its turn too, freed as it ends.
mkdir -p $fb/.slots/m1; print -r -- "$holdme" > $fb/.slots/m1/owner; rm -f $tmp/hran
( HOME=$fh here_on_test_mac h5 $tmp/here "touch $tmp/hran #perf" > $tmp/hout 2>&1 ) &
hp=$!
said_in $tmp/hout "measurement turns are taken"
[[ ! -e $tmp/hran && $(<$tmp/hout) == *"measurement turns are taken"* ]] && ok "a measurement run on the test Mac itself waits for its turn" || no "a measurement run on the test Mac itself waits for its turn" "$(<$tmp/hout)"
rm -rf $fb/.slots/m1; wait $hp
[[ -e $tmp/hran && $(<$tmp/hout) == *"a measurement run, turn 1 of 1)"* && -z $(ls $fb/.slots) ]] && ok "and goes once it is free, its turn freed" || no "and goes once it is free, its turn freed" "$(<$tmp/hout); $(ls $fb/.slots)"
rm -rf $fb/.slots
# Its waiting line with no owner to name (a turn just taken, its owner not yet written), its stdin
# open (a caller's, never closed): said, and the run goes once that turn is freed (10 s on, no owner).
# (Its stdin a fifo whose one writer is this shell: closed after, a run hung reading it ends then.)
mkdir -p $fb/.slots/m1; rm -f $tmp/hran $tmp/stdin; mkfifo $tmp/stdin
( HOME=$fh here_on_test_mac h6 $tmp/here "touch $tmp/hran #perf" < $tmp/stdin > $tmp/hout 2>&1 ) &
hp=$!
exec {sfd}>$tmp/stdin
said_in $tmp/hran ''
[[ -e $tmp/hran && $(<$tmp/hout) == *"measurement turns are taken ()"* ]] && ok "a measurement run's wait with no owner to name, its stdin open: it goes" || no "a measurement run's wait with no owner to name, its stdin open: it goes" "$(<$tmp/hout)"
exec {sfd}>&-
[[ -e $tmp/hran ]] || { rm -rf $fb/.slots/m1; kill -TERM $hp 2>/dev/null }  # (the run held up, a broken one's: ended, not waited on)
wait $hp; rm -rf $fb/.slots $tmp/stdin

# The capacity lowered: the build folders above its slots go, but one in use.
mkdir -p $fb/.build/{4,5}/target $fb/.slots/4; print -r -- $holdme > $fb/.slots/4/owner
out=$(run_remote c1 "true")
[[ ! -e $fb/.build/5 && -d $fb/.build/4 && -d $fb/.build/0 && $out == *"slot 5 on the test Mac (above the test Mac's 3 slots"* ]] \
  && ok "the capacity lowered: the folders above its slots go, those in use kept" || no "the capacity lowered: the folders above its slots go, those in use kept" "$(ls $fb/.build): $out"
rm -rf $fb/.slots/4 $fb/.build/4
# A slot above them whose run is gone: freed, its folder gone too.
true & dead=$!; wait $dead
mkdir -p $fb/.build/6/target $fb/.slots/6; print -r -- "$dead 1 test" > $fb/.slots/6/owner
out=$(run_remote c1 "true")
[[ ! -e $fb/.build/6 && ! -e $fb/.slots/6 && $out == *"slot 6 on the test Mac (above"* ]] \
  && ok "the capacity lowered: a slot above it whose run is gone freed, its folder gone" || no "the capacity lowered: a slot above it whose run is gone freed, its folder gone" "$(ls $fb/.build $fb/.slots): $out"
# One whose owner, alive, was written under another LANG and TZ (#1555): kept, its folder too.
mkdir -p $fb/.build/6/target $fb/.slots/6
print -r -- "$(env -u LC_ALL LANG=fr_FR.UTF-8 TZ=Asia/Tokyo zsh -c 'source $1; proc_id $2' zsh $lib $$) test" > $fb/.slots/6/owner
out=$(LC_ALL=en_US.UTF-8 TZ=America/Vancouver run_remote c1 "true")
[[ -d $fb/.build/6 && -e $fb/.slots/6/owner ]] \
  && ok "a slot whose owner was written under LANG=fr_FR and another TZ: still alive, kept" || no "a slot whose owner was written under LANG=fr_FR and another TZ: still alive, kept" "$(<$fb/.slots/6/owner) vs $(proc_id): $out"
rm -rf $fb/.slots/6 $fb/.build/6
# One whose owner was written by the code before #1555 (ps's start time in the writer's own locale
# and zone; the run's here): still alive under this code, kept.
mkdir -p $fb/.build/6/target $fb/.slots/6
print -r -- "$$ $(ps -o lstart= -p $$) test" > $fb/.slots/6/owner
out=$(run_remote c1 "true")
[[ -d $fb/.build/6 && -e $fb/.slots/6/owner ]] \
  && ok "a slot whose owner id is in the form before #1555: still alive, kept" || no "a slot whose owner id is in the form before #1555: still alive, kept" "$(<$fb/.slots/6/owner) vs $(proc_id): $out"
rm -rf $fb/.slots/6 $fb/.build/6

base=$tmp/scenic-check
# The slot a key tries first: its last; then the least recently used, those never used before all.
k=$tmp/k4; mkdir -p $k/.build/{1,2}; touch -t 202601010100 $k/.build/1/stamp; touch -t 202601010000 $k/.build/2/stamp
[[ ${(j: :)${(f)"$(slot_order $k 3 $k)"}} == "3 2 1" ]] && ok "a new key: the least recently used" \
  || no "a new key: the least recently used" "$(slot_order $k 3 $k)"
print 1 > $k/slot
[[ ${(j: :)${(f)"$(slot_order $k 3 $k)"}} == "1 3 2" ]] && ok "a key's own slot first" \
  || no "a key's own slot first" "$(slot_order $k 3 $k)"
# A check's key (<worktree>.check, #1524) with no slot yet: its worktree's key's.
rm $k/slot; mkdir -p $k.check; print 2 > $k/slot
[[ ${(j: :)${(f)"$(slot_order $k 3 $k.check)"}} == "2 3 1" ]] && ok "a check's first slot: its worktree's" \
  || no "a check's first slot: its worktree's" "$(slot_order $k 3 $k.check)"
print 1 > $k.check/slot
[[ ${(j: :)${(f)"$(slot_order $k 3 $k.check)"}} == "1 3 2" ]] && ok "a check's own slot once it has one" \
  || no "a check's own slot once it has one" "$(slot_order $k 3 $k.check)"
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

# check.sh's key, <worktree>.check (#1524), is its worktree's: kept while the worktree is live, gone
# with it.
fresh; target 1
copy me 10 1; copy live.check 10 5 "m1:/repo/.git"; copy gone.check 10 5 "m1:/repo/.git"
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
out=$(make_room $base me "m1:/repo/.git" "me live" $base/.build/1)
[[ $(have) == "live.check me" ]] && ok "a check's key goes with its worktree" || no "a check's key goes with its worktree" "left $(have): $out"

# A drop removes a key's folder and its check's, not one in use nor another key's.
fresh; copy wt 10 1; copy wt.check 10 1; copy wt2 10 1; copy u 10 1; copy u.check 10 1; mkdir $base/u.check/.busy
out=$(HOME=$tmp; remote_drop wt; remote_drop u)
[[ $(have) == "u.check wt2" && $out == *"removed wt on"*"removed wt.check on"*"removed u on"*"in use: u.check" ]] && ok "a drop: the key's and its check's" || no "a drop: the key's and its check's" "left $(have): $out"

# A removal cut short (.gone-*) is finished.
fresh; copy me 10 1; mkdir -p $base/.gone-x-1/target
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
make_room $base me "" "" $base/.build/1 >/dev/null
[[ ! -e $base/.gone-x-1 ]] && ok "a cut-short removal finished" || no "a cut-short removal finished" "left"

# The real room target's file read as the agent writes it.
fresh; target 2048
[[ $(room_target_kb) == 2048 ]] && ok "room target read" || no "room target read" "$(room_target_kb)"

exit $failed
