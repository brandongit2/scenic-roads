#!/bin/zsh
# The PM's health check of the build and the agents. Prints what needs attention, and stamps the
# time (.claude/health-stamp): the next check's failures are those since this one. It starts with the
# PM's own list for the check (.claude/next-check.md), which it then clears.
#
# The build's part is one `scenic status --flags` (crates/agentcore/src/agent/flags.rs): the lead's
# status covers every Mac, and it prints only what needs a look, a line each: its flags ("idle:" a
# Mac with a slot free while work it could take waits, with what in its capacity held the job back,
# or the waits its remaining work is held behind; "stuck:" a job whose work hasn't moved, by its
# beats, for longer than the pool's overdue factor times its step's usual stillness, with where it
# is and its batch's progress; "slow:" a job whose progress goes on, over the factor times its
# step's usual longest wait for progress, at less than its step's slowest tenth of jobs divided by
# the factor; "stale:" a map update due that hasn't gone out; "repeat:" a GC or backup run that did
# nothing soon after the one before; "refused:" a step's targets whose hand-offs the lead's merge refused
# twice in a row or more), the pool's
# warnings ("warning:" no
# lead, the lead or a member out of touch, the lead's loop stuck) and the jobs failed since the last
# check's stamp ("failed:"). The same flags are on the build page's alerts and the menu bar.
#
# Links on the NAS (#1004): none belongs in the project folder, and a walker that follows one to an
# ancestor loops (the backup's "Too many levels of symbolic links", 7 Nov). The NAS is asked over
# SSH, at most every 6 hours (.claude/nas-links, the last answer): links and XSym files (a link
# made from a Mac over SMB is stored as a 1,067-byte file starting "XSym") anywhere under inputs/,
# state/ and sources/inputs/, and a top folder's link to itself (`<name>/<name>`). A link found is
# a "link:" line; a flag, nothing is blocked.
#
# The staged declarations (#1402): producers/'s declarations and notes against the NAS's inputs
# folders (tools/dev/inputsnas.py drift, docs/inputs.md §8): one line when they match.
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
# The PM's list for this check (.claude/next-check.md, a line each, added to between checks): shown
# first, then set aside under .claude/next-check-done/ with its time, so the next check starts with an
# empty list and a check cut short loses nothing. Copies older than 2 days are deleted.
print "attend to first (.claude/next-check.md):"
if [[ -s .claude/next-check.md ]]; then
  sed 's/^/  /' .claude/next-check.md
  mkdir -p .claude/next-check-done
  mv .claude/next-check.md .claude/next-check-done/$(date +%Y%m%d-%H%M%S).md
else
  print "  nothing listed"
fi
find .claude/next-check-done -name '*.md' -mtime +2 -delete 2>/dev/null
df -h / | awk 'NR==2{print "disk: "$4" free of "$2}'
print "NAS: $([[ -d /Volumes/personal/projects/scenic-roads/inputs ]] && echo mounted || echo NOT MOUNTED)"
if [[ ! -f .claude/nas-links || -n $(find .claude/nas-links -mmin +360) ]]; then
  # (On the NAS, POSIX sh: the candidates, then each checked; a find cut short says so. ~70 s on
  # 7 Nov; bounded: 5 s to connect, find's 180 s, a silent link dropped after 45 s.)
  # (The NAS by its LAN name at home, else through Tailscale by its bare name; the host key checked
  # as the LAN name's either way.)
  nas_host=fishandchips.local; dscacheutil -q host -a name $nas_host | grep -q ip_address || nas_host=fishandchips
  if ans=$(ssh -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o HostKeyAlias=fishandchips.local brandontsang@$nas_host '
    cd /volume1/personal/projects/scenic-roads || exit 3
    { for d in */; do d=${d%/}; echo "$d/$d"; done
      timeout 180 find inputs state sources/inputs -mindepth 1 \( -type l -o -type f -size 1067c \) -print || echo "!find $?"
    } | while IFS= read -r p; do
      case $p in "!find "*) echo "$p"; continue ;; esac
      if [ -L "$p" ] || { [ -f "$p" ] && [ "$(head -c 4 "$p" 2>/dev/null)" = XSym ]; }; then echo "link: $p"; fi
    done' 2>/dev/null); then
    print -r -- "checked $(date '+%F %T')${ans:+
}$ans" > .claude/nas-links
  else
    print "NAS links: NOT READ (ssh to the NAS failed, exit $?): asked again at the next check"
  fi
fi
if [[ -f .claude/nas-links ]]; then
  print "NAS links in inputs/, state/, sources/inputs/ and the top folders (none belong; $(head -1 .claude/nas-links)):"
  tail -n +2 .claude/nas-links | sed 's/^!find \(.*\)/find ended with exit \1: the list may be short/; s/^/  /'
  [[ $(wc -l < .claude/nas-links) -gt 1 ]] || print "  none"
fi
# The staged declarations against the NAS (#1402): tools/dev/inputsnas.py drift, a line when all
# match, else a line a file that differs; one SSH pass, ~1 s. Drift is flagged: its dry run shows
# what placing would change (a NAS edit the repository lacks would be undone), then place it.
drift=$(python3 tools/dev/inputsnas.py drift 2>&1); st=$?
(( st == 1 )) && drift="${drift/#inputs on the NAS: DRIFT/inputs on the NAS: DRIFT  <-- REVIEW, THEN PLACE}"
print -r -- $drift
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
# The board (tools/dev/board.py health): the parked tasks (backgrounded or integrating), each with its
# last event, so every check looks again at what it waits on and affirms or corrects its status; and
# the small tasks under way by batch, a batch of fewer than five flagged when a task queued now was
# already small when it was recorded and is neither in it nor named as left out, and a batch no start
# event records (small tasks start five at a time, tools/dev/agent-rules.md, "Starting tasks").
python3 tools/dev/board.py health 2>&1
print "== also: check every running subagent and background task, update the board (status, history, now line), email the owner if anything notable"
print "== fill free slots by priority, classifying each task's size (small true or false) as you pick it; a small task next starts with the four next small ones, one slot, builder, branch and review: tools/dev/board.py batch <five numbers> (fewer only with --left-out '#N (why); …' for each other small one waiting; agent-rules.md, \"Starting tasks\")"
# (The PM's marker, #1398: while it is there, every agent's tests on this Mac run here, not on the test Mac.)
[[ -e .claude/tests-local ]] && print -r -- "tests-local: agents' tests run on this Mac, not the test Mac, since $(stat -f %Sm .claude/tests-local) ($(head -1 .claude/tests-local)): delete .claude/tests-local once the test Mac is back"
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
