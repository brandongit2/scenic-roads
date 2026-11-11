#!/bin/zsh
# tools/dev/ran.zsh (#1131): ran_dir gives a new folder of this script's, after ran_clean: a gone
# process's folder is marked when first seen gone, however old, and removed once marked
# RAN_KEEP_S, an unreadable one in it too; never a live one's nor a name that isn't a PID's. And
# the keep time is store::testkit's RAN_KEEP too, and the cargo shim cleans its clones with
# ran_clean. Run by check.sh when ran.zsh, this file, testkit or the shim changes.
root=${0:A:h:h:h:h}
source $root/tools/dev/ran.zsh
fails=0
fail() { print -r -- "FAIL $1"; fails=1 }
# A stand-in TMPDIR: the real one's folders are other scripts'.
export TMPDIR=$(mktemp -d)/
trap 'chmod -R u+rwx $TMPDIR; rm -rf $TMPDIR' EXIT
k=${TMPDIR%/}/scenic-ran; mkdir -p $k
/usr/bin/true & gone=$!; wait $gone
old=$(date -v-$(( RAN_KEEP_S + 60 ))S +%Y%m%d%H%M.%S)
mkdir -p $k/$gone.x.seen/locked $k/$gone.x.unseen $k/$gone.x.now $k/$gone $k/$$.x.live $k/notapid
touch $k/$gone.x.seen/.scenic-gone $k/$gone.x.now/.scenic-gone $k/$gone/.scenic-gone $k/$$.x.live/.scenic-gone
chmod 000 $k/$gone.x.seen/locked
touch -t $old $k/$gone.x.seen/.scenic-gone $k/$gone/.scenic-gone $k/$$.x.live/.scenic-gone
touch -t $old $k/$gone.x.seen $k/$gone.x.unseen $k/$gone.x.now $k/$$.x.live $k/notapid
d=$(ran_dir probe) || fail "ran_dir failed"
[[ -d $d && $d == $k/$$.probe.* ]] || fail "not a folder of this script's: $d"
[[ ! -e $k/$gone.x.seen ]] || fail "a gone script's folder, seen gone long ago, stayed (an unreadable one in it too)"
[[ ! -e $k/$gone ]] || fail "a gone shim's clones (a bare PID), seen gone long ago, stayed"
[[ -e $k/$gone.x.unseen/.scenic-gone ]] || fail "a gone script's old folder not marked when first seen gone"
[[ -d $k/$gone.x.now ]] || fail "a gone script's folder, seen gone just now, went"
[[ -d $k/$$.x.live ]] || fail "a live script's folder went"
[[ -d $k/notapid ]] || fail "a folder not named by a PID went"
e=$(ran_dir probe); [[ $e != $d ]] || fail "two calls gave one folder"
# Under errexit and nounset (guards.sh's), a folder failing to go is no failure of the caller's.
( set -euo pipefail; f=$(ran_dir strict); [[ -d $f ]] ) || fail "ran_dir under errexit"
# One keep time: testkit's; the shim cleans with ran_clean.
rs=$(sed -n 's/^pub const RAN_KEEP: Duration = Duration::from_secs(\([0-9]*\));$/\1/p' $root/crates/store/src/testkit.rs)
[[ $rs == $RAN_KEEP_S ]] || fail "keep times differ: ran.zsh $RAN_KEEP_S, testkit '$rs'"
grep -q '^source ${self:h:h}/ran.zsh$' $root/tools/dev/bin/cargo && grep -q 'ran_clean \$runs' $root/tools/dev/bin/cargo \
  || fail "the cargo shim doesn't clean its clones with ran.zsh's ran_clean"
(( fails )) || print "PASS ran_dir"
exit $fails
