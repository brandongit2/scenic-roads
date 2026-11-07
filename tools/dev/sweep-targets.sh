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
# Agent worktrees build into this Mac's shared folder (tools/dev/bin/cargo, #915); a worktree's own
# target/ comes only from a cargo run without tools/dev/bin on the PATH.
# - Room (#954, make_room): while this Mac's free disk is below its room target (the agent's
#   room-target.json, set on the build page or by `scenic room`) plus the sweep's margin
#   (sweep-margin.json beside it, the build page's "Dev sweep margin"; none set:
#   SWEEP_MARGIN_DEFAULT_GB in crates/pipeline/src/agent/settings.rs), the shared build folder's
#   entries go, the least recently used (read or written) first, until it is back above both; then, still below,
#   the compile cache. No room target: the margin alone is kept free.
# - The main checkout's target/debug: entries untouched for 2 days go.
setopt extendedglob
usage="usage: tools/dev/sweep-targets.sh [--dry-run]"
dry=
case $#:${1:-} in
  0:) ;;
  1:--dry-run) dry=1 ;;
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  *) print -ru2 -- "sweep-targets.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
log=$repo/.claude/sweep-targets.log
say() { if [[ -n $dry ]]; then print "would: $*"; else print "$(date '+%F %T') $*" >> $log; fi }
skip() { [[ -n $dry ]] && print "keep ${wt:t}: $*"; }
agent_home=~/"Library/Application Support/scenic/agent"
shared=${SCENIC_SHARED_TARGET:-$HOME/Library/Caches/scenic-target}
cache=~/Library/Caches/scenic-sccache
zmodload zsh/system; zmodload -F zsh/stat b:zstat; zmodload -F zsh/files b:zf_mv b:zf_mkdir

free_kb() { df -k $1 | awk 'NR==2 {print $4}' }   # the free space on <path>'s disk (KB)
gb() { printf '%.1f GB' $(( $1 / 1048576.0 )) }   # gb <KB>
json_bytes() { local b=$(plutil -extract bytes raw -o - $1 2>/dev/null); [[ $b == <-> ]] && print $b }
# The free space wanted (KB, into $want): this Mac's room target plus the sweep's margin; $room
# says which, in words.
wanted() {
  local t m from="set on the build page"
  t=$(json_bytes $agent_home/room-target.json)
  m=$(json_bytes $agent_home/sweep-margin.json)
  if [[ -z $m ]]; then
    from=default
    m=$(sed -n 's/^pub const SWEEP_MARGIN_DEFAULT_GB: u64 = \([0-9][0-9]*\);.*/\1/p' $repo/crates/pipeline/src/agent/settings.rs 2>/dev/null)
    if [[ -n $m ]]; then m=$(( m << 30 ))
    else m=0 from="its default unreadable: 0"; say "the sweep's default margin can't be read from crates/pipeline/src/agent/settings.rs (SWEEP_MARGIN_DEFAULT_GB): 0 used"; fi
  fi
  if [[ -n $t ]]; then room="room target $(gb $(( t >> 10 ))) + margin $(gb $(( m >> 10 ))) ($from)"
  else room="margin $(gb $(( m >> 10 ))) ($from; no room target: no room-target.json in the agent's folder)" t=0; fi
  want=$(( (t + m) >> 10 ))
}
# The shared build folder's entries (each profile's deps, incremental, build, .fingerprint and
# examples children, and its own files but cargo's locks: the programs it links there), the least
# recently used first: "<last use> <KB> <path>" lines.
entries() {
  local -a ps dirs files sz m; local p d s; local -A kb
  for d in $shared/*/.fingerprint(N:h) $shared/*/*/.fingerprint(N:h); do
    ps+=( $d/(deps|incremental|build|.fingerprint|examples)/*(DN) $d/^.cargo-*(.DN) )
  done
  dirs=( ${^ps}(N/) ); files=( ${^ps}(N^/) )
  if (( $#dirs )); then
    print -rN -- $dirs | xargs -0 du -sk 2>/dev/null | while IFS=$'\t' read -r s p; do kb[$p]=$s; done
  fi
  for p in $files; do zstat -A sz +blocks $p 2>/dev/null && kb[$p]=$(( sz[1] / 2 )); done
  for p in $ps; do used $p && print -r -- "$REPLY ${kb[$p]:-0} $p"; done | sort -n
}
# used <entry>: its last use (epoch s, into $REPLY), the newest of its write and, for a file, its
# last read; for a folder, its files' last read or write and its folders' last write. A build reads
# what it reuses without writing it (a crate built weeks ago, linked into every build since: its
# rlib's and fingerprint's reads), so a write alone would make it look stale. A folder's own read is
# no use: du reads them all (this sweep's sizes).
used() {
  local -a s; local c; REPLY=
  zstat -A s +mtime $1 2>/dev/null || return 1; REPLY=$s[1]
  if [[ -d $1 ]]; then
    for c in $1/*(DN.oa[1]); do zstat -A s +atime $c 2>/dev/null && (( s[1] > REPLY )) && REPLY=$s[1]; done
    for c in $1/*(DNom[1]); do zstat -A s +mtime $c 2>/dev/null && (( s[1] > REPLY )) && REPLY=$s[1]; done
  else zstat -A s +atime $1 2>/dev/null && (( s[1] > REPLY )) && REPLY=$s[1]; fi
  return 0
}

# Room (#954): while the disk's free space is below the room target plus the margin, the shared
# build folder's entries go, the least recently used first (used), enough by their sizes to be
# back above it; then, still below, the compile cache. The folder's lock (tools/dev/bin/cargo) is
# only tried, never waited for: held, the log says so, the folder waits for the next run and the
# compile cache (no lock of ours) goes as if the folder were emptied. Under it each entry still
# unused since it was listed is only renamed into .scenic-sweep-trash (a build waits for renames at
# most), deleted once the lock is let go. Up to three rounds (an entry hard-linked elsewhere frees less
# than its size).
make_room() {
  local want room free round lockfd line m p n k s first last holder
  local -a pick t
  [[ -n $dry ]] || rm -rf $shared/.scenic-sweep-trash  # (a run cut short)
  wanted; free=$(free_kb $shared:h)
  if (( free >= want )); then [[ -n $dry ]] && print "room: $(gb $free) free, at least the $room"; return 0; fi
  for round in 1 2 3; do
    [[ -e $shared/.scenic-lock ]] || break
    pick=() k=0
    for line in ${(f)"$(entries)"}; do
      (( k >= want - free )) && break
      pick+=$line; k=$(( k + ${${line#* }%% *} ))
    done
    (( $#pick )) || break
    first=$(date -r ${pick[1]%% *} '+%F %R') last=$(date -r ${pick[-1]%% *} '+%F %R')
    if [[ -n $dry ]]; then
      print "would remove $#pick entries of the shared build folder ($(gb $k), last used $first to $last): $(gb $free) free, below the $room"
      break
    fi
    if ! zsystem flock -t 0 -f lockfd $shared/.scenic-lock 2>/dev/null; then
      holder=$(head -1 $shared/.scenic-holder 2>/dev/null)
      say "shared build folder locked (held by: ${holder:-unknown}): nothing removed, the next run tries again; $(gb $free) free, below the $room"
      break
    fi
    zf_mkdir -p $shared/.scenic-sweep-trash; n=0 k=0
    for line in $pick; do
      m=${line%% *} s=${${line#* }%% *} p=${${line#* }#* }
      used $p && (( REPLY == m )) || continue  # used since: kept
      zf_mv $p $shared/.scenic-sweep-trash/$n 2>/dev/null && n=$(( n + 1 )) k=$(( k + s ))
    done
    exec {lockfd}>&-
    rm -rf $shared/.scenic-sweep-trash
    free=$(free_kb $shared:h)
    say "removed $n entries of the shared build folder ($(gb $k), last used $first to $last): below the $room; now $(gb $free) free"
    (( free >= want )) && return 0
  done
  # The agents' compile cache (#439; sccache, set up by .claude/worktrees/.cargo/config.toml) holds
  # itself under its 10G cap; still below, it is emptied: only a cache, which builds refill (a
  # removed entry is a miss; the server stays up and writes new entries as before).
  t=( $cache/*(DN[1]) ); (( $#t )) || return 0
  s=$(du -sh $cache 2>/dev/null | cut -f1)
  if [[ -n $dry ]]; then print "would empty the compile cache ($s): $(gb $free) free, below the $room"; return 0; fi
  rm -rf $cache/*(DN)
  say "emptied the compile cache ($s): below the $room; now $(gb $(free_kb $shared:h)) free"
}
# (Sourced by its test, tools/dev/test/sweep-targets.zsh: the definitions alone, its options left as they are.)
[[ $ZSH_EVAL_CONTEXT == *:file ]] && return 0
set -uo pipefail

cd $repo || exit 1
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
  # (a grace period: an agent's report is often followed by a review and a resume within minutes)
  if [[ -n $(find $wt -maxdepth 1 -newermt '15 minutes ago' -print -quit 2>/dev/null) ]]; then skip "$why, but touched in the last 15 min"; continue; fi
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
    # Its copy and build folder on the test Mac (tools/app/remote-tests.sh, #442), unless a run uses it;
    # an unreachable test Mac drops it later itself (the next run from this repo: its worktree gone).
    if [[ ! -d $wt && -x $repo/tools/app/remote-tests.sh ]]; then
      $repo/tools/app/remote-tests.sh --drop ${wt:t} >/dev/null 2>&1 && say "dropped ${wt:t}'s folder on the test Mac"
    fi
  fi
done
# The main checkout's target/debug: each change of flags, dependencies or branch leaves old
# artifacts behind (cargo never deletes them); entries untouched for 2 days go, unless cargo runs there.
if ! print -r -- "$busy" | grep -qx "$repo"; then
  d=$repo/target/debug
  n=$(find $d/deps $d/incremental $d/build $d/.fingerprint -mindepth 1 -maxdepth 1 ! -newermt '2 days ago' 2>/dev/null | wc -l | tr -d ' ')
  if (( n > 0 )); then
    say "swept $n stale entries from target/debug"
    [[ -z $dry ]] && find $d/deps $d/incremental $d/build $d/.fingerprint -mindepth 1 -maxdepth 1 ! -newermt '2 days ago' -exec rm -rf {} + 2>/dev/null
  fi
fi
make_room
exit 0
