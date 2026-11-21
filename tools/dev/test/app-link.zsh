#!/bin/zsh
# tools/dev/app-link.sh (#1579) in a scratch home: ~/scenic made a link to the app's home, kept when
# it is one, and anything else there (a link elsewhere, a file, a folder) left as it is, warned of.
# Run by check.sh.
set -u
here=${0:A:h}
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }
tmp=$(mktemp -d); trap 'rm -rf $tmp' EXIT
tmp=${tmp:A}
app="$tmp/Library/Application Support/scenic"
mkdir -p $app/agent
run() { HOME=$tmp zsh $here/../app-link.sh 2>&1 }
out=$(run); rc=$?
[[ $rc == 0 && $(readlink $tmp/scenic) == $app && -d $tmp/scenic/agent && $out == *made* ]] && ok "made, a link to the app's home" || bad "made: exit $rc, $(ls -l $tmp/scenic 2>&1): $out"
[[ -z $(print -l $tmp/scenic.tmp-*(N)) ]] && ok "no temporary link left" || bad "left: $(print $tmp/scenic.tmp-*)"
out=$(run); rc=$?
[[ $rc == 0 && -z $out && $(readlink $tmp/scenic) == $app ]] && ok "run again: kept, nothing said" || bad "again: exit $rc: $out"
rm $tmp/scenic; ln -s $tmp/elsewhere $tmp/scenic
out=$(run); rc=$?
[[ $rc == 0 && $(readlink $tmp/scenic) == $tmp/elsewhere && $out == *WARNING*"left as it is"* ]] && ok "a link elsewhere left, warned of" || bad "a link elsewhere: exit $rc, $(readlink $tmp/scenic): $out"
rm $tmp/scenic; mkdir $tmp/scenic
out=$(run); rc=$?
[[ $rc == 0 && -d $tmp/scenic && ! -L $tmp/scenic && $out == *WARNING*"not a link"* ]] && ok "a folder left, warned of" || bad "a folder: exit $rc: $out"
rmdir $tmp/scenic; print x > $tmp/scenic
out=$(run); rc=$?
[[ $rc == 0 && $(<$tmp/scenic) == x && $out == *WARNING* ]] && ok "a file left, warned of" || bad "a file: exit $rc: $out"
exit $fail
