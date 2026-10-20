#!/bin/zsh
# Runs a test command on the M4 (`ssh m4`) against an exact copy of a source tree, streaming its
# output back; on the M4 itself, when the M4 can't be reached, or with SCENIC_CHECK_LOCAL=1 (or
# SCENIC_TESTS_LOCAL=1), it runs here instead. Its exit status is the command's. (Task #442: the M1
# builds and serves; the heavy test runs of tools/dev/check.sh and tools/app/publish.sh go to the
# M4, mostly idle.)
#
#   tools/app/remote-tests.sh <key> <source dir> <shell command>
#   tools/app/remote-tests.sh --drop <key>     removes the M4's folder for <key> (unless in use)
#
# The key's lock: one run per key at a time (~/scenic-check/<key>/.busy, naming this Mac, process,
# its start time and a nonce), taken before the copy and held until this script exits; a second run
# of the same key waits for it (up to 30 min, then runs here). A lock whose process on this Mac is
# gone (or is another process under a reused PID), or older than 6 h, is taken over.
# The tree: the source dir's tracked files and its untracked, not-ignored ones (as on disk,
# uncommitted edits and symlinks included) are copied by rsync into the M4's
# ~/scenic-check/<key>/src; any file or symlink there not in that list, and then any empty folder,
# is deleted, so the M4 builds exactly what's here. A tree holding ignored test data (data/build,
# which tests read when present and skip without) runs here, where that data is.
# The build folder, ~/scenic-check/<key>/target, is kept between runs (only what changed rebuilds),
# and rustc goes through the M4's sccache when it has one (third-party crates shared between keys).
# The command runs at nice 10 (the M4's pool jobs first), under `mise env` of the tree, with 8192
# open files, its stdin closed, with this side's RUST_LOG, RUST_BACKTRACE, RUST_TEST_THREADS,
# RUST_MIN_STACK, CARGO_TERM_COLOR, CARGO_INCREMENTAL, CARGO_BUILD_JOBS, CARGO_PROFILE_*, NEXTEST_*
# and SCENIC_* (exported ones). Both sides print their rustc, cargo and nextest versions.
# At most three runs on the M4 at once (slots in ~/scenic-check/.slots, each naming its process and
# its start time; a gone process's slot is freed); the rest wait for one, up to 30 min, then run
# here. A run whose connection drops is stopped on the M4 (a watcher sees its sshd gone).
# The result: the M4's side ends with a line "@@remote-tests-status <exit>" (or "none <why>"), read
# here and not shown. Without one (the connection failed, no tree, no slot), it prints
# "--- remote result discarded, local rerun ---" and runs the command here: what follows the last
# such line is the result.
# Disk: a key's folder unused for 3 days goes; past 60 GB in all, the least recently used go
# (never one locked). tools/dev/sweep-targets.sh drops a removed worktree's key.
host=${SCENIC_TEST_HOST:-m4}
self=${0:A}
setopt extendedglob
[[ -n ${SCENIC_TESTS_TRACE:-} ]] && set -x
marker='--- remote result discarded, local rerun ---'

loadavg() { sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}'; }
versions() {  # the toolchain this side runs
  print -r -- "remote-tests: $1: $(rustc -V 2>&1 | head -1); $(cargo -V 2>&1 | head -1); $(cargo nextest --version 2>/dev/null | head -1 || true); NAS $([[ -d /Volumes/personal/projects/scenic-roads ]] && print mounted || print absent)"
}

# --- The M4's side ---------------------------------------------------------------------------------

stale_after=$(( 6 * 3600 ))
remote_lock() {  # <key> <owner>: 0 taken, 1 busy (prints its owner)
  local d=~/scenic-check/$1 owner=$2
  mkdir -p $d || exit 2
  if mkdir $d/.busy 2>/dev/null; then
    print -r -- "$owner" > $d/.busy/owner; touch $d/stamp; exit 0
  fi
  local held=$(cat $d/.busy/owner 2>/dev/null)
  local age=$(( $(date +%s) - $(stat -f %m $d/.busy 2>/dev/null || date +%s) ))
  # Older than 6 h, or its owner never written (10 s on): taken over (moved aside, checked unchanged).
  if (( age > stale_after )) || [[ -z $held && age -gt 10 ]]; then
    remote_unlock $1 "$held" quiet && remote_lock $1 "$owner"
  fi
  print -r -- "${held:-?}"; exit 1
}
remote_unlock() {  # <key> <owner> [quiet]: removes the lock only while it is still <owner>'s
  local d=~/scenic-check/$1 aside=~/scenic-check/$1/.busy.aside-$$
  mv $d/.busy $aside 2>/dev/null || return 1
  if [[ $(cat $aside/owner 2>/dev/null) == "$2" ]]; then rm -rf $aside; return 0; fi
  # Someone else's (taken over and retaken meanwhile): left aside, not put back (a move onto a
  # lock made since would nest inside it); its run, seeing its lock gone as it ends, gives no
  # result, and reruns here.
  return 1
}

remote_main() {  # on the M4: <key> <owner> <command>
  local key=$1 owner=$2 cmd=$3 base=~/scenic-check slots=3 cap_gb=60
  local dir=$base/$key slot= waited=0 s held pid start mine
  setopt nullglob
  none() { print -r -- "@@remote-tests-status none $*"; exit 0; }
  [[ $(cat $dir/.busy/owner 2>/dev/null) == "$owner" ]] || none "the key's lock isn't this run's"
  cd $dir/src 2>/dev/null || none "no tree in $dir"
  [[ -s $dir/files.list ]] || none "no file list in $dir"
  # The copy matches the list: files and symlinks not in it go, then empty folders.
  comm -23 <(find . \( -type f -o -type l \) | sed 's|^\./||' | LC_ALL=C sort) \
           <(tr '\0' '\n' < $dir/files.list | LC_ALL=C sort) | while IFS= read -r f; do rm -f -- "$f"; done
  find . -mindepth 1 -type d -empty -delete 2>/dev/null
  mkdir -p $base/.slots
  mine="$$ $(ps -o lstart= -p $$) $key"
  while true; do
    for s in {1..$slots}; do
      if mkdir $base/.slots/$s 2>/dev/null; then
        print -r -- "$mine" > $base/.slots/$s/owner; slot=$s; break 2
      fi
      # A slot whose process is gone, or is another process under the same PID (its start time
      # differs), or never wrote its owner (10 s on), is freed: moved aside, then removed only if
      # it still names what was read (another waiter may have freed and retaken it meanwhile).
      held=$(cat $base/.slots/$s/owner 2>/dev/null); pid=${held%% *}
      if [[ -n $held ]]; then
        start=${${held#* }% *}
        kill -0 $pid 2>/dev/null && [[ "$(ps -o lstart= -p $pid)" == "$start" ]] && continue
      elif [[ -z $(find $base/.slots/$s -maxdepth 0 ! -newermt '10 seconds ago' 2>/dev/null) ]]; then continue
      fi
      # (Slots set aside so, another's, go after 10 min: their runs count no longer.)
      find $base/.slots -maxdepth 1 -name '.aside-*' ! -newermt '10 minutes ago' -exec rm -rf {} + 2>/dev/null
      if mv $base/.slots/$s $base/.slots/.aside-$s-$$ 2>/dev/null; then
        touch $base/.slots/.aside-$s-$$  # (its 10 min counted from now: mv keeps the old mtime)
        if [[ $(cat $base/.slots/.aside-$s-$$/owner 2>/dev/null) == "$held" ]]; then rm -rf $base/.slots/.aside-$s-$$
        fi  # (another's, retaken meanwhile: left aside, as with a key's lock)
      fi
    done
    if (( waited == 0 )); then
      print "remote-tests: the M4's $slots test slots are taken ($(awk '{print $NF}' $base/.slots/[0-9]*/owner 2>/dev/null | tr '\n' ' ')): waiting"
    fi
    (( waited >= 1800 )) && none "no M4 test slot within 30 min"
    sleep 5; (( waited += 5 ))
  done
  # On any exit: the watcher stopped (by its PID), the slot freed while still this run's.
  typeset -g g_wd= g_cpid= g_slotdir=$base/.slots/$slot g_mine=$mine
  trap '[[ -n $g_wd ]] && kill $g_wd 2>/dev/null; [[ $(cat $g_slotdir/owner 2>/dev/null) == "$g_mine" ]] && rm -rf $g_slotdir' EXIT
  # A signal stops the whole run, grandchildren too (cargo's rustc and test binaries): the watcher
  # stopped and the slot freed first, then this connection's process group (only this run is in
  # it), as the watcher does.
  typeset -g g_pgid=$(ps -o pgid= -p $$ | tr -d ' ')
  on_signal() {
    trap '' TERM HUP INT
    [[ -n $g_wd ]] && kill $g_wd 2>/dev/null
    [[ $(cat $g_slotdir/owner 2>/dev/null) == "$g_mine" ]] && rm -rf $g_slotdir
    kill -TERM -- -$g_pgid 2>/dev/null
    exit 143
  }
  trap on_signal TERM HUP INT
  (( waited )) && print "remote-tests: got an M4 test slot after ${waited} s"
  # Disk: stale keys, then the least recently used past the cap; never a locked one (a lock older
  # than 6 h aside), whose stamp was touched as it was taken.
  local d f
  locked() { [[ -d $1/.busy && -n $(find $1/.busy -maxdepth 0 -newermt "$stale_after seconds ago" 2>/dev/null) ]]; }
  for d in $base/*(/); do
    locked $d && continue
    f=$d/stamp; [[ -e $f ]] || f=$d
    [[ -n $(find $f -maxdepth 0 ! -newermt '3 days ago' 2>/dev/null) ]] && rm -rf $d
  done
  local total=$(du -sk $base 2>/dev/null | cut -f1)
  if (( total > cap_gb * 1024 * 1024 )); then
    for d in $base/*(/Om); do  # oldest first (a run touches its folder as it starts)
      (( total <= cap_gb * 1024 * 1024 )) && break
      locked $d && continue
      local sz=$(du -sk $d | cut -f1)
      print "remote-tests: over ${cap_gb} GB on the M4: removed ${d:t} ($(( sz / 1024 )) MB)"
      rm -rf $d; (( total -= sz ))
    done
  fi
  touch $dir $dir/stamp
  # (ssh's non-login shell has only the system's PATH: brew's, where mise is, first.)
  path=(/opt/homebrew/bin ~/.cargo/bin $path)
  mise trust -q 2>/dev/null
  eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
  export CARGO_TARGET_DIR=$dir/target
  if [[ -x /opt/homebrew/bin/sccache ]]; then
    export RUSTC_WRAPPER=/opt/homebrew/bin/sccache SCCACHE_DIR=~/Library/Caches/scenic-sccache SCCACHE_CACHE_SIZE=10G
  fi
  ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
  # The watcher: this connection's sshd gone (the M1's side stopped), the run's process group goes.
  local p=$$ sshd=
  while [[ $p -gt 1 ]]; do
    [[ $(ps -o comm= -p $p) == *sshd* ]] && { sshd=$p; break; }
    p=$(ps -o ppid= -p $p | tr -d ' ')
  done
  local pgid=$(ps -o pgid= -p $$ | tr -d ' ') wd=
  # (And it keeps the key's lock and stamp fresh, so a run past 6 h isn't taken for stale or
  # evicted.)
  ( while sleep 3; do
      [[ -z $sshd ]] || kill -0 $sshd 2>/dev/null || { kill -TERM -- -$pgid; exit; }
      [[ -d $dir/.busy ]] && touch -c $dir/.busy $dir/stamp 2>/dev/null
    done ) </dev/null >/dev/null 2>&1 &
  wd=$!; g_wd=$wd
  versions "the M4"
  print "remote-tests: on the M4 (load $(loadavg), slot $slot, $dir)"
  # (In the background, waited for: a TERM's trap runs at once, not when the command ends.)
  nice -n 10 zsh -c "$cmd" </dev/null 2>&1 &
  g_cpid=$!
  wait $g_cpid
  local rc=$?
  [[ -n $wd ]] && kill $wd 2>/dev/null
  print "remote-tests: exit $rc on the M4 (load $(loadavg))"
  # (A run whose lock was taken over meanwhile, its tree perhaps copied over: no result.)
  [[ $(cat $dir/.busy/owner 2>/dev/null) == "$owner" ]] || none "the key's lock was taken over during the run"
  print "@@remote-tests-status $rc"
  exit 0
}

remote_drop() {  # on the M4: <key>
  local base=~/scenic-check key=$1
  [[ -n $key && $key != .* && $key != */* ]] || exit 2
  [[ -d $base/$key/.busy ]] && { print "in use: $key"; exit 0; }
  rm -rf $base/$key
}

# --- This side ------------------------------------------------------------------------------------

run_local() {  # <source dir> <command>
  ( cd $1 && eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
    ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
    zsh -c "$2" </dev/null 2>&1 )
}

on_m4() {  # <args...>: this script's M4 side, over ssh (the shared connection)
  ssh -o ConnectTimeout=10 -o BatchMode=yes $host zsh -s -- ${(q)@} < $self
}

local_main() {  # <key> <source dir> <command>
  local key=$1 src=${2:A} cmd=$3
  [[ $key == [A-Za-z0-9._-]## && $key != .* ]] || { print "remote-tests: bad key $key"; return 2; }
  ( cd $src && versions "this Mac ($(scutil --get LocalHostName 2>/dev/null))" )
  local why= rname
  if [[ -n ${SCENIC_CHECK_LOCAL:-}${SCENIC_TESTS_LOCAL:-} ]]; then why="SCENIC_CHECK_LOCAL set"
  elif [[ -d $src/data/build ]]; then why="the tree has ignored test data in data/build, which the M4's copy wouldn't"
  elif ! rname=$(ssh -o ConnectTimeout=5 -o BatchMode=yes $host scutil --get LocalHostName 2>/dev/null); then why="the M4 isn't reachable"
  elif [[ $rname == "$(scutil --get LocalHostName 2>/dev/null)" ]]; then why="this is the M4"
  fi
  if [[ -n $why ]]; then
    print "remote-tests: running here ($why; load $(loadavg))"
    run_local $src "$cmd"; return
  fi
  # The key's lock, held until this script exits.
  local me=$(scutil --get LocalHostName 2>/dev/null) owner held waited=0 out rc
  owner="$me $$ $(ps -o lstart= -p $$ | tr -s ' ') $RANDOM$RANDOM"
  while true; do
    out=$(on_m4 --remote-lock $key "$owner" 2>&1); rc=$?
    (( rc == 0 )) && break
    if (( rc != 1 )); then
      print "remote-tests: can't take the key's lock on the M4 ($rc: $out): running here"
      run_local $src "$cmd"; return
    fi
    held=$out
    # Held by a process on this Mac that is gone (or a different process under its PID): taken over.
    local hp=${${held#$me }%% *}
    if [[ $held == "$me "* ]] && { ! kill -0 $hp 2>/dev/null || [[ "$me $hp $(ps -o lstart= -p $hp | tr -s ' ') " != "${held% *} " ]]; }; then
      on_m4 --remote-unlock $key "$held" >/dev/null 2>&1; continue
    fi
    (( waited == 0 )) && print "remote-tests: key $key is in use on the M4 ($held): waiting"
    if (( waited >= 1800 )); then
      print "remote-tests: key $key still in use after 30 min: running here"
      run_local $src "$cmd"; return
    fi
    sleep 5; (( waited += 5 ))
  done
  (( waited )) && print "remote-tests: got key $key after $waited s"
  trap "on_m4 --remote-unlock $key ${(q)owner} >/dev/null 2>&1" EXIT
  trap 'exit 143' INT TERM HUP
  local tmp=$(mktemp -d)
  # Tracked and untracked-not-ignored files and symlinks that exist (a deleted tracked file isn't
  # sent, and goes there).
  ( cd $src && git ls-files -co --exclude-standard -z | while IFS= read -r -d '' f; do
      [[ -f $f || -L $f ]] && print -rn -- "$f"$'\0'; done ) > $tmp/files.list
  # (The M4's prune compares lists a line a path: a path with a newline runs here.)
  if [[ $(<$tmp/files.list) == *$'\n'* ]]; then
    rm -rf $tmp
    print "remote-tests: running here (a path in the tree has a newline)"
    run_local $src "$cmd"; return
  fi
  local t0=$SECONDS
  ssh -o BatchMode=yes $host "mkdir -p ~/scenic-check/$key/src" &&
  rsync -a --from0 --files-from=$tmp/files.list $src/ ${host}:scenic-check/$key/src/ &&
  rsync -a $tmp/files.list ${host}:scenic-check/$key/files.list
  rc=$?
  if (( rc )); then
    rm -rf $tmp
    print "remote-tests: copying to the M4 failed ($rc): running here"
    run_local $src "$cmd"; return
  fi
  print "remote-tests: tree copied to the M4 in $(( SECONDS - t0 )) s (M1 load $(loadavg))"
  # This side's settings the tests read, exported there too.
  local v envs=
  for v in RUST_LOG RUST_BACKTRACE RUST_TEST_THREADS RUST_MIN_STACK CARGO_TERM_COLOR CARGO_INCREMENTAL \
           CARGO_BUILD_JOBS ${(M)${(k)parameters}:#(CARGO_PROFILE_|NEXTEST_|SCENIC_)*}; do
    [[ $v == SCENIC_(CHECK_LOCAL|TESTS_LOCAL|TEST_HOST|TESTS_TRACE) ]] && continue
    [[ ${(Pt)v:-} == *export* ]] && envs+="export $v=${(q)${(P)v}}; "
  done
  # (Its own connection, not the shared ControlMaster one: its sshd ends with it, which the
  # watcher there reads.) The status line is read off the stream, not shown.
  print -rn -- '' > $tmp/status
  local full="$envs$cmd"
  # (No MULTIOS here: a coprocess's stdin plus `< $self` would be joined into one never-ending input.)
  setopt localoptions nomultios
  coproc ssh -o BatchMode=yes -o ControlPath=none -o ServerAliveInterval=30 -o ServerAliveCountMax=6 \
      $host zsh -s -- --remote $key ${(q)owner} ${(q)full} < $self 2>&1
  local sp=$!
  trap "kill $sp 2>/dev/null; exit 143" INT TERM HUP
  while IFS= read -r -p l || [[ -n $l ]]; do
    if [[ $l == '@@remote-tests-status '* ]]; then print -r -- ${l#@@remote-tests-status } > $tmp/status
    else print -r -- "$l"; fi
    l=
  done
  wait $sp 2>/dev/null
  trap 'exit 143' INT TERM HUP
  local st=$(<$tmp/status)
  rm -rf $tmp
  if [[ $st == <-> ]]; then return $st; fi
  print "remote-tests: the M4 run ended without a result (${st:-the connection ended}): running here"
  print -r -- "$marker"
  run_local $src "$cmd"
}

case ${1:-} in
  --remote) remote_main $2 $3 $4 ;;
  --remote-lock) remote_lock $2 $3 ;;
  --remote-unlock) remote_unlock $2 $3; exit $? ;;
  --remote-drop) remote_drop $2 ;;
  --drop)
    [[ -n ${2:-} ]] || exit 2
    # (On the M4 itself, which has no ssh alias for itself, the folder is dropped here.)
    if [[ -d ~/scenic-check ]]; then remote_drop $2
    else ssh -o ConnectTimeout=5 -o BatchMode=yes $host zsh -s -- --remote-drop $2 < $self; fi ;;
  *)
    (( $# == 3 )) || { print "usage: remote-tests.sh <key> <source dir> <command> | --drop <key>"; exit 2; }
    local_main "$@"; exit $? ;;
esac
