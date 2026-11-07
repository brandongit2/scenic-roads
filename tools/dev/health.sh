#!/bin/zsh
# The PM's health check of the build and the agents. Prints what needs attention, and stamps the
# time (.claude/health-stamp): the next check's failures are those since this one.
#
# The build's part is one `scenic status --flags` (crates/pipeline/src/agent/flags.rs): the lead's
# status covers every Mac, and it prints only what needs a look, a line each: its flags ("idle:" a
# Mac with a slot free while work it could take waits, with what in its capacity held the job back,
# or the waits its remaining work is held behind; "stuck:" a job whose work hasn't moved, by its
# beats, for longer than the pool's overdue factor times its step's usual stillness, with where it
# is and its batch's progress; "slow:" a job whose progress goes on, over the factor times its
# step's usual longest wait for progress, at less than its step's slowest tenth of jobs divided by
# the factor; "stale:" a catalogue due that hasn't gone out), the pool's
# warnings ("warning:" no
# lead, the lead or a member out of touch, the lead's loop stuck) and the jobs failed since the last
# check's stamp ("failed:"). The same flags are on the build page's alerts and the menu bar.
set -uo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) print "usage: tools/dev/health.sh   (no arguments)"; exit 0 ;;
  *) print -u2 "health.sh takes no arguments: $*"; exit 2 ;;
esac
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
cd $repo || exit 1
# (No stamp yet: the failures of the last 6 hours, scenic status --flags's own default.)
since=$(cat .claude/health-stamp 2>/dev/null)
date +%s > .claude/health-stamp
print "== health $(date '+%F %T')"
df -h / | awk 'NR==2{print "disk: "$4" free of "$2}'
print "NAS: $([[ -d /Volumes/personal/projects/scenic-roads/map ]] && echo mounted || echo NOT MOUNTED)"
from="6 h ago"; [[ -n $since ]] && from=$(date -r $since '+%F %T')
print "the build (scenic status --flags, failures since $from):"
if out=$(scenic status --flags ${since:+--since} ${since:+$since} 2>&1); then
  print -r -- "${out:-  nothing flagged}" | sed 's/^\([^ ]\)/  \1/'
else
  print -r -- "  STATUS UNREADABLE: $out"
fi
print "agent worktrees (newest file change):"
for wt in .claude/worktrees/*(N/); do
  n=$(find $wt -maxdepth 3 -not -path '*/target/*' -not -path '*/node_modules/*' -type f -newermt "30 minutes ago" 2>/dev/null | head -1)
  [[ -n $n ]] && print "  ${wt:t}: active (changed in the last 30 min)"
done
tools/dev/sweep-targets.sh
tail -3 .claude/sweep-targets.log 2>/dev/null | sed 's/^/sweep: /'
print "== also: check every running subagent and background task, update the board (stage, history, status line), email the owner if anything notable"
print "gates (.claude/gates.txt):"
grep -v "^# " .claude/gates.txt | grep -v '^$' | while IFS='|' read -r task what test; do
  if zsh -c "$test" >/dev/null 2>&1; then s="READY  <-- ACT NOW"; else s="waiting"; fi
  print "  ${task% } — ${what# }: $s"
done
print "long runs (tools/dev/longrun.sh; flags: gone without a final line, log quiet, restarting repeatedly):"
tools/dev/longrun.sh check
print "this Mac's own scenic-build jobs:"
# (None is no failure: grep finding nothing fails the pipeline under pipefail, so it's said.)
own=$(ps -axo pid,lstart,command | grep "[a]pp/current/scenic-build\|[s]cenic/app/.*/scenic-build" | awk '{printf "  pid %s since %s %s %s  ", $1, $3, $4, $5; for(i=7;i<=9;i++) printf "%s ", $i; print ""}' | cut -c1-160)
print -r -- "${own:-  none}"
