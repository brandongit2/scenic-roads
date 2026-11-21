#!/bin/zsh
# Claude Code hook (SubagentStop; .claude/settings.local.json; task #273): a subagent working in a
# worktree (.claude/worktrees/agent-<agent_id>) that has commits beyond main can't finish (report)
# until tools/dev/check.sh passed on its current HEAD with a clean tree. Exit 2 shows the reason to
# the subagent, which keeps going. Agents without a worktree, or with nothing committed, pass.
# SubagentStop fires at every turn's end, also one that only waits (#1522, #1576). A stop passes:
#   - while a check.sh, a remote-tests.sh or another devjob.py wrap of its own (no parent span)
#     works in that worktree, whoever started it: its span's working folder (tools/dev/devjob.py
#     WHERE, devjob.zsh dj_where) is the worktree or in it, and its live file names a process still
#     running as it started (a cargo's span, or a part's, counts not);
#   - while the PM holds runs: the main checkout's .git/hold-runs exists (its text says why);
#   - when a line of the last paragraph of the agent's last message (Claude Code's
#     last_assistant_message, else the transcript's last assistant text this turn) begins "blocked
#     on #N" (or by, "waiting on/for", "waits on/for"), or "blocked on: <what>" (not "nothing").
# It can't recall a report already delivered: the gate that holds is tools/dev/merge-ok.sh.
set -uo pipefail
in=$(cat)
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
said=$(print -r -- $in | python3 -c '
import json, re, sys
d = json.load(sys.stdin)
print(d.get("agent_id") or "")
def text(e):
    c = e.get("message", {}).get("content", [])
    return c if isinstance(c, str) else "".join(x.get("text", "") for x in c if isinstance(x, dict) and x.get("type") == "text")
msg = d.get("last_assistant_message")
if not isinstance(msg, str):  # (else the transcript: the last assistant text of the current turn)
    msg = ""
    try:
        for l in open(d.get("agent_transcript_path") or ""):
            try:
                e = json.loads(l)
            except ValueError:
                continue
            if e.get("type") == "user" and text(e):  # (a turn begins: a message, not a tool result)
                msg = ""
            elif e.get("type") == "assistant":
                msg = text(e) or msg
    except OSError:
        pass
# (A line of the last paragraph that begins with it, naming a task or what blocks.)
last = re.split(r"\n\s*\n", msg.strip())[-1]
w = r"^[\s>*_-]*(?:blocked\s+(?:on|by)|waiting\s+(?:on|for)|waits\s+(?:on|for))(?:\s+#\d+|:[\s*_]*(?!(?:nothing|none|n/?a)\b)\w)"
print("blocked" if re.search(w, last, re.I | re.M) else "")
' 2>/dev/null)
said=( "${(@f)said}" ); id=${said[1]:-}
[[ -n $id ]] || exit 0
wt=$repo/.claude/worktrees/agent-$id
[[ -d $wt ]] || exit 0
head=$(git -C $wt rev-parse HEAD 2>/dev/null) || exit 0
# (commits whose change isn't in main yet; ones already cherry-picked count as in)
[[ -n $(git -C $wt cherry main HEAD 2>/dev/null | grep '^+') ]] || exit 0
[[ -e $repo/.git/hold-runs ]] && exit 0
[[ ${said[2]:-} == blocked ]] && exit 0
dj=${SCENIC_DEVJOB_DIR:-$HOME/Library/Logs/scenic/devjobs}
for f in $dj/where/*(N.); do
  read -r cwd < $f 2>/dev/null || continue
  [[ -n $cwd && ( $cwd == ${wt:A} || $cwd == ${wt:A}/* ) ]] || continue
  read -r pid st < $dj/live/${f:t} 2>/dev/null || continue
  [[ $pid == <-> ]] || continue
  now=$(LC_ALL=C TZ=UTC ps -o lstart= -p $pid 2>/dev/null) || continue
  # (As it started: a pid used again isn't it.)
  [[ ${(j: :)${=now}} == ${(j: :)${=st}} ]] && exit 0
done
if [[ -n $(git -C $wt status --porcelain --untracked-files=no) ]]; then
  print -u2 "Uncommitted changes in $wt: commit them, run tools/dev/check.sh there, then report."
  exit 2
fi
# (A pass naming HEAD's tree, #980: check.sh records nothing when the checkout changed during its run.)
grep -q "^$head .* tree $(git -C $wt rev-parse "$head^{tree}") " $repo/.git/check-passed 2>/dev/null && exit 0
print -u2 "tools/dev/check.sh hasn't passed on HEAD ${head:0:7} of $wt. Run it there (cd $wt && zsh tools/dev/check.sh), fix what fails, commit, and report only after it prints PASS. While a run of yours works in the background, end your turn: its notification wakes you. Blocked by another task or the PM, say so (\"blocked on #N\", \"blocked on: <what>\") and stop. See tools/dev/testing.md."
exit 2
