#!/bin/zsh
# The PM's health check of the build and the agents. Prints what needs attention and stamps the
# time (.claude/health-stamp), which tools/dev/health-due.sh reads: Claude Code's hooks make the PM
# run this at least every 20 minutes.
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
cd $repo || exit 1
date +%s > .claude/health-stamp
print "== health $(date '+%F %T')"
df -h / | awk 'NR==2{print "disk: "$4" free of "$2}'
print "NAS: $([[ -d /Volumes/personal/projects/scenic-roads/catalog ]] && echo mounted || echo NOT MOUNTED)"
curl -s -m 10 localhost:7311/api/build | python3 -c '
import json,sys,time
try: d=json.load(sys.stdin); s=d["status"]
except Exception as e: print("BUILD STATUS UNREADABLE:",e); sys.exit()
now=time.time()
print("status from: %s (the lead view when this Mac does not lead), app %s" % (s.get("host"), s.get("app")))
c=s.get("catalog") or {}
age=(now-c.get("at",now))/3600
print("catalog: %s made %.1f h ago" % (c.get("n"), age)+("  <-- STALE (>6 h)" if age>6 else ""))
js=s.get("jobs") or [x for x in [s.get("job"),s.get("beside")]+list(s.get("more") or []) if x]
for j in js:
  p=j.get("progress") or {}; mv=(now-p.get("moved_at",now))/60
  print("the lead job (slot %s): %s %s/%s %s, last progress %.0f min ago" % (j.get("slot"), j.get("id"), p.get("done"), p.get("total"), p.get("unit",""), mv)+("  <-- NO PROGRESS 30 min" if mv>30 else ""))
if not js: print("the lead job: none running")
r=s.get("recent") or []
if r:
  last=(now-max(x.get("ended",0) for x in r))/60
  print("last job finished %.0f min ago" % last+("  <-- NOTHING FINISHED 2 h" if last>120 else ""))
  bad=[x["id"] for x in r[:20] if not x.get("ok")]
  if bad: print("FAILED recently:",bad)
for w in s.get("waiting") or []: print("WAIT:",w.get("what"),"|",(w.get("why") or "")[:200])
p=d.get("pool") or s.get("pool") or {}
for m in (p.get("lead") or {}).get("members",[]):
  beat=(now-m.get("beat",now))/60
  print("member %s: %s %s, beat %.0f min ago" % (m.get("host"), m.get("app"), m.get("state"), beat)+("  <-- SILENT" if beat>10 else ""))
'
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
ps -axo pid,lstart,command | grep "[a]pp/current/scenic-build\|[s]cenic/app/.*/scenic-build" | awk '{printf "  pid %s since %s %s %s  ", $1, $3, $4, $5; for(i=7;i<=9;i++) printf "%s ", $i; print ""}' | cut -c1-160
