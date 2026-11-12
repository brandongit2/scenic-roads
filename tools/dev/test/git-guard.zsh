#!/bin/zsh
# tools/dev/git-guard.py (#980) on a toy repo with worktrees: a git write goes only to the
# session's own checkouts (an agent's worktree; the PM's main checkout, the checkout it started in
# and the worktrees it made); reads and other repos pass. And shell-guard.sh runs it. Run by check.sh.
set -u
here=${0:A:h}
fail=0
tmp=$(mktemp -d)
sleep 3600 & live=$!   # (stands in for session s1's Claude Code process)
trap 'kill $live 2>/dev/null; rm -rf $tmp' EXIT
tmp=${tmp:A}   # (the guard compares real paths: /var is /private/var)
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
r=$tmp/r W=$tmp/r/.claude/worktrees
mkdir -p $r/tools/dev; cd $r
g init -q -b main .
cp $here/../git-guard.py $here/../shellwords.py $here/../shell-guard.sh $r/tools/dev/
print x > README.md; g add -A; g commit -m b0
for a in aaa bbb fff ddd abc; do g worktree add -q $W/agent-$a -b br-$a; done
mkdir -p $tmp/other; git -C $tmp/other init -q
proj=$tmp/projects/p; mkdir -p $proj $tmp/sessions
# session <session> <pid> [<seconds ago>]: Claude Code's record of the process running it, started
# then (<config>/sessions/<pid>.json; <config>/projects/<project> holds the transcripts).
session() {
  python3 -c '
import json, sys, time
reg, s, pid, ago, lstart = sys.argv[1:]
json.dump({"pid": int(pid), "sessionId": s, "procStart": lstart, "startedAt": int((time.time() - float(ago)) * 1000)}, open("%s/%s.json" % (reg, pid), "w"))' \
    $tmp/sessions $1 $2 ${3:-0} "$(LC_ALL=C TZ=UTC ps -o lstart= -p $2)"
}
session s1 $live 7200
# agent <id> <session> <seconds from now> [finished | <text>]: Claude Code's files for a subagent:
# its meta (finished: true once it finished) and an entry appended to its transcript.
agent() {
  mkdir -p $proj/$2/subagents
  python3 -c '
import json, sys, datetime
b, off, what = sys.argv[1], float(sys.argv[2]), sys.argv[3]
json.dump({"agentType": "general-purpose", **({"finished": True} if what == "finished" else {})}, open(b + ".meta.json", "w"))
t = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=off)
e = {"type": "user", "timestamp": t.isoformat().replace("+00:00", "Z"), "message": {"role": "user", "content": [{"type": "text", "text": what}]}}
open(b + ".jsonl", "a").write(json.dumps(e) + "\n")' $proj/$2/subagents/agent-$1 $3 ${4:-working}
}
agent aaa s1 1; agent bbb s1 1   # (both running)

# check <want: ok | a text the refusal has> <agent id, or - for the main session> <session> <cwd> <command>
check() {
  local want=$1 agent=$2 s=$3 cwd=$4 cmd=$5 out
  out=$(python3 -c '
import json, sys
a, s, cwd, cmd, proj = sys.argv[1:]
d = {"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": cwd, "session_id": s, "transcript_path": "%s/%s.jsonl" % (proj, s)}
if a != "-": d["agent_id"] = a; d["transcript_path"] = "%s/%s/subagents/agent-%s.jsonl" % (proj, s, a)
print(json.dumps(d))' $agent $s $cwd $cmd $proj | python3 $r/tools/dev/git-guard.py 2>&1)
  if [[ $want == ok && -z $out ]] || [[ $want != ok && -n $want && $out == *"$want"* ]]; then print "ok ($want): [$agent] $cmd"
  else print "FAIL ($want): [$agent in $cwd] $cmd\n  ${out:-(allowed)}"; fail=1; fi
}
own=$W/agent-aaa other=$W/agent-bbb no="isn't this session's"

# An agent: its own worktree only.
check ok aaa s1 $own 'git reset --hard HEAD'
check ok aaa s1 $own "git -C $own commit -m x && git -C $own/tools stash push -m t"
check ok aaa s1 $r "cd $own && git rebase main"
check $no aaa s1 $own "git -C $other reset --hard HEAD"
check $no aaa s1 $own "cd $other && git reset --hard HEAD && tools/dev/check.sh"
check $no aaa s1 $own "cd $other/tools; git checkout main"
check $no aaa s1 $own "git -C $W -C agent-bbb switch main"
check $no aaa s1 $own "git -C $r merge br-aaa"   # (the main checkout is the PM's)
check $no aaa s1 $own "git --work-tree=$other reset --hard"
check $no aaa s1 $own "GIT_WORK_TREE=$other git clean -fd"
check $no aaa s1 $own "git --git-dir=$r/.git/worktrees/agent-bbb reset --hard"
check $no aaa s1 $own "git -C $other stash"
check $no aaa s1 $own "git -C $other stash pop"
check "removed or moved by the main session" aaa s1 $own "git worktree remove --force $other"
check $no aaa s1 $own "zsh -c 'cd $other && git commit -am x'"
check $no aaa s1 $own "w=$other; git -C \$w reset --hard"
check "Can't tell" aaa s1 $own 'git -C $UNSET_WT reset --hard'
check "Can't tell" aaa s1 $own 'cd "$(git rev-parse --show-toplevel)/../agent-bbb" && git commit -m x'
# An agent started without a worktree, in another's (6 Nov): nothing of its own there.
check $no ccc s1 $other 'git reset --hard 9730575e && tools/dev/check.sh'
check $no ccc s1 $r 'git cherry-pick br-bbb'
# Reads, another repo, text that isn't a command: not refused.
check ok aaa s1 $own "git -C $other log -1 && git -C $other status && git -C $other diff main"
check ok aaa s1 $own "git -C $other stash list; git -C $other branch -a; git -C $other worktree list"
check ok aaa s1 $own "git -C $tmp/other reset --hard; cd $tmp/other && git commit -m x"
check ok aaa s1 $own "echo 'git -C $other reset --hard'; grep -n 'git reset' x"
check ok aaa s1 $own "cat > f <<'EOF'
git -C $other reset --hard
EOF"
check ok aaa s1 $own "git -C $tmp/nowhere reset --hard"

# Ways to another folder on the line.
check ok aaa s1 $own "cd $own/tools && git add -A && git commit -m x"   # (a subfolder of its own)
check ok aaa s1 $own "pushd $other; popd; git reset --hard"
check $no aaa s1 $other "pushd $own && popd && git reset --hard"
check $no aaa s1 $own "env -C $other git reset --hard"
check $no aaa s1 $own "env --chdir $other git reset --hard"
check $no aaa s1 $own "export GIT_WORK_TREE=$other; git reset --hard"
check ok aaa s1 $own 'git -C $(pwd) commit -m x && git -C "$PWD" commit -m y'
check $no aaa s1 $own "cd $other && git -C \"\$(pwd)\" commit -m x"
check "Can't tell" aaa s1 $own "git -C \$(echo $other)/tools reset --hard"
check $no aaa s1 $own "echo \$(git -C $other reset --hard) \`git -C $other clean -fd\`"
check ok aaa s1 $own "git -C $other rev-parse HEAD && git -C $other merge-base main HEAD && git -C $other show HEAD"

# The main session (any: a resumed or forked one has a new session id): every checkout but a
# subagent's.
check ok - s1 $r 'git cherry-pick br-aaa && git merge --ff-only br-bbb && git stash pop && git push origin main'
check ok - s1 $own "cd $r && git reset --hard main"
check $no - s1 $r "git -C $other reset --hard"
check ok - s1 $r "git worktree add -b int-1 .claude/worktrees/int-1 main"
g worktree add -q -b int-1 $W/int-1 main
check ok - s1 $r "cd .claude/worktrees/int-1 && git checkout -b int-1b && git rebase main && tools/dev/check.sh"
check ok - s1 $W/int-1 "git checkout -b int-1c && git rebase main"   # (a later call, the cwd carried)
check ok - s1 $r "git -C .claude/worktrees/int-1 rebase main"
check ok - s2 $r "git -C $W/int-1 merge br-aaa"   # (the session resumed: a new id)
g worktree add -q -b int-2 $W/int-2 main   # (made before the guard: no record)
check ok - s1 $r "git -C $W/int-2 rebase main"
check $no aaa s1 $own "git -C $W/int-1 merge br-aaa"   # (subagents: their own only)
check ok aaa s1 $own "git worktree add $tmp/mine HEAD"   # (an agent's own too)
g worktree add -q $tmp/mine HEAD
check ok aaa s1 $own "git -C $tmp/mine reset --hard HEAD~0"
check $no bbb s1 $other "git -C $tmp/mine reset --hard"
check $no - s1 $r "git -C $tmp/mine reset --hard"   # (its agent runs)
check ok - s1 $r "git worktree add $other"   # (there already: no claim)
check $no - s1 $r "git -C $other reset --hard"
# A finished agent's worktree: free to the next session writing there (a reviewer, the PM), not
# while a running subagent works there; removed by the PM.
check $no - s1 $r "git worktree remove --force $other"
check $no ccc s1 $r "git -C $other rebase main"
agent bbb s1 2 finished; agent ccc s1 1; agent eee s1 1
check ok ccc s1 $r "cd $other && git rebase main && git commit -m review"   # (the reviewer takes it)
check "a running subagent (ccc)" eee s1 $r "git -C $other reset --hard"
check "a running subagent (ccc)" - s1 $r "git worktree remove --force $other"
check ok ccc s1 $r "git -C $other commit -m again"
check "removed or moved by the main session" ccc s1 $r "git worktree remove --force $other"
agent ccc s1 2 finished
check ok eee s1 $r "git -C $other reset --hard"   # (ccc finished: eee takes it)
agent eee s1 2 finished
check ok - s1 $r "git -C $other reset --hard && git worktree remove --force $other"
check ok - s1 $r "git worktree remove --force .claude/worktrees/int-1"
check $no - s1 $r "git -C $own reset --hard"   # (aaa runs)

# Running is told from liveness (#1153), not from the meta alone: an agent stopped with TaskStop,
# cut off by its session's restart, or of a session no process runs any more never gets
# finished: true.
agent fff s1 1
check "its agent (fff) is still running" - s1 $r "git worktree remove --force $W/agent-fff"
agent fff s1 2 '[Request interrupted by user for tool use]'   # (TaskStop's mark, its last entry)
check ok - s1 $r "git worktree remove --force $W/agent-fff"
agent ddd s1 2
check "its agent (ddd) is still running" - s1 $r "git -C $W/agent-ddd reset --hard"
agent ddd s1 3 '[Request interrupted by user]'; agent ddd s1 4   # (stopped, then resumed by SendMessage)
check "its agent (ddd) is still running" - s1 $r "git worktree remove --force $W/agent-ddd"
agent ddd s1 -3600   # (idle for an hour, waiting on a background run; s1's process has run for two)
touch -t 202601010000 $proj/s1/subagents/agent-ddd.*   # (its files' times don't count)
check "its agent (ddd) is still running" - s1 $r "git worktree remove --force $W/agent-ddd"
sleep 1000 & s0=$!   # (another session, s0, and its agent abc)
session s0 $s0; agent abc s0 1
check ok abc s0 $W/agent-abc "git worktree add $tmp/abc-own HEAD"
g worktree add -q $tmp/abc-own HEAD
check "its agent (abc) is still running" - s1 $r "git worktree remove --force $W/agent-abc"
check "a subagent (abc) made it" - s1 $r "git -C $tmp/abc-own reset --hard"
python3 -c 'import json,sys; f=sys.argv[1]; d=json.load(open(f)); d["procStart"]="Thu Jan  1 00:00:00 2026"; json.dump(d,open(f,"w"))' $tmp/sessions/$s0.json
check ok - s1 $r "git worktree remove --force $W/agent-abc"   # (s0's pid is another process's now)
session s0 $s0; kill $s0; wait $s0 2>/dev/null   # (s0 ended without removing its record)
check ok - s1 $r "git worktree remove --force $W/agent-abc"
check ok - s1 $r "git -C $tmp/abc-own reset --hard"   # (made by abc, which no longer runs)
check $no ccc s1 $r "git -C $tmp/abc-own reset --hard"   # (still not another subagent's to write in)
session s1 $live   # (s1 restarted: its process started now, after ddd's last entry)
check ok - s1 $r "git worktree remove --force $W/agent-ddd"

# shell-guard.sh runs the check (exit 2, the reason on stderr).
in=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":"git -C %s reset --hard" % sys.argv[1]},"cwd":sys.argv[2],"agent_id":"aaa","session_id":"s1"}))' $other $own)
out=$(print -r -- $in | zsh $r/tools/dev/shell-guard.sh 2>&1); rc=$?
if (( rc == 2 )) && [[ $out == *"$no"* ]]; then print "ok: shell-guard.sh refuses it"; else print "FAIL: shell-guard.sh: $rc $out"; fail=1; fi
in=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":"git reset --hard"},"cwd":sys.argv[1],"agent_id":"aaa","session_id":"s1"}))' $own)
out=$(print -r -- $in | zsh $r/tools/dev/shell-guard.sh 2>&1); rc=$?
if (( rc == 0 )) && [[ -z $out ]]; then print "ok: shell-guard.sh lets the own worktree's pass"; else print "FAIL: shell-guard.sh: $rc $out"; fail=1; fi
# The ln rule (#1004): `ln -s` without -n/-h refused as any command of the line; -sn, -sfn, -sfh
# pass, and so does `ln -s` in quoted text, a comment or a heredoc.
for want cmd in \
  "No \`ln -s\` without" "ln -s /srv/x/inputs root/inputs" \
  "No \`ln -s\` without" "for e in /srv/x/*; do ln -s \$e r/\${e:t}; done" \
  "No \`ln -s\` without" "mkdir r && ln -sf a r/b" \
  "No \`ln -s\` without" "sudo ln -s a b" \
  "No \`ln -s\` without" $'cd r\nln -s a b' \
  "No \`ln -s\` without" "x=\$(ln --symbolic a b)" \
  "No \`ln -s\` without" "sh -c 'ln -s a b'" \
  ok "ln -sn a b" ok "ln -sfn a b" ok "ln -s -h a b" ok "ln -sfh v1 app/current" ok "ln a b" ok "echo ln -s a b" \
  ok "ln -s --no-dereference a b" ok "echo 'x; (ln -s a b)'" ok "echo hi # then ln -s a b" \
  ok $'cat > f <<EOF\na; ln -s a b\nEOF'; do
  in=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]},"cwd":sys.argv[2],"session_id":"s1"}))' $cmd $own)
  out=$(print -r -- $in | zsh $r/tools/dev/shell-guard.sh 2>&1); rc=$?
  if [[ $want == ok && $rc == 0 ]] || [[ $want != ok && $rc == 2 && $out == *"$want"* ]]; then
    print "ok ($want): $cmd"
  else
    print "FAIL ($want): $cmd: $rc $out"; fail=1
  fi
done
exit $fail
