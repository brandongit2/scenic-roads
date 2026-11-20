#!/bin/zsh
# Claude Code hook (SubagentStop; .claude/settings.local.json; task #273): a subagent working in a
# worktree (.claude/worktrees/agent-<agent_id>) that has commits beyond main can't finish (report)
# until tools/dev/check.sh passed on its current HEAD with a clean tree. Exit 2 shows the reason to
# the subagent, which keeps going. Agents without a worktree, or with nothing committed, pass.
# SubagentStop fires at every turn's end, also one that only waits for a background run (#1522):
# a stop passes while that worktree's check.sh runs (its git folder's check-running.<pid>, naming a
# live process with that start time), unless the agent's last tool call, in its transcript, is
# SubagentHandback: then it handed its work back, and only a pass lets that through.
set -uo pipefail
in=$(cat)
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
# (The agent's id, and 1 when the last tool call in its transcript is SubagentHandback. The
# transcript is read from its end, to the last line with a tool call.)
read -r id handback <<<"$(print -r -- $in | python3 -c '
import json, sys
p = json.load(sys.stdin)
last = ""
try:
    for line in reversed(open(p.get("agent_transcript_path") or "", errors="replace").readlines()):
        if "\"tool_use\"" not in line: continue
        r = json.loads(line)
        c = (r.get("message") or {}).get("content") if r.get("type") == "assistant" else None
        uses = [b.get("name") for b in c if isinstance(b, dict) and b.get("type") == "tool_use"] if isinstance(c, list) else []
        if uses: last = uses[-1]; break
except (OSError, ValueError): pass
print(p.get("agent_id") or "-", int(last == "SubagentHandback"))' 2>/dev/null)"
[[ -n ${id:-} && $id != - ]] || exit 0
wt=$repo/.claude/worktrees/agent-$id
[[ -d $wt ]] || exit 0
head=$(git -C $wt rev-parse HEAD 2>/dev/null) || exit 0
# (commits whose change isn't in main yet; ones already cherry-picked count as in)
[[ -n $(git -C $wt cherry main HEAD 2>/dev/null | grep '^+') ]] || exit 0
if [[ $handback != 1 ]]; then
  for f in $(git -C $wt rev-parse --absolute-git-dir)/check-running.<->(N); do
    [[ $(LC_ALL=C TZ=UTC ps -o lstart= -p ${f##*.} 2>/dev/null) == $(<$f) ]] && exit 0
  done
fi
if [[ -n $(git -C $wt status --porcelain --untracked-files=no) ]]; then
  print -u2 "Uncommitted changes in $wt: commit them, run tools/dev/check.sh there, then report."
  exit 2
fi
# (A pass naming HEAD's tree, #980: check.sh records nothing when the checkout changed during its run.)
grep -q "^$head .* tree $(git -C $wt rev-parse "$head^{tree}") " $repo/.git/check-passed 2>/dev/null && exit 0
print -u2 "tools/dev/check.sh hasn't passed on HEAD ${head:0:7} of $wt. Run it there (cd $wt && zsh tools/dev/check.sh), fix what fails, commit, and report only after it prints PASS. See tools/dev/testing.md."
exit 2
