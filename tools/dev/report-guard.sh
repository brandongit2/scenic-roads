#!/bin/zsh
# Claude Code hook (SubagentStop; .claude/settings.local.json; task #273): a subagent working in a
# worktree (.claude/worktrees/agent-<agent_id>) that has commits beyond main can't finish (report)
# until tools/dev/check.sh passed on its current HEAD with a clean tree. Exit 2 shows the reason to
# the subagent, which keeps going. Agents without a worktree, or with nothing committed, pass.
# SubagentStop fires at every turn's end, also one that only waits (#1522, #1576). A stop passes:
#   - while a live dev job span's process works in that worktree (a check.sh, a remote-tests.sh or a
#     devjob.py-wrapped run, whoever started it): a file of this Mac's dev jobs' live folder
#     (tools/dev/devjob.py) naming a process still running as it started, whose working folder is
#     the worktree or in it (the folder devjob.py who reads its checkout from);
#   - while the PM holds runs: the main checkout's .git/hold-runs exists (its text says why);
#   - when the agent's last message says what blocks it: "blocked on #N" (or by, "waiting on/for
#     #N", "waits on/for #N"), or "blocked on: <what>" ("waiting for: <what>").
# It can't recall a report already delivered: the gate that holds is tools/dev/merge-ok.sh.
set -uo pipefail
in=$(cat)
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
said=$(print -r -- $in | python3 -c '
import json, re, sys
d = json.load(sys.stdin)
print(d.get("agent_id") or "")
msg = d.get("last_assistant_message")
if not isinstance(msg, str):  # (else the last text of the transcript, its last assistant entry)
    msg = ""
    try:
        for l in open(d.get("agent_transcript_path") or ""):
            try:
                e = json.loads(l)
            except ValueError:
                continue
            if e.get("type") == "assistant":
                t = "".join(c.get("text", "") for c in e.get("message", {}).get("content", []) if isinstance(c, dict) and c.get("type") == "text")
                msg = t or msg
    except OSError:
        pass
w = r"(?:blocked\s+(?:on|by)|waiting\s+(?:on|for)|waits\s+(?:on|for))"
print("blocked" if re.search(w + r"\s+#\d+|" + w + r":\s*\S", msg, re.I) else "")
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
for f in ${SCENIC_DEVJOB_DIR:-$HOME/Library/Logs/scenic/devjobs}/live/*(N.); do
  read -r pid st < $f 2>/dev/null || continue
  [[ $pid == <-> && $st != *' sample' ]] || continue   # (" sample": a Mac sampled, no span)
  now=$(LC_ALL=C TZ=UTC ps -o lstart= -p $pid 2>/dev/null) || continue
  [[ ${(j: :)${=now}} == ${(j: :)${=st}} ]] || continue   # (as it started: a pid used again isn't it)
  cwd=${${(M)${(f)"$(lsof -a -p $pid -d cwd -Fn 2>/dev/null)"}:#n*}#n}
  [[ -n $cwd && ( ${cwd:A} == ${wt:A} || ${cwd:A} == ${wt:A}/* ) ]] && exit 0
done
if [[ -n $(git -C $wt status --porcelain --untracked-files=no) ]]; then
  print -u2 "Uncommitted changes in $wt: commit them, run tools/dev/check.sh there, then report."
  exit 2
fi
# (A pass naming HEAD's tree, #980: check.sh records nothing when the checkout changed during its run.)
grep -q "^$head .* tree $(git -C $wt rev-parse "$head^{tree}") " $repo/.git/check-passed 2>/dev/null && exit 0
print -u2 "tools/dev/check.sh hasn't passed on HEAD ${head:0:7} of $wt. Run it there (cd $wt && zsh tools/dev/check.sh), fix what fails, commit, and report only after it prints PASS. While a run of yours works in the background, end your turn: its notification wakes you. Blocked by another task or the PM, say so (\"blocked on #N\", \"blocked on: <what>\") and stop. See tools/dev/testing.md."
exit 2
