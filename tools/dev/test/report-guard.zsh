#!/bin/zsh
# tools/dev/report-guard.sh (#273, #1522) on a toy repo with agent worktrees: a stop with commits
# beyond main is refused until check.sh passed on HEAD, except while that worktree's check.sh runs
# (its git folder's check-running.<pid>, a live process with that start time) and the agent's last
# tool call wasn't SubagentHandback. Run by check.sh.
set -u
here=${0:A:h}
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }
tmp=$(mktemp -d)
sleep 3600 & live=$!   # (stands in for a running check.sh)
sleep 0 & dead=$!; wait $dead   # (a pid no longer running)
trap 'kill $live 2>/dev/null; rm -rf $tmp' EXIT
tmp=${tmp:A}
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
r=$tmp/r W=$tmp/r/.claude/worktrees
mkdir -p $r/tools/dev; cd $r
g init -q -b main .
cp $here/../report-guard.sh $r/tools/dev/
print x > README.md; g add -A; g commit -m b0
for a in aaa bbb; do g worktree add -q $W/agent-$a -b br-$a; done
print y > $W/agent-aaa/a.md; g -C $W/agent-aaa add -A; g -C $W/agent-aaa commit -m work
lstart() { LC_ALL=C TZ=UTC ps -o lstart= -p $1 }
gitdir() { git -C $W/agent-$1 rev-parse --absolute-git-dir }
# transcript <last tool>: the agent's transcript, its last tool call that one (an assistant
# record), then a tool result and a closing text, as Claude Code writes them.
transcript() {
  python3 -c '
import json, sys
recs = [
  {"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "t0", "name": "Bash", "input": {}}]}},
  {"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t0", "content": "x"}]}},
  {"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "now"}, {"type": "tool_use", "id": "t1", "name": sys.argv[1], "input": {}}]}},
  {"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}},
  {"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "Waiting."}]}},
]
print("\n".join(json.dumps(x) for x in recs))' $1 > $tmp/transcript.jsonl
}
# stop <name> <want: pass | a text the refusal has> [<agent id>]: the hook on a SubagentStop for
# that agent (default aaa), with the transcript as it is.
stop() {
  local name=$1 want=$2 id=${3-aaa} out rc
  out=$(print -r -- '{"hook_event_name":"SubagentStop","agent_id":"'$id'","agent_transcript_path":"'$tmp'/transcript.jsonl","background_tasks":[]}' |
    zsh $r/tools/dev/report-guard.sh 2>&1); rc=$?
  if [[ $want == pass ]]; then (( rc == 0 )) && ok "$name: passes" || bad "$name: wanted a pass; exit $rc: $out"
  elif (( rc == 2 )) && [[ $out == *"$want"* ]]; then ok "$name: refused"
  else bad "$name: wanted exit 2 and \"$want\"; exit $rc: $out"; fi
}
nopass="hasn't passed on HEAD"

transcript Bash
stop "no pass, no check.sh running" $nopass
stop "an agent with nothing beyond main" pass bbb
stop "an agent without a worktree" pass zzz
stop "no agent id" pass ''
lstart $live > $(gitdir aaa)/check-running.$live
stop "a stop while the worktree's check.sh runs" pass
transcript SubagentHandback
stop "a hand-back while the worktree's check.sh runs" $nopass
transcript Bash
print y >> $W/agent-aaa/a.md
stop "uncommitted changes while check.sh runs" pass
rm $(gitdir aaa)/check-running.$live
stop "uncommitted changes, no check.sh running" "Uncommitted changes"
g -C $W/agent-aaa checkout a.md
lstart $live > $(gitdir aaa)/check-running.$dead
stop "a check-running file whose pid is gone" $nopass
rm $(gitdir aaa)/check-running.$dead
print "Thu Jan  1 00:00:00 1970" > $(gitdir aaa)/check-running.$live
stop "a live pid started at another time (reused)" $nopass
rm $(gitdir aaa)/check-running.$live
lstart $live > $(gitdir bbb)/check-running.$live
stop "another worktree's check.sh running" $nopass
rm $(gitdir bbb)/check-running.$live
lstart $live > $(gitdir aaa)/check-running.$live
rm $tmp/transcript.jsonl
stop "no transcript, check.sh running" pass
rm $(gitdir aaa)/check-running.$live
h=$(git -C $W/agent-aaa rev-parse HEAD)
print -r -- "$h 2026-11-19T00:00:00Z tree $(git -C $W/agent-aaa rev-parse 'HEAD^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
transcript Bash
stop "HEAD passed" pass
transcript SubagentHandback
stop "HEAD passed, handed back" pass
print -r -- "$h 2026-11-19T00:00:00Z tree $(git rev-parse 'main^{tree}') base $(git rev-parse 'main^{tree}') at $(git rev-parse main)" > $r/.git/check-passed
stop "a pass naming another tree" $nopass
exit $fail
