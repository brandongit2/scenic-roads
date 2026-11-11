#!/bin/zsh
# tools/dev/ran.zsh's ran_dir (#1131): a new folder of this script's, and the folders it removes
# first: a gone script's unchanged for RAN_KEEP_S, never a live one's nor a fresh one; and the
# keep time it says is store::testkit's RAN_KEEP and the cargo shim's. Run by check.sh when either
# file or this one changes.
root=${0:A:h:h:h:h}
source $root/tools/dev/ran.zsh
fails=0
fail() { print -r -- "FAIL $1"; fails=1 }
# A stand-in TMPDIR: the real one's folders are other scripts'.
export TMPDIR=$(mktemp -d)/
trap 'rm -rf $TMPDIR' EXIT
k=${TMPDIR%/}/scenic-ran; mkdir -p $k
/usr/bin/true & gone=$!; wait $gone
old=$(date -v-$(( RAN_KEEP_S + 60 ))S +%Y%m%d%H%M.%S)
mkdir -p $k/$gone.x.old/locked $k/$gone.x.fresh $k/$$.x.live $k/notapid
chmod 000 $k/$gone.x.old/locked
touch -t $old $k/$gone.x.old $k/$$.x.live $k/notapid
d=$(ran_dir probe) || fail "ran_dir failed"
[[ -d $d && $d == $k/$$.probe.* ]] || fail "not a folder of this script's: $d"
[[ ! -e $k/$gone.x.old ]] || fail "a gone script's old folder stayed (an unreadable one in it too)"
[[ -d $k/$gone.x.fresh ]] || fail "a gone script's fresh folder went"
[[ -d $k/$$.x.live ]] || fail "a live script's old folder went"
[[ -d $k/notapid ]] || fail "a folder not named by a PID went"
e=$(ran_dir probe); [[ $e != $d ]] || fail "two calls gave one folder"
# One keep time, said three times.
rs=$(sed -n 's/^pub const RAN_KEEP: Duration = Duration::from_secs(\([0-9]*\));$/\1/p' $root/crates/store/src/testkit.rs)
shim=$(sed -n 's/^runs=.* RAN_KEEP_S=\([0-9]*\)$/\1/p' $root/tools/dev/bin/cargo)
[[ $rs == $RAN_KEEP_S && $shim == $RAN_KEEP_S ]] || fail "keep times differ: ran.zsh $RAN_KEEP_S, testkit '$rs', the cargo shim '$shim'"
(( fails )) || print "PASS ran_dir"
exit $fails
