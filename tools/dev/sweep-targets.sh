#!/bin/zsh
# Cleans up finished agent worktrees so they don't fill the disk. Run by launchd every 20 minutes
# (tools/dev/local.scenic.sweep.plist, installed by install-sweep.sh) and by the health check; safe to run any time.
# `sweep-targets.sh --dry-run` prints what it would do and changes nothing.
#
# - A worktree whose commits' patches are all on main (`git cherry` shows no `+`; merges here are
#   cherry-picks) loses its target/. The whole worktree goes (`git worktree remove`, branch kept)
#   only when it is on a branch, has nothing uncommitted and no ignored file besides target/ and
#   data/ (`git status --porcelain --ignored`), and Claude Code's lock on it names a pid that is gone.
# - A worktree whose "Task #N" commits are all done on the board ($BOARD, tools/dev/board.sh), with no `+` in
#   `git cherry`, loses only its target/.
# - Any worktree's target/ untouched for 2 days goes.
# - data/ is never deleted on its own; it goes only with its whole worktree.
# Never touched: a worktree a cargo or rustc process runs in; one with any file modified in the last
# hour; one listed in .claude/sweep-keep (one agent id per line).
# Build folders of new worktrees are seeded from main's by tools/dev/seed-target.sh.
# The agents' compile cache (sccache) is emptied when free disk is below 20 GB (end of this file).
setopt extendedglob
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
log=$repo/.claude/sweep-targets.log
dry=; [[ $1 == --dry-run ]] && dry=1
cd $repo || exit 1
say() { if [[ -n $dry ]]; then print "would: $*"; else print "$(date '+%F %T') $*" >> $log; fi }
skip() { [[ -n $dry ]] && print "keep ${wt:t}: $*"; }
busy=$(for p in $(pgrep -x cargo; pgrep -x rustc); do lsof -a -d cwd -p $p -Fn 2>/dev/null | sed -n 's/^n//p'; done | sort -u)
keep=$(cat $repo/.claude/sweep-keep 2>/dev/null)
locks=$(git worktree list --porcelain)
source ${0:A:h}/board.sh
done_tasks=$(curl -s --max-time 5 $BOARD/api/c/items 2>/dev/null | python3 -c '
import json, sys
for i in json.load(sys.stdin).get("docs", []):
    d = i.get("data") or {}
    if d.get("stage") == "done" and d.get("num") is not None: print(d["num"])' 2>/dev/null)
for wt in $repo/.claude/worktrees/*(N/); do
  [[ -d $wt/target || ${wt:t} == agent-* ]] || continue
  if print -r -- "$busy" | grep -q "^$wt"; then skip "cargo runs in it"; continue; fi
  if print -r -- "$keep" | grep -qxF -e ${${wt:t}#agent-} -e ${wt:t}; then skip "in sweep-keep"; continue; fi
  head=$(git -C $wt rev-parse HEAD 2>/dev/null) || { skip "not a git worktree"; continue; }
  b=$(git -C $wt symbolic-ref -q --short HEAD)
  ahead=$(git cherry main $head 2>/dev/null)
  onmain=; [[ -n $ahead ]] && ! print -r -- "$ahead" | grep -q '^+' && onmain=1
  tdone=
  # The board counts only when no commit is off main (`+`); with all on main, onmain already holds.
  if [[ -z $onmain && -n $done_tasks ]] && ! print -r -- "$ahead" | grep -q '^+'; then
    nums=(${(u)${(f)"$(git log --format=%s main..$head 2>/dev/null | grep -oE 'Task #[0-9]+' | grep -oE '[0-9]+')"}})
    if (( $#nums )); then tdone=1; for n in $nums; do print -r -- "$done_tasks" | grep -qx $n || tdone=; done; fi
  fi
  t=$wt/target
  if [[ -z $onmain && -z $tdone ]]; then
    if [[ -d $t && -z $(find $t -maxdepth 1 -newermt '2 days ago' 2>/dev/null | head -1) ]]; then
      size=$(du -sh $t 2>/dev/null | cut -f1)
      say "swept ${wt:t}/target ($size): untouched 2 days"; [[ -z $dry ]] && rm -rf $t
    else skip "not finished"; fi
    continue
  fi
  why="on main (${b:-detached})"; [[ -n $tdone ]] && why="tasks done on the board (${b:-detached})"
  if [[ -n $(find $wt -newermt '1 hour ago' -print -quit 2>/dev/null) ]]; then skip "$why, but modified in the last hour"; continue; fi
  if [[ -d $t ]]; then
    size=$(du -sh $t 2>/dev/null | cut -f1)
    say "swept ${wt:t}/target ($size): $why"; [[ -z $dry ]] && rm -rf $t
  fi
  # Removing the whole worktree: all commits on main, on a branch, nothing besides target/ and
  # data/ uncommitted or ignored, and no live process holding Claude Code's lock.
  [[ -n $onmain && ${wt:t} == agent-* ]] || continue
  [[ -n $b ]] || { skip "no branch"; continue; }
  extra=$(git -C $wt status --porcelain --ignored 2>/dev/null | grep -vxE '!! (target|data)/')
  [[ -z $extra ]] || { skip "$why; has $(print -r -- $extra | head -3 | tr '\n' ' ')"; continue; }
  lock=$(print -r -- "$locks" | awk -v w=$wt '/^worktree /{c=($2==w)} c && /^locked/{print; exit}')
  pid=$(print -r -- "$lock" | grep -oE 'pid [0-9]+' | grep -oE '[0-9]+')
  if [[ -n $lock && -z $pid ]]; then skip "locked: $lock"; continue; fi
  if [[ -n $pid ]] && kill -0 $pid 2>/dev/null; then skip "$why; lock holder pid $pid alive"; continue; fi
  size=$(du -sh $wt 2>/dev/null | cut -f1)
  say "removed worktree ${wt:t} ($size, branch $b kept): $why"
  if [[ -z $dry ]]; then
    [[ -n $lock ]] && git worktree unlock $wt 2>/dev/null
    git worktree remove $wt 2>/dev/null || say "could not remove ${wt:t}"
    # Its copy and build folder on the M4 (tools/app/remote-tests.sh, #442), unless a run uses it;
    # an unreachable M4 drops it later itself (3 days unused, or past its disk cap).
    if [[ ! -d $wt && -x $repo/tools/app/remote-tests.sh ]]; then
      $repo/tools/app/remote-tests.sh --drop ${wt:t} >/dev/null 2>&1 && say "dropped ${wt:t}'s folder on the M4"
    fi
  fi
done
# The main checkout's own debug builds: each change of flags or dependencies leaves the old
# artifacts behind (cargo never deletes them). Those untouched for 2 days go, unless cargo runs here.
if ! print -r -- "$busy" | grep -qx "$repo"; then
  d=$repo/target/debug
  n=$(find $d/deps $d/incremental $d/build -mindepth 1 -maxdepth 1 ! -newermt '2 days ago' 2>/dev/null | wc -l | tr -d ' ')
  if (( n > 0 )); then
    say "swept $n stale entries from target/debug"
    [[ -z $dry ]] && find $d/deps $d/incremental $d/build -mindepth 1 -maxdepth 1 ! -newermt '2 days ago' -exec rm -rf {} + 2>/dev/null
  fi
fi
# The agents' shared compile cache (#439; sccache, set up by .claude/worktrees/.cargo/config.toml):
# sccache holds it under its 10G cap by itself. When the disk's free space falls below the M1's
# room target (20 GB), it is emptied: it is only a cache, and builds refill it (a removed entry is a
# miss; the server stays up and writes new entries as before).
cache=~/Library/Caches/scenic-sccache
if [[ -d $cache ]]; then
  free=$(df -g / | awk 'NR==2 {print $4}')
  if (( free < 20 )); then
    say "emptied the compile cache ($(du -sh $cache 2>/dev/null | cut -f1)): ${free} GB free"
    [[ -z $dry ]] && rm -rf $cache/*(N)
  elif [[ -n $dry ]]; then print "keep compile cache ($(du -sh $cache 2>/dev/null | cut -f1)): ${free} GB free"; fi
fi
exit 0
