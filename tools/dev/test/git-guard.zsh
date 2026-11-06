#!/bin/zsh
# tools/dev/git-guard.py (#980) on a toy repo with worktrees: a git write goes only to the
# session's own checkouts (an agent's worktree; the PM's main checkout, the checkout it started in
# and the worktrees it made); reads and other repos pass. And shell-guard.sh runs it. Run by check.sh.
set -u
here=${0:A:h}
fail=0
tmp=$(mktemp -d); trap 'rm -rf $tmp' EXIT
tmp=${tmp:A}   # (the guard compares real paths: /var is /private/var)
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
r=$tmp/r W=$tmp/r/.claude/worktrees
mkdir -p $r/tools/dev; cd $r
g init -q -b main .
cp $here/../git-guard.py $here/../shellwords.py $here/../shell-guard.sh $r/tools/dev/
print x > README.md; g add -A; g commit -m b0
for a in aaa bbb ddd; do g worktree add -q $W/agent-$a -b br-$a; done
mkdir -p $tmp/other; git -C $tmp/other init -q
proj=$tmp/projects/p; mkdir -p $proj/s1/subagents
print '{"type":"user","cwd":"'$r'"}' > $proj/s1.jsonl
print '{"type":"user","cwd":"'$W/agent-ddd/tools'"}' > $proj/s2.jsonl

# check <want: ok | a text the refusal has> <agent id, or - for the main session> <session> <cwd> <command>
check() {
  local want=$1 agent=$2 s=$3 cwd=$4 cmd=$5 out
  out=$(python3 -c '
import json, sys
a, s, cwd, cmd, proj = sys.argv[1:]
d = {"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": cwd, "session_id": s, "transcript_path": "%s/%s.jsonl" % (proj, s)}
if a != "-": d["agent_id"] = a
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
check $no aaa s1 $own "git worktree remove --force $other"
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

# The main session: the main checkout, the checkout it started in, the worktrees it made.
check ok - s1 $r 'git cherry-pick br-aaa && git merge --ff-only br-bbb && git stash pop'
check ok - s1 $own "cd $r && git reset --hard main"
check $no - s1 $r "git -C $other reset --hard"
check $no - s1 $r "git -C $W/agent-ddd commit -m x"
check ok - s2 $r "git -C $W/agent-ddd commit -m x"   # (s2 started in agent-ddd)
check ok - s1 $r "git worktree add -b int-1 $W/int-1 main"
g worktree add -q -b int-1 $W/int-1 main
check ok - s1 $r "git -C $W/int-1 merge br-aaa && cd $W/int-1 && git reset --hard main"
check $no - s2 $r "git -C $W/int-1 merge br-aaa"   # (another session's)
check $no aaa s1 $own "git -C $W/int-1 merge br-aaa"   # (the session's agents aren't it)
check ok aaa s1 $own "git worktree add $tmp/mine HEAD"   # (an agent's own too)
g worktree add -q $tmp/mine HEAD
check ok aaa s1 $own "git -C $tmp/mine reset --hard HEAD~0"
check $no bbb s1 $other "git -C $tmp/mine reset --hard"
check ok - s1 $r "git worktree add $other"   # (there already: no claim, so still not the PM's)
check $no - s1 $r "git -C $other reset --hard"
# Removing a subagent's worktree: once Claude Code says it finished.
check $no - s1 $r "git worktree remove --force $other"
print '{"finished":false}' > $proj/s1/subagents/agent-bbb.meta.json
check $no - s1 $r "git worktree remove --force $other"
print '{"agentType":"general-purpose","finished":true}' > $proj/s1/subagents/agent-bbb.meta.json
check ok - s1 $r "git worktree remove --force $other"
check $no - s1 $r "git -C $other reset --hard"   # (only its removal)
check $no aaa s1 $own "git worktree remove --force $other"   # (the PM's to remove)

# shell-guard.sh runs the check (exit 2, the reason on stderr).
in=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":"git -C %s reset --hard" % sys.argv[1]},"cwd":sys.argv[2],"agent_id":"aaa","session_id":"s1"}))' $other $own)
out=$(print -r -- $in | zsh $r/tools/dev/shell-guard.sh 2>&1); rc=$?
if (( rc == 2 )) && [[ $out == *"$no"* ]]; then print "ok: shell-guard.sh refuses it"; else print "FAIL: shell-guard.sh: $rc $out"; fail=1; fi
in=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":"git reset --hard"},"cwd":sys.argv[1],"agent_id":"aaa","session_id":"s1"}))' $own)
out=$(print -r -- $in | zsh $r/tools/dev/shell-guard.sh 2>&1); rc=$?
if (( rc == 0 )) && [[ -z $out ]]; then print "ok: shell-guard.sh lets the own worktree's pass"; else print "FAIL: shell-guard.sh: $rc $out"; fail=1; fi
exit $fail
