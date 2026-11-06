#!/bin/zsh
# tools/dev/check.sh on a toy repo (#980): what it records in check-passed. A pass names the tree
# it checked; nothing is recorded when HEAD moved during the run, a file was written or appeared
# then, or the checkout wasn't clean at the start. Its parts are stubs (cargo, guards.sh,
# affected-tests.sh, check-reuse.py); the stub cargo's clippy can hold the run while the test
# changes the checkout. Run by check.sh.
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
w tools/dev/affected-tests.sh '#!/bin/zsh'
w tools/dev/check-reuse.py ''
w tools/app/guards.sh '#!/bin/zsh'
# (clippy, while $tmp/hold is there: says it started, then waits for $tmp/go, a minute at most.)
w tools/dev/bin/cargo "#!/bin/zsh
[[ \$1 == clippy && -e $tmp/hold ]] || exit 0
: > $tmp/started
for i in {1..600}; do [[ -e $tmp/go ]] && exit 0; sleep 0.1; done
exit 1"
w README.md 1
g add -A; g commit -m b0
g checkout -q -b br
w docs/a.md 1; g add -A; g commit -m branch; P=$(git rev-parse HEAD)
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
run "a file edited and put back" "files written during the run" 'print 2 > docs/a.md; print 1 > docs/a.md'
run "an untracked file appearing" "untracked files" 'print x > new.txt'
run "an untracked file there from the start" "untracked files"
rm new.txt
run "an uncommitted edit" "uncommitted changes" 'print 2 > docs/a.md'
g checkout -q docs/a.md
run "a clean run after" recorded
exit $fail
