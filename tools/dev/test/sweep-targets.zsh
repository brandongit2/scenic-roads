#!/bin/zsh
# Tests of tools/dev/sweep-targets.sh's room (make_room, #954) in a scratch folder: a fake shared
# build folder and compile cache, a fake agent folder with its room target and margin, and a fake
# df (the fake disk's free space rises by what is removed). Run by tools/dev/check.sh when the
# sweep or this file changes:
#   zsh tools/dev/test/sweep-targets.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
source ${0:A:h:h}/sweep-targets.sh
repo=${0:A:h:h:h:h}  # (this checkout's settings.rs, for the default margin)
tmp=$(mktemp -d)
trap 'rm -rf $tmp' EXIT
failed=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1: $2"; failed=1 }

shared=$tmp/caches/scenic-target cache=$tmp/caches/scenic-sccache agent_home=$tmp/agent log=$tmp/log
# The fake disk: a fixed total less what the fake caches hold (KB), as df -k shows it.
disk_kb=0
df() { print "Filesystem 1024-blocks Used Available"; print "fake 0 0 $(( disk_kb - $(du -sk $tmp/caches | cut -f1) ))" }
entry() {  # <profile>/<kind>/<name> <KB> <age in hours>: a file entry, or a folder of one (name ending in /)
  local p=$shared/$1
  mkdir -p ${p:h} $shared/${1%%/*}/.fingerprint
  if [[ $1 == */ ]]; then mkdir -p $p; mkfile $(( $2 ))k $p/blob; else mkfile $(( $2 ))k $p; fi
  touch -t $(date -v-$3H +%Y%m%d%H%M) $p $p/blob(N)  # (written, and last read, then)
}
target() { mkdir -p $agent_home; print -r -- "{\"bytes\":$(( $1 * 1024 )),\"by\":\"test\",\"at\":1}" > $agent_home/room-target.json }
margin() { mkdir -p $agent_home; print -r -- "{\"bytes\":$(( $1 * 1024 )),\"by\":\"test\",\"at\":1}" > $agent_home/sweep-margin.json }
fresh() {
  rm -rf $tmp/caches $agent_home $log; mkdir -p $shared $cache
  : > $shared/.scenic-lock
  entry debug/deps/old.rlib 300 50; entry debug/incremental/mid/ 300 30; entry debug/.fingerprint/new/ 300 10
  entry debug/deps/now.rlib 300 0
  mkfile 200k $cache/c
}
have() { local r=() p; for p in $shared/debug/(deps|incremental|.fingerprint)/*(DN); do r+=${p:t}; done; print -r -- "${(j: :)${(@o)r}}" }
free() { df | awk 'NR==2 {print $4}' }
sz() { du -sk $tmp/caches | cut -f1 }

# Above the target plus the margin: nothing removed, nothing logged.
fresh; target 1000; margin 500
disk_kb=$(( $(sz) + 1500 + 100 ))
make_room
[[ $(have) == "mid new now.rlib old.rlib" && -e $cache/c && ! -e $log ]] && ok "above: nothing removed" || no "above: nothing removed" "left $(have); $(cat $log 2>/dev/null)"

# A run cut short between its renames and its delete: the next run deletes its trash, a dry run doesn't.
mkdir -p $shared/.scenic-sweep-trash/0
dry=1 make_room >/dev/null
[[ -e $shared/.scenic-sweep-trash/0 ]] || no "a run cut short: a dry run keeps its trash" "deleted"
make_room
[[ ! -e $shared/.scenic-sweep-trash ]] && ok "a run cut short: its trash deleted next run" || no "a run cut short: its trash deleted next run" "$(ls -a $shared)"

# Below it: the least recently used entries go until it is cleared; the cache is kept.
fresh; target 1000; margin 500
disk_kb=$(( $(sz) + 1500 - 450 ))  # 450 KB short: the oldest two
make_room
[[ $(have) == "new now.rlib" ]] && ok "below: the oldest first, until cleared" || no "below: the oldest first, until cleared" "left $(have)"
(( $(free) >= 1500 )) && [[ -e $cache/c ]] || no "below: cleared, the cache kept" "$(free) KB free; cache $(ls $cache)"
grep -q "removed 2 entries of the shared build folder (.*): below the room target 0.0 GB + margin 0.0 GB (set on the build page); now" $log \
  && ok "below: logged" || no "below: logged" "$(cat $log)"
[[ ! -e $shared/.scenic-sweep-trash ]] || no "below: the trash deleted" "$(ls -a $shared)"

# By last use, not last write: a crate built long ago but read since (a file's own read, a
# folder's file's) outlasts one written more lately and unused since.
fresh; target 1000; margin 500
touch -a -t $(date -v-1H +%Y%m%d%H%M) $shared/debug/deps/old.rlib $shared/debug/incremental/mid/blob
disk_kb=$(( $(sz) + 1500 - 250 ))  # 250 KB short: one entry
make_room
[[ $(have) == "mid now.rlib old.rlib" ]] && ok "by last use: read since outlasts written since" \
  || no "by last use: read since outlasts written since" "left $(have)"

# Still below with every entry gone: the compile cache is emptied too.
fresh; target 1000; margin 500
disk_kb=$(( $(sz) + 1500 - 1350 ))
make_room
[[ -z $(have) && ! -e $cache/c ]] && grep -q "emptied the compile cache" $log \
  && ok "below past the folder: the cache emptied" || no "below past the folder: the cache emptied" "left $(have); $(cat $log)"

# The lock held: nothing of the folder removed, the log says so (the next run tries again).
fresh; target 1000; margin 500
disk_kb=$(( $(sz) + 1500 - 450 ))
print "test holder" > $shared/.scenic-holder
( zsystem flock -f fd $shared/.scenic-lock; : > $tmp/held; while [[ ! -e $tmp/go ]]; do sleep 0.05; done ) & holding=$!
until [[ -e $tmp/held ]]; do sleep 0.05; done
make_room
: > $tmp/go; wait $holding
[[ $(have) == "mid new now.rlib old.rlib" ]] && grep -q "shared build folder locked (held by: test holder): nothing removed, the next run tries again" $log \
  && ok "lock held: skipped and logged" || no "lock held: skipped and logged" "left $(have); $(cat $log 2>/dev/null)"

# No room target: the margin alone is kept free, and the log says so.
fresh; margin 500
disk_kb=$(( $(sz) + 500 - 250 ))
make_room
[[ $(have) == "mid new now.rlib" ]] && grep -q "below the margin 0.0 GB (set on the build page; no room target: no room-target.json in the agent's folder)" $log \
  && ok "no room target: the margin alone" || no "no room target: the margin alone" "left $(have); $(cat $log 2>/dev/null)"

# No margin set: settings.rs's default (GB), read from its line.
fresh; target 1000
wanted
d=$(sed -n 's/^pub const SWEEP_MARGIN_DEFAULT_GB: u64 = \([0-9]*\);.*/\1/p' $repo/crates/agentcore/src/agent/settings.rs)
[[ -n $d ]] && (( want == 1000 + (d << 20) )) && [[ $room == *"(default)"* ]] \
  && ok "no margin: the default" || no "no margin: the default" "want $want KB, default $d; $room"

# A dry run changes nothing and says what it would do.
fresh; target 1000; margin 500
disk_kb=$(( $(sz) + 1500 - 450 ))
out=$(dry=1 make_room)
[[ $(have) == "mid new now.rlib old.rlib" && ! -e $log && $out == *"would remove 2 entries"* ]] \
  && ok "dry run" || no "dry run" "left $(have); $out"
exit $failed
