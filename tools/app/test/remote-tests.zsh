#!/bin/zsh
# Tests of tools/app/remote-tests.sh's disk room on the test Mac (make_room, #912), in a scratch
# folder: a fake ~/scenic-check, a fake agent folder with its room target, a fake free space (the
# fake disk's free space rises by what is removed), locks held, stale and free. Run by
# tools/dev/check.sh when tools/app changes:
#   zsh tools/app/test/remote-tests.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
source ${0:A:h:h}/remote-tests.sh
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
failed=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1: $2"; failed=1 }

base=$tmp/scenic-check
agent_home=$tmp/agent
# The fake disk: a fixed total less what the fake ~/scenic-check holds (KB).
disk_kb=0
free_kb() { print $(( disk_kb - $(du -sk $base | cut -f1) )) }
copy() {  # <key> <target KB> <age in minutes> [origin]
  mkdir -p $base/$1/src $base/$1/target
  mkfile $(( $2 ))k $base/$1/target/blob
  touch -t $(date -v-$3M +%Y%m%d%H%M) $base/$1/stamp
  [[ -n ${4:-} ]] && print -r -- "$4" > $base/$1/origin
  return 0
}
target() { mkdir -p $agent_home; print -r -- "{\"bytes\":$(( $1 * 1024 )),\"by\":\"test\",\"at\":1}" > $agent_home/room-target.json }
fresh() { rm -rf $base $agent_home; mkdir -p $base; }
have() { local k r=(); for k in $base/*(/N); do r+=${k:t}; done; print -r -- "${(j: :)r}" }

# The least recently used go until the room target plus this key's build folder is free; the
# newest kept.
fresh; target 1000
copy me 200 1; copy old 300 50; copy mid 300 30; copy new 300 10
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 200 - 250 ))  # 250 KB short: the oldest alone
out=$(make_room $base me "" "")
[[ $(have) == "me mid new" ]] && ok "least recently used first" || no "least recently used first" "left $(have): $out"

# A copy whose lock is held stays, even the oldest; a stale lock's (older than 6 h) goes.
fresh; target 1000
copy me 100 1; copy held 300 90; copy stale 300 80; copy other 300 5
mkdir $base/held/.busy; mkdir $base/stale/.busy; touch -t $(date -v-7H +%Y%m%d%H%M) $base/stale/.busy
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 100 - 500 ))
out=$(make_room $base me "" "")
[[ $(have) == "held me" ]] && ok "held lock kept, stale one not" || no "held lock kept, stale one not" "left $(have): $out"

# Every other copy held: nothing removed, said so, and the run goes on (status 0).
fresh; target 1000
copy me 100 1; copy a 300 9; mkdir $base/a/.busy
disk_kb=$(( $(du -sk $base | cut -f1) ))
out=$(make_room $base me "" ""); rc=$?
[[ $rc == 0 && $(have) == "a me" && $out == *"other copies in use"* ]] && ok "all held: nothing removed" || no "all held: nothing removed" "rc $rc, left $(have): $out"

# Room enough: nothing removed.
fresh; target 1000
copy me 100 1; copy a 300 900
disk_kb=$(( $(du -sk $base | cut -f1) + 5000 ))
make_room $base me "" "" >/dev/null
[[ $(have) == "a me" ]] && ok "room enough: nothing removed" || no "room enough: nothing removed" "left $(have)"

# A new key needs the largest build folder there (a full build).
fresh; target 1000
mkdir -p $base/me/src; copy big 600 50; copy small 100 40
disk_kb=$(( $(du -sk $base | cut -f1) + 1000 + 300 ))  # room for a 100 KB need, short of 600
make_room $base me "" "" >/dev/null
[[ $(have) == "me small" ]] && ok "a new key needs the largest build" || no "a new key needs the largest build" "left $(have)"

# No room target (no file): this run's need alone.
fresh
copy me 100 1; copy a 300 9
disk_kb=$(( $(du -sk $base | cut -f1) + 200 ))
make_room $base me "" "" >/dev/null
[[ $(have) == "a me" ]] && ok "no room target" || no "no room target" "left $(have)"

# A gone worktree's copy (same origin, not among the live worktrees) goes, room or not; another
# origin's, a live one's and a held gone one's stay; this key's origin is recorded.
fresh; target 1
copy me 10 1; copy gone 10 5 "m1:/repo/.git"; copy live 10 5 "m1:/repo/.git"
copy elsewhere 10 5 "m4:/repo/.git"; copy plain 10 5; copy heldgone 10 5 "m1:/repo/.git"; mkdir $base/heldgone/.busy
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
out=$(make_room $base me "m1:/repo/.git" "me live other")
[[ $(have) == "elsewhere heldgone live me plain" && $(<$base/me/origin) == "m1:/repo/.git" ]] \
  && ok "a gone worktree's copy goes" || no "a gone worktree's copy goes" "left $(have): $out"

# A removal cut short (.gone-*) is finished.
fresh; copy me 10 1; mkdir -p $base/.gone-x-1/target
disk_kb=$(( $(du -sk $base | cut -f1) + 100000 ))
make_room $base me "" "" >/dev/null
[[ ! -e $base/.gone-x-1 ]] && ok "a cut-short removal finished" || no "a cut-short removal finished" "left"

# The real room target's file read as the agent writes it.
fresh; target 2048
[[ $(room_target_kb) == 2048 ]] && ok "room target read" || no "room target read" "$(room_target_kb)"

exit $failed
