#!/bin/zsh
# tools/dev/report-guard.sh (#273, #1522, #1576) on a toy repo with agent worktrees: a stop with
# commits beyond main is refused until check.sh passed on HEAD, except while a live dev job span's
# process works in that worktree (a file of the dev jobs' live folder: its pid and start time), while
# the PM holds runs (.git/hold-runs), or when the agent's last message says what blocks it.
# Run by check.sh.
set -u
here=${0:A:h}
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }
tmp=$(mktemp -d)
tmp=${tmp:A}
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
r=$tmp/r W=$tmp/r/.claude/worktrees
mkdir -p $r/tools/dev; cd $r
g init -q -b main .
cp $here/../report-guard.sh $r/tools/dev/
print x > README.md; g add -A; g commit -m b0
for a in aaa bbb; do g worktree add -q $W/agent-$a -b br-$a; done
mkdir -p $W/agent-aaa/sub
print y > $W/agent-aaa/a.md; g -C $W/agent-aaa add -A; g -C $W/agent-aaa commit -m work
# (Stand-ins for a run's process: working in the worktree, in a folder of it, in another worktree.)
(cd $W/agent-aaa && exec sleep 3600) & inwt=$!
(cd $W/agent-aaa/sub && exec sleep 3600) & insub=$!
(cd $W/agent-bbb && exec sleep 3600) & other=$!
sleep 0 & dead=$!; wait $dead   # (a pid no longer running)
trap 'kill $inwt $insub $other 2>/dev/null; rm -rf $tmp' EXIT
export SCENIC_DEVJOB_DIR=$tmp/dj
mkdir -p $SCENIC_DEVJOB_DIR/live
lstart() { local s=$(LC_ALL=C TZ=UTC ps -o lstart= -p $1); print -r -- ${(j: :)${=s}} }
span() { print -r -- "$1 ${2:-$(lstart $1)}" > $SCENIC_DEVJOB_DIR/live/toy-$1 }   # span <pid> [<start>]: a live file, as devjob.py writes it
nospan() { rm -f $SCENIC_DEVJOB_DIR/live/*(N) }
# stop <name> <want: pass | a text the refusal has> [<agent id> [<more JSON fields>]]: the hook on a
# SubagentStop for that agent (default aaa).
stop() {
  local name=$1 want=$2 id=${3-aaa} more=${4:-} out rc
  out=$(print -r -- '{"hook_event_name":"SubagentStop","agent_id":"'$id'","background_tasks":[]'$more'}' |
    zsh $r/tools/dev/report-guard.sh 2>&1); rc=$?
  if [[ $want == pass ]]; then (( rc == 0 )) && ok "$name: passes" || bad "$name: wanted a pass; exit $rc: $out"
  elif (( rc == 2 )) && [[ $out == *"$want"* ]]; then ok "$name: refused"
  else bad "$name: wanted exit 2 and \"$want\"; exit $rc: $out"; fi
}
nopass="hasn't passed on HEAD"

stop "no pass, nothing running" $nopass
stop "an agent with nothing beyond main" pass bbb
stop "an agent without a worktree" pass zzz
stop "no agent id" pass ''

# A live span working in the worktree: its check.sh, remote-tests.sh or a wrapped run.
span $inwt
stop "a stop while a span works in the worktree" pass
print y >> $W/agent-aaa/a.md
stop "uncommitted changes while a span works there" pass
nospan
stop "uncommitted changes, nothing running" "Uncommitted changes"
g -C $W/agent-aaa checkout a.md
span $insub
stop "a span working in a folder of the worktree" pass
nospan; span $other
stop "a span working in another worktree" $nopass
nospan; span $dead "$(lstart $inwt)"
stop "a span whose process is gone" $nopass
nospan; span $inwt "Thu Jan 1 00:00:00 1970"
stop "a live pid started at another time (reused)" $nopass
nospan; span $inwt "$(lstart $inwt) sample"
stop "a Mac sampled, no span" $nopass
nospan; : > $SCENIC_DEVJOB_DIR/live/toy-empty
stop "an empty live file" $nopass
nospan

# The PM's hold.
print "the M4 is overloaded: no runs until 15:50" > $r/.git/hold-runs
stop "a stop while the PM holds runs" pass
rm $r/.git/hold-runs

# The agent's last message says what blocks it (Claude Code's last_assistant_message, else the
# transcript's last assistant text).
msg() { print -r -- ',"last_assistant_message":"'$1'"' }
stop "blocked on a task" pass aaa "$(msg 'Its guard fails until #1548 lands. Blocked on #1548.')"
stop "waiting for a task" pass aaa "$(msg 'Waiting for #1548 to merge')"
stop "blocked on something named" pass aaa "$(msg 'Blocked on: the M4 is away (no ssh).')"
stop "a message naming no blocker" $nopass aaa "$(msg 'All done; the report follows.')"
stop "a task named, not as a blocker" $nopass aaa "$(msg 'Done: #1576 and #1577 are built.')"
stop "a wait with nothing named" $nopass aaa "$(msg 'I am waiting.')"
print -r -- '{"type":"assistant","message":{"content":[{"type":"text","text":"Blocked by #1600."}]}}' > $tmp/t.jsonl
print -r -- '{"type":"user","message":{"content":"Stop hook feedback"}}' >> $tmp/t.jsonl
stop "blocked, said in the transcript" pass aaa ',"agent_transcript_path":"'$tmp/t.jsonl'"'
print -r -- '{"type":"assistant","message":{"content":[{"type":"text","text":"Finished."}]}}' >> $tmp/t.jsonl
stop "the transcript's last message names no blocker" $nopass aaa ',"agent_transcript_path":"'$tmp/t.jsonl'"'

h=$(git -C $W/agent-aaa rev-parse HEAD)
print -r -- "$h 2026-11-19T00:00:00Z tree $(git -C $W/agent-aaa rev-parse 'HEAD^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
stop "HEAD passed" pass
print -r -- "$h 2026-11-19T00:00:00Z tree $(git rev-parse 'main^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
stop "a pass naming another tree" $nopass
exit $fail
