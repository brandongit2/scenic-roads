#!/bin/zsh
# tools/dev/report-guard.sh (#273, #1522, #1576) on a toy repo with agent worktrees: a stop with
# commits beyond main is refused until check.sh passed on HEAD, except while a check.sh, a
# remote-tests.sh or a devjob.py wrap of its own works in that worktree (its live file and its
# working folder's), while the PM holds runs (.git/hold-runs), or when the agent's last message
# says what blocks it.
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
# (Stand-ins for a run's process.)
sleep 3600 & run=$!
sleep 0 & dead=$!; wait $dead   # (a pid no longer running)
trap 'kill $run 2>/dev/null; rm -rf $tmp' EXIT
export SCENIC_DEVJOB_DIR=$tmp/dj
dj=$SCENIC_DEVJOB_DIR
mkdir -p $dj/live $dj/where
lstart() { local s=$(LC_ALL=C TZ=UTC ps -o lstart= -p $1); print -r -- ${(j: :)${=s}} }
# span <pid> <working folder | -> [<start>]: a live span as devjob.py writes it, its working folder
# beside (none for "-": a cargo's or a part's span)
span() {
  print -r -- "$1 ${3:-$(lstart $1)}" > $dj/live/toy-$1
  [[ $2 == - ]] || print -r -- $2 > $dj/where/toy-$1
}
nospan() { rm -f $dj/live/*(N) $dj/where/*(N) }
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

# A live run working in the worktree: its check.sh, remote-tests.sh or a wrap of its own.
span $run $W/agent-aaa
stop "a stop while a run works in the worktree" pass
print y >> $W/agent-aaa/a.md
stop "uncommitted changes while a run works there" pass
nospan
stop "uncommitted changes, nothing running" "Uncommitted changes"
g -C $W/agent-aaa checkout a.md
span $run $W/agent-aaa/sub
stop "a run working in a folder of the worktree" pass
nospan; span $run $W/agent-aaa-old
stop "a run in a folder whose name begins as the worktree's" $nopass
nospan; span $run $W/agent-bbb
stop "a run working in another worktree" $nopass
nospan; span $run -
stop "a live span with no working folder (a cargo's, a part's)" $nopass
nospan; print -r -- $W/agent-aaa > $dj/where/toy-alone
stop "a working folder with no live span" $nopass
nospan; span $dead $W/agent-aaa "$(lstart $run)"
stop "a run whose process is gone" $nopass
nospan; span $run $W/agent-aaa "Thu Jan 1 00:00:00 1970"
stop "a live pid started at another time (reused)" $nopass
nospan; span $run $W/agent-aaa "$(lstart $run) sample"
stop "a Mac sampled, no span" $nopass
nospan; : > $dj/live/toy-empty; print -r -- $W/agent-aaa > $dj/where/toy-empty
stop "an empty live file" $nopass
nospan

# The PM's hold.
print "the M4 is overloaded: no runs until 15:50" > $r/.git/hold-runs
stop "a stop while the PM holds runs" pass
rm $r/.git/hold-runs

# The agent's last message says what blocks it: a line of its last paragraph begins so (Claude
# Code's last_assistant_message, else the transcript's last assistant text this turn).
msg() { print -r -- ',"last_assistant_message":"'$1'"' }
stop "blocked on a task" pass aaa "$(msg 'Its guard fails until #1548 lands.\n\nBlocked on #1548.')"
stop "waiting for a task" pass aaa "$(msg 'Waiting for #1548 to merge')"
stop "blocked on something named" pass aaa "$(msg 'Report:\n- **Blocked on:** the M4 is away (no ssh).')"
stop "a message naming no blocker" $nopass aaa "$(msg 'All done; the report follows.')"
stop "a task named, not as a blocker" $nopass aaa "$(msg 'Done: #1576 and #1577 are built.')"
stop "a wait with nothing named" $nopass aaa "$(msg 'I am waiting.')"
stop "a left-out entry's blocker" $nopass aaa "$(msg 'Left out: #1610 (blocked by #1600)')"
stop "another task waiting" $nopass aaa "$(msg '#1580 waits on #1576')"
stop "a wait in the past" $nopass aaa "$(msg 'I was waiting for #1548 earlier')"
stop "blocked on nothing" $nopass aaa "$(msg 'Blocked on: nothing')"
stop "a blocker in an earlier paragraph" $nopass aaa "$(msg 'Blocked on #1548.\n\nIt landed; all done now.')"
tr=$tmp/t.jsonl
entry() { print -r -- '{"type":"'$1'","message":{"content":'$2'}}' >> $tr }
: > $tr; entry user '"Build it."'; entry assistant '[{"type":"text","text":"Blocked by #1600."}]'
entry user '[{"type":"tool_result","content":"ok"}]'
stop "blocked, said this turn in the transcript" pass aaa ',"agent_transcript_path":"'$tr'"'
entry user '"Stop hook feedback: run check.sh"'
stop "said in an earlier turn only" $nopass aaa ',"agent_transcript_path":"'$tr'"'
entry assistant '[{"type":"text","text":"Finished."}]'
stop "the transcript's last message names no blocker" $nopass aaa ',"agent_transcript_path":"'$tr'"'

h=$(git -C $W/agent-aaa rev-parse HEAD)
print -r -- "$h 2026-11-19T00:00:00Z tree $(git -C $W/agent-aaa rev-parse 'HEAD^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
stop "HEAD passed" pass
print -r -- "$h 2026-11-19T00:00:00Z tree $(git rev-parse 'main^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
stop "a pass naming another tree" $nopass
exit $fail
