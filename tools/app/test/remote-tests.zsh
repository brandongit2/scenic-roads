#!/bin/zsh
# Tests of tools/app/remote-tests.sh, in a scratch folder: the copies (send_tree, #983; into_build,
# #969) build exactly the tree sent, a toy crate built by cargo before and after an edit dated back,
# and another key's tree in the same build folder; the slot a key takes; and the disk room on
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
