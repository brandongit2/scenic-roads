#!/bin/zsh
# tools/dev/bin/cargo (#915) on a toy workspace: two worktrees build into one shared folder, each runs
# its own code (worktree b's sources older than a's build), and a second cargo waits for the first,
# saying so. Run by check.sh when tools/dev/bin or this file changes.
sh=${0:A:h:h}/bin/cargo
s=$(mktemp -d); trap 'rm -rf $s' EXIT
w=$s/.claude/worktrees; mkdir -p $w
for x in a b; do
  mkdir -p $w/$x/src; : > $w/$x/.git
  printf '[package]\nname = "toy"\nversion = "0.1.0"\nedition = "2021"\n[workspace]\n' > $w/$x/Cargo.toml
  print "fn main() { println!(\"$x\"); }" > $w/$x/src/main.rs
done
touch -t 202601010000 $w/b/src/main.rs $w/b/Cargo.toml
export SCENIC_SHARED_TARGET=$s/shared
fail() { print -u2 "FAIL: $*"; exit 1; }
run() { (cd $w/$1 && $sh run -q 2>$s/err) }
[[ $(run a) == a ]] || fail "a's first build"
[[ $(run b) == b ]] || fail "b after a ran a's build"
[[ $(run a) == a ]] || fail "a after b ran b's build"
[[ $(cat $s/shared/.scenic-built-by) == $w/a ]] || fail "built-by not recorded"
# The lock: a holder that sleeps 3 s; the second cargo says it waits and then runs.
zmodload zsh/system
( zsystem flock -f fd $s/shared/.scenic-lock; print "test holder" > $s/shared/.scenic-holder; sleep 3 ) &
sleep 0.5
out=$(run b) || fail "b behind the lock: $(cat $s/err)"
[[ $out == b ]] || fail "b behind the lock ran $out"
grep -q 'waiting for the shared build folder .*held by: test holder' $s/err || fail "no waiting line: $(cat $s/err)"
wait
# Outside .claude/worktrees: plain cargo, the shared folder untouched.
mkdir -p $s/plain/src; : > $s/plain/.git; cp $w/a/Cargo.toml $s/plain/; cp $w/a/src/main.rs $s/plain/src/
(cd $s/plain && $sh build -q) || fail "plain build"
[[ -d $s/plain/target ]] || fail "outside worktrees: not its own target/"
print "shared-target: ok"
