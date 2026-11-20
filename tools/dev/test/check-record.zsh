#!/bin/zsh
# tools/dev/check.sh on a toy repo (#980): what it records in check-passed. A pass names the tree
# it checked; nothing is recorded when HEAD moved during the run, a file was written or appeared
# then, or the checkout wasn't clean at the start. Its parts are stubs (cargo, guards.sh,
# affected-tests.sh, check-reuse.py, remote-tests.sh); the stub cargo's clippy can hold the run
# while the test changes the checkout; its check-running file, there while it runs, gone after; its
# stop at a failed part (#1524): fmt formatting a file, or guards failing, and clippy, prod and the
# tests don't run; and its tests' key, <worktree>.check, free while its agent's own run holds the
# worktree's. Run by check.sh.
set -u
setopt no_bg_nice   # (a run in the background: no nice, which the test Mac's utility priority refuses)
here=${0:A:h}
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }
tmp=$(mktemp -d); trap 'rm -rf $tmp' EXIT
mkdir -p $tmp/r $tmp/home; cd $tmp/r
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
w() { mkdir -p ${1:h}; print -r -- "$2" > $1; chmod +x $1 }
g init -q -b main .
mkdir -p tools/dev tools/app
cp $here/../check.sh $here/../devjob.zsh $here/../devjob.py tools/dev/
# (A test to run while $tmp/tests is there; its --nextest line as affected-tests.sh's.)
w tools/dev/affected-tests.sh "#!/bin/zsh
[[ -e $tmp/tests ]] || exit 0
[[ \${1:-} == --nextest ]] && print 'cargo nextest run -p toy' || print 'cargo test -p toy'"
w tools/dev/check-reuse.py ''
w tools/app/guards.sh "#!/bin/zsh
[[ -e $tmp/guardsfail ]] && { print 'guards: a stub failure'; exit 1 }; exit 0"
# (The test Mac's runner: refuses a key whose lock $tmp/locks/<key> holds, where remote-tests.sh
# would wait for it; else notes the key.)
w tools/app/remote-tests.sh "#!/bin/zsh
[[ -d $tmp/locks/\$1 ]] && { print \"remote-tests: key \$1 is in use on the test Mac: waiting\"; exit 3 }
print -r -- \$1 > $tmp/key"
# (Each call noted in $tmp/calls. fmt, while $tmp/fmtfail is there: formats docs/a.md and says so,
# as cargo fmt -- -l; while $tmp/fmtnoise is there: a toolchain sync's line on stderr, nothing
# formatted; while $tmp/fmtbroken is there: formats docs/a.md, then fails on a file it can't read. clippy, while $tmp/hold is there: says it started, then waits for $tmp/go, a
# minute at most.)
w tools/dev/bin/cargo "#!/bin/zsh
print -r -- \"\$*\" >> $tmp/calls
[[ \"\$1 \$2\" == 'fmt --' && -e $tmp/fmtfail ]] && { print formatted >> docs/a.md; print -r -- \$PWD/docs/a.md; exit 0 }
[[ \"\$1 \$2\" == 'fmt --' && -e $tmp/fmtnoise ]] && { print -u2 'info: syncing channel updates for 1.98.1-aarch64-apple-darwin'; exit 0 }
[[ \"\$1 \$2\" == 'fmt --' && -e $tmp/fmtbroken ]] && { print formatted >> docs/a.md; print -r -- \$PWD/docs/a.md; print -u2 'error: this file contains an unclosed delimiter'; exit 1 }
[[ \$1 == clippy && -e $tmp/hold ]] || exit 0
: > $tmp/started
for i in {1..600}; do [[ -e $tmp/go ]] && exit 0; sleep 0.1; done
exit 1"
w README.md 1
w docs/a.md 1
g add -A; g commit -m b0
g checkout -q -b br
w docs/a.md 2; g add -A; g commit -m branch; P=$(git rev-parse HEAD)
passed=$tmp/r/.git/check-passed

# run <name> <want: recorded[:<a text the output has>] | a text the output has> [<what to do while
# clippy holds the run>]
run() {
  local name=$1 want=$2 during=${3:-} out rc before after pid i
  rm -f $tmp/started $tmp/go $tmp/hold
  [[ -n $during ]] && : > $tmp/hold
  before=$(cat $passed 2>/dev/null | wc -l)
  HOME=$tmp/home SCENIC_DEVJOB=0 SCENIC_CHECK_LOCAL=1 zsh tools/dev/check.sh > $tmp/out 2>&1 &
  pid=$!
  if [[ -n $during ]]; then
    for i in {1..600}; do [[ -e $tmp/started ]] && break; sleep 0.1; done
    [[ -e $tmp/started ]] || bad "$name: clippy never started"
    eval $during
    : > $tmp/go
  fi
  wait $pid; rc=$?
  out=$(<$tmp/out); after=$(cat $passed 2>/dev/null | wc -l)
  if [[ $want == recorded* ]]; then
    if (( rc == 0 && after == before + 1 )) && [[ $(tail -1 $passed) == "$(git rev-parse HEAD) "*" tree $(git rev-parse 'HEAD^{tree}') "* && $out == *"${want#recorded:}"* ]]; then
      ok "$name: recorded with its tree"
    else bad "$name: wanted a record naming HEAD's tree; exit $rc, records $before -> $after:"; print -r -- $out | tail -8 | sed 's/^/    /'; fi
  elif (( rc != 0 && after == before )) && [[ $out == *"$want"* ]]; then ok "$name: $want"
  else bad "$name: wanted exit != 0, nothing recorded and \"$want\"; exit $rc, records $before -> $after:"; print -r -- $out | tail -8 | sed 's/^/    /'; fi
}

run "a clean run" recorded
w tools/dev/affected-tests.sh '#!/bin/zsh
exit 3'
g add -A; g commit -m broken
run "affected-tests.sh failing" "affected-tests.sh failed"
g checkout main -- tools/dev/affected-tests.sh; g commit -m mended
export CARGO_PROFILE_TEST_OPT_LEVEL=0
g commit --allow-empty -m env
run "the environment setting how tests build" "NOT recorded: the environment sets how the tests build"
unset CARGO_PROFILE_TEST_OPT_LEVEL
run "the same tree again" "recorded:this tree was already checked"
# (A record naming a tree its commit doesn't have vouches for nothing: the run goes on.)
print -r -- "$(git rev-parse main) t tree $(git rev-parse 'HEAD^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $passed
run "a record naming another commit's tree" "recorded:== check.sh summary"
: > $passed
run "HEAD moved by a commit" "HEAD moved during the run" 'g commit --allow-empty -m moved'
g reset -q --hard $P
run "HEAD reset to another commit" "HEAD moved during the run" 'g reset -q --hard main'
g reset -q --hard $P
run "HEAD reset away and back" "files written during the run" 'g reset -q --hard main; g reset -q --hard '$P
run "a file edited and put back" "files written during the run" 'print 3 > docs/a.md; print 2 > docs/a.md'
run "an untracked file appearing" "untracked files" 'print x > new.txt'
run "an untracked file there from the start" "untracked files"
rm new.txt
run "an uncommitted edit" "uncommitted changes" 'print 3 > docs/a.md'
g checkout -q docs/a.md
run "a clean run after" recorded
# While it runs, its git folder's check-running.<pid> holds its start time (report-guard.sh reads
# it, #1522); no run leaves one, whatever way it ended (passed, reused, failed, not recorded).
running() {
  local f=( .git/check-running.<->(N) )
  if (( ${#f} == 1 )) && [[ $(LC_ALL=C TZ=UTC ps -o lstart= -p ${f[1]##*.}) == $(<$f[1]) ]]; then ok "check-running names the live run"
  else bad "while it runs: wanted one check-running.<pid> naming it; have: ${f[*]:-none}"; fi
}
w docs/b.md 1; g add -A; g commit -m running
run "a run, its check-running file" recorded running
f=( .git/check-running.*(N) )
(( ! ${#f} )) && ok "no check-running file left by the runs" || bad "check-running files left: ${f[*]}"

# The stop at a failed part (#1524): fmt formatting a file, or guards failing, and the parts of
# minutes (clippy, prod, the tests) don't run, each saying why; the run fails and records nothing.
# stop <name> <what failed> <texts the output has…>
stop() {
  local out rc before after t has=1
  rm -f $tmp/calls $tmp/key; before=$(cat $passed 2>/dev/null | wc -l)
  HOME=$tmp/home SCENIC_DEVJOB=0 SCENIC_CHECK_LOCAL=1 zsh tools/dev/check.sh > $tmp/out 2>&1; rc=$?
  out=$(<$tmp/out); after=$(cat $passed 2>/dev/null | wc -l)
  for t in ${@:3}; do [[ $out == *"$t"* ]] || has=0; done
  if (( rc != 0 && after == before && has )) && [[ $out == *"clippy ... not run ($2 failed)"* && $out == *"prod ... not run ($2 failed)"* &&
      $out == *"tests ... not run ($2 failed)"* && $out == *"FAIL: $2 failed"* ]] &&
      ! grep -qE '^(clippy|check)' $tmp/calls && [[ ! -e $tmp/key ]]; then
    ok "$1: stopped before clippy, prod and the tests"
  else bad "$1: wanted $2 failed, clippy, prod and the tests not run, nothing recorded; exit $rc, records $before -> $after, calls: $(tr '\n' ';' < $tmp/calls):"
    print -r -- $out | tail -12 | sed 's/^/    /'; fi
}
w docs/c.md 1; g add -A; g commit -m stop; : > $tmp/tests
: > $tmp/fmtfail
stop "fmt formatting a file" fmt "check.sh formatted these files here (cargo fmt):"$'\n'"    docs/a.md"$'\n'"    Commit them"
rm $tmp/fmtfail; g checkout -q docs/a.md
: > $tmp/guardsfail
stop "guards failing" guards 'guards: a stub failure'
: > $tmp/fmtfail
stop "fmt and guards failing" "fmt, guards" "Commit them"
rm $tmp/fmtfail $tmp/guardsfail; g checkout -q docs/a.md
: > $tmp/fmtbroken
stop "fmt failing on a file it can't read" fmt "unclosed delimiter" "cargo fmt failed (above)" "It had formatted these files here already (commit them too):"$'\n'"    docs/a.md"
rm $tmp/fmtbroken; g checkout -q docs/a.md
# (A toolchain sync's stderr is no formatted file: the run passes.)
: > $tmp/fmtnoise; w docs/e.md 1; g add -A; g commit -m noise
run "fmt with a toolchain sync on stderr" recorded
rm $tmp/fmtnoise
run "the tests' run after" recorded
[[ $(<$tmp/key) == main-checkout.check ]] && ok "the main checkout's key: main-checkout.check" || bad "the main checkout's key: $(<$tmp/key)"

# The tests' key (#1524): <worktree>.check, its own; an agent's runs take the worktree's name, and
# a check whose key that run held queued behind it (#1076: 490 s). Its agent's run holding the
# worktree's key, the check doesn't wait.
mkdir -p $tmp/x/.claude/worktrees; g worktree add -q $tmp/x/.claude/worktrees/wt -b wtb
mkdir -p $tmp/locks/wt
cd $tmp/x/.claude/worktrees/wt; w docs/d.md 1; g add -A; g commit -m wt
run "a worktree's check while its agent's run holds the worktree's key" recorded
[[ $(<$tmp/key) == wt.check ]] && ok "a worktree's key: wt.check" || bad "a worktree's key: $(<$tmp/key)"
exit $fail
