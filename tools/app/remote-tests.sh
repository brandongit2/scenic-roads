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
# The tree: the source dir's tracked files and its untracked, not-ignored ones (as on disk,
# uncommitted edits included) are copied by rsync into the M4's ~/scenic-check/<key>/src, and any
# file there not in that list is deleted, so the M4 builds exactly what's here. The build folder,
# ~/scenic-check/<key>/target, is kept between runs (only what changed rebuilds), and rustc goes
# through the M4's sccache when it has one (third-party crates shared between keys).
# The command runs at nice 10 (the M4's pool jobs first), under `mise env` of the tree, with 8192
# open files, its stdin closed. At most three run on the M4 at once (slots in ~/scenic-check/.slots,
# each naming its process; a gone process's slot is freed); the rest wait for one, up to 30 min,
# then run here. A run whose connection drops is stopped on the M4 (a watcher sees its sshd gone).
# Disk: a key's folder untouched for 3 days goes; past 60 GB in all, the least recently used go
# (never one in use). tools/dev/sweep-targets.sh drops a removed worktree's key.
host=${SCENIC_TEST_HOST:-m4}
self=${0:A}
setopt extendedglob
[[ -n ${SCENIC_TESTS_TRACE:-} ]] && set -x

remote_main() {  # on the M4: <key> <command>
  local key=$1 cmd=$2 base=~/scenic-check slots=3 cap_gb=60
  local dir=$base/$key slot= waited=0 s pid
  setopt nullglob
  cd $dir/src || exit 97
  [[ -s $dir/files.list ]] || { print "remote-tests: no file list in $dir"; exit 97; }
  # The copy matches the list: what isn't in it goes.
  comm -23 <(find . -type f ! -path './target/*' | sed 's|^\./||' | LC_ALL=C sort) \
           <(tr '\0' '\n' < $dir/files.list | LC_ALL=C sort) | while IFS= read -r f; do rm -f -- "$f"; done
  touch $dir/stamp
  mkdir -p $base/.slots
  while true; do
    for s in {1..$slots}; do
      if mkdir $base/.slots/$s 2>/dev/null; then
        print -r -- "$$ $key" > $base/.slots/$s/owner; slot=$s; break 2
      fi
      pid=$(cut -d' ' -f1 $base/.slots/$s/owner 2>/dev/null)
      # A slot whose process is gone (or never wrote its owner, 10 s on) is freed.
      if [[ -n $pid ]] && ! kill -0 $pid 2>/dev/null; then rm -rf $base/.slots/$s
      elif [[ -z $pid && -n $(find $base/.slots/$s -maxdepth 0 ! -newermt '10 seconds ago' 2>/dev/null) ]]; then rm -rf $base/.slots/$s
      fi
    done
    if (( waited == 0 )); then
      print "remote-tests: the M4's $slots test slots are taken ($(cut -d' ' -f2 $base/.slots/*/owner 2>/dev/null | tr '\n' ' ')): waiting"
    fi
    (( waited >= 1800 )) && { print "remote-tests: no M4 test slot within 30 min"; exit 98; }
    sleep 5; (( waited += 5 ))
  done
  trap "rm -rf $base/.slots/$slot" EXIT
  trap 'exit 143' TERM HUP INT
  (( waited )) && print "remote-tests: got an M4 test slot after ${waited} s"
  # Disk: stale keys, then the least recently used past the cap, never one in a slot.
  local inuse=( $(cut -d' ' -f2 $base/.slots/*/owner 2>/dev/null) ) d
  for d in $base/*(/); do
    (( ${inuse[(Ie)${d:t}]} )) && continue
    # (A key's stamp is touched as its run starts; a key being copied has none yet: its folder's age.)
    local f=$d/stamp; [[ -e $f ]] || f=$d
    [[ -n $(find $f -maxdepth 0 ! -newermt '3 days ago' 2>/dev/null) ]] && rm -rf $d
  done
  local total=$(du -sk $base 2>/dev/null | cut -f1)
  if (( total > cap_gb * 1024 * 1024 )); then
    for d in $base/*(/Om); do  # oldest first (by the folder's mtime; stamp touches don't move it)
      (( total <= cap_gb * 1024 * 1024 )) && break
      (( ${inuse[(Ie)${d:t}]} )) && continue
      local sz=$(du -sk $d | cut -f1)
      print "remote-tests: over ${cap_gb} GB on the M4: removed ${d:t} ($(( sz / 1024 )) MB)"
      rm -rf $d; (( total -= sz ))
    done
  fi
  touch $dir
  eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
  mise trust -q 2>/dev/null
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
  local pgid=$(ps -o pgid= -p $$ | tr -d ' ')
  if [[ -n $sshd ]]; then
    ( while sleep 3; do kill -0 $sshd 2>/dev/null || { kill -TERM -- -$pgid; exit; }; done ) </dev/null >/dev/null 2>&1 &
  fi
  local wd=$!
  print "remote-tests: on the M4 ($(sysctl -n vm.loadavg | tr -d '{}' | awk '{print "load " $1}'), slot $slot, $dir)"
  nice -n 10 zsh -c "$cmd" </dev/null 2>&1
  local rc=$?
  [[ -n $sshd ]] && kill $wd 2>/dev/null
  print "remote-tests: exit $rc on the M4 (load $(sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}'))"
  exit $rc
}

remote_drop() {  # on the M4: <key>
  local base=~/scenic-check key=$1
  [[ -n $key && $key != .* && $key != */* ]] || exit 2
  grep -qx "[0-9]* $key" $base/.slots/*/owner(N) 2>/dev/null && { print "in use: $key"; exit 0; }
  rm -rf $base/$key
}

run_local() {  # <source dir> <command>
  ( cd $1 && eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
    ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
    zsh -c "$2" </dev/null 2>&1 )
}

local_main() {  # <key> <source dir> <command>
  local key=$1 src=${2:A} cmd=$3
  [[ $key == [A-Za-z0-9._-]## && $key != .* ]] || { print "remote-tests: bad key $key"; return 2; }
  local why=
  if [[ -n ${SCENIC_CHECK_LOCAL:-}${SCENIC_TESTS_LOCAL:-} ]]; then why="SCENIC_CHECK_LOCAL set"
  elif [[ ${(L)$(scutil --get LocalHostName 2>/dev/null)}.local == $(ssh -G $host 2>/dev/null | awk '$1=="hostname" {print $2}') ]]; then why="this is the M4"
  elif ! ssh -o ConnectTimeout=5 -o BatchMode=yes $host true 2>/dev/null; then why="the M4 isn't reachable"
  fi
  if [[ -n $why ]]; then
    print "remote-tests: running here ($why; load $(sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}'))"
    run_local $src "$cmd"; return
  fi
  local tmp=$(mktemp -d) rc
  # Tracked and untracked-not-ignored files that exist (a deleted tracked file isn't sent, and goes there).
  ( cd $src && git ls-files -co --exclude-standard -z | while IFS= read -r -d '' f; do
      [[ -f $f || -L $f ]] && print -rn -- "$f"$'\0'; done ) > $tmp/files.list
  local t0=$SECONDS
  ssh -o BatchMode=yes $host "mkdir -p ~/scenic-check/$key/src" &&
  rsync -a --from0 --files-from=$tmp/files.list $src/ ${host}:scenic-check/$key/src/ &&
  rsync -a $tmp/files.list ${host}:scenic-check/$key/files.list
  rc=$?
  rm -rf $tmp
  if (( rc )); then
    print "remote-tests: copying to the M4 failed ($rc): running here"
    run_local $src "$cmd"; return
  fi
  print "remote-tests: tree copied to the M4 in $(( SECONDS - t0 )) s (M1 load $(sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}'))"
  # (Its own connection, not the shared ControlMaster one: its sshd ends with it, which the
  # watcher there reads.)
  ssh -o BatchMode=yes -o ControlPath=none -o ServerAliveInterval=30 -o ServerAliveCountMax=6 $host zsh -s -- --remote $key ${(q)cmd} < $self &
  local sp=$!
  trap "kill $sp 2>/dev/null" EXIT INT TERM HUP
  wait $sp; rc=$?
  trap - EXIT INT TERM HUP
  # 255: the connection failed (not the command); 97/98: no tree or no slot there.
  if (( rc == 255 || rc == 97 || rc == 98 )); then
    print "remote-tests: the M4 run ended without a result ($rc): running here"
    run_local $src "$cmd"; return
  fi
  return $rc
}

case ${1:-} in
  --remote) remote_main $2 $3 ;;
  --remote-drop) remote_drop $2 ;;
  --drop)
    [[ -n ${2:-} ]] || exit 2
    ssh -o ConnectTimeout=5 -o BatchMode=yes $host zsh -s -- --remote-drop $2 < $self ;;
  *)
    (( $# == 3 )) || { print "usage: remote-tests.sh <key> <source dir> <command> | --drop <key>"; exit 2; }
    local_main "$@"; exit $? ;;
esac
