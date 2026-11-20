#!/bin/zsh
# Runs a test command on the test Mac (`ssh $SCENIC_TEST_HOST`, default `m4`: the M4) against an exact
# copy of a source tree, streaming its output back; on the test Mac itself, or with
# SCENIC_CHECK_LOCAL=1 (or SCENIC_TESTS_LOCAL=1) or the PM's .claude/tests-local (tools/app/test-mac.zsh's
# tests_local), it runs here instead. Its exit status is the
# command's. Anything else keeping it from the test Mac (no connection, no lock or slot within
# 30 min, a failed copy, a run that ended without a result, a tree it can't copy) runs nothing:
# a line "remote-tests: NOT RUN: <why> …" and exit 3 (`refuse`; the owner's rule, 31 Oct: tests run
# on the M4, and a run here in its place had passed check.sh silently, task #668's review).
# (Task #442: the heavy test runs of tools/dev/check.sh and tools/app/publish.sh go to one
# Mac, so the others keep building and serving.)
#
#   tools/app/remote-tests.sh <key> <source dir> <shell command>
#   tools/app/remote-tests.sh --drop <key>     removes the test Mac's folders for <key> and <key>.check
#                                              (unless in use)
#
# The key's lock: one run per key at a time (~/scenic-check/<key>/.busy, naming this Mac, process,
# its start time and a nonce), taken before the copy and held until this script exits; a second run
# of the same key waits for it (up to 30 min, then refuses). A lock whose process on this Mac is
# gone (or is another process under a reused PID), or older than 6 h, is taken over.
# A worktree's keys: its folder name, an agent's own runs', and <name>.check, tools/dev/check.sh's
# tests part's (#1524: a key of its own, so a check never waits behind its agent's run, as #1076's
# did 490 s); both are the worktree's for the disk rule and the drop below.
# The tree: the source dir's tracked files and its untracked, not-ignored ones (as on disk,
# uncommitted edits and symlinks included) are copied by rsync into the test Mac's
# ~/scenic-check/<key>/src; any file or symlink there not in that list, and then any empty folder,
# is deleted. Once the run has a slot, that copy goes into the slot's build folder (into_build), so
# the test Mac builds exactly what's here. Each copy is by content (rsync --checksum), times not
# kept (#983): a file written is dated when it arrives, after every build in that folder, so cargo
# (which takes a source older than its last build's output for built) rebuilds it; a file whose
# content is there already is left as it is, its crate not rebuilt. (Kept times let a file edited
# during a run arrive at the next dated before that run's build, and the next run test the old
# binary.)
# A tree holding ignored test data (data/build, which tests read when present and skip without)
# is refused: the test Mac's copy wouldn't have it.
# The build folders are the slots', not the keys' (#969): ~/scenic-check/.build/<slot>/{src,target},
# the same paths whichever tree comes, kept between runs, the target set by that folder's
# .cargo/config.toml. A key's first run is so no new build: rustc's incremental state holds only
# for the folder it was made in (its working directory is part of what it checks), and a folder per
# key compiled every workspace crate anew (77 % of a new key's compile, M4, 7 Nov); only what
# differs from the slot's last tree rebuilds. A key takes the slot it last used if free (a check's
# key with none yet, its worktree's key's), else the one least recently used (slot_order). rustc goes through the test Mac's sccache when it has one
# (third-party crates, for an emptied folder).
# The command runs at nice 20 and utility priority (`taskpolicy -c utility`, as the pool's jobs: the
# agent's loop first, task #700; the lowest nice, so the pool's jobs at nice 0 win the CPU, #887),
# timed (a line with its wall and CPU), under `mise env` of the tree, with 8192
# open files, its stdin closed, with this side's RUST_LOG, RUST_BACKTRACE, RUST_TEST_THREADS,
# RUST_MIN_STACK, CARGO_TERM_COLOR, CARGO_INCREMENTAL, CARGO_BUILD_JOBS, CARGO_PROFILE_*, NEXTEST_*
# and SCENIC_* (exported ones). Both sides print their rustc, cargo and nextest versions.
# As many runs on the test Mac at once as its capacity's slots (capacity, below: from its cores and memory,
# or ~/scenic-check/capacity there), each with its share of the cores the pool's jobs leave as test
# threads (pool_busy); publish's run (key `publish`, publish.sh's alone: SCENIC_PUBLISH) in a slot of
# its own besides, which has the test Mac to itself (#1563, reserve_all): while it runs no other run
# starts, it takes every check slot as it frees, and its tests start once the runs under way have
# ended (wait_alone, called by its command; slots in ~/scenic-check/.slots, each
# naming its process and its start time; a gone process's slot is freed); the rest wait for one, up
# to 30 min (behind publish's run, as long as it lasts), then refuse. A run on the test Mac itself
# (no ssh, no copy: here_on_test_mac) takes a slot too. A run whose connection drops is stopped on the test Mac (a watcher sees its sshd gone). A slot
# is its build folder's lock: freed only once nothing the run started is left, a run killed outright too.
# The result: the test Mac's side ends with a line "@@remote-tests-status <exit>" (or "none <why>"), read
# here and not shown. Without one (the connection failed, no tree, no slot), it refuses.
# Disk (#912, make_room): before a run, other keys' folders not in use go, a gone worktree's first,
# then the least recently used, and last the other slots' build folders not in use, until the test
# Mac has its agent's room target free plus what this run needs (its build folder; never one
# locked). tools/dev/sweep-targets.sh drops a removed worktree's key too.
# Each run is recorded for the dev job dashboard (tools/dev/devjob.py wrap, the script run again
# under it): its waits for the key and a slot, the copy, the run on the test Mac (which it names, and
# whose load and memory the test Mac's own sampler records meanwhile), the compiles, each suite and
# each test, from what it prints.
# Connecting never hangs (#637): a fresh connection asks ~/.ssh/config's agent (1Password) for the
# key, and a locked 1Password waits for its approval for ever, BatchMode or not. So each ssh this
# side makes runs under a watchdog: the first, a fresh-connection probe, within 20 s; the lock and
# drop calls within 60 s; the run itself must print its first line within 120 s (its first,
# "connected", comes at once). Past a bound, the ssh is stopped and the run refused, saying why.
# The test Mac, known two ways (owner, 31 Oct and 10 Nov): who it is, its LocalHostName, the pool's name
# for a Mac (test_mac_name, tools/app/test-mac.zsh: SCENIC_TEST_MAC, the M4 by default), checked here before anything connects, so on it the
# tests run here, in a slot of its capacity as any run (here_on_test_mac), with no ssh to itself (no
# 1Password approval, no 20 s probe) nor copy; and how to
# reach it from another Mac (SCENIC_TEST_HOST, an ssh alias, `m4` by default).
host=${SCENIC_TEST_HOST:-m4}
# The fresh-connection probe's bound in seconds (a test's shorter: SCENIC_TEST_PROBE_S).
probe_s=${SCENIC_TEST_PROBE_S:-20}
self=${0:A}
setopt extendedglob
# Owner ids (proc_id, proc_alive) and bounded are tools/app/lib.zsh's: sourced here, and sent ahead of this
# script to the test Mac's side (`< <(cat $lib $self)`), where they are defined already.
lib=${self:h}/lib.zsh
(( $+functions[proc_id] )) || source $lib
[[ -n ${SCENIC_TESTS_TRACE:-} ]] && set -x

loadavg() { sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}'; }
versions() {  # the toolchain this side runs
  print -r -- "remote-tests: $1: $(rustc -V 2>&1 | head -1); $(cargo -V 2>&1 | head -1); $(cargo nextest --version 2>/dev/null | head -1 || true); NAS $([[ -d /Volumes/personal/projects/scenic-roads ]] && print mounted || print absent)"
}

# --- The test Mac's side ---------------------------------------------------------------------------------

stale_after=$(( 6 * 3600 ))
term_wait=${SCENIC_TESTS_TERM_WAIT:-15}  # (seconds from a run's TERM to its KILL; shorter in this script's test)
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
    remote_unlock $1 "$held" && remote_lock $1 "$owner"
  fi
  print -r -- "${held:-?}"; exit 1
}
remote_unlock() {  # <key> <owner>: removes the lock only while it is still <owner>'s
  local d=~/scenic-check/$1 aside=~/scenic-check/$1/.busy.aside-$$
  mv $d/.busy $aside 2>/dev/null || return 1
  if [[ $(cat $aside/owner 2>/dev/null) == "$2" ]]; then rm -rf $aside; return 0; fi
  # Someone else's (taken over and retaken meanwhile): left aside, not put back (a move onto a
  # lock made since would nest inside it); its run, seeing its lock gone as it ends, gives no
  # result, and is refused.
  return 1
}

# Disk (#912): the test Mac's agent keeps its room target free (room-target.json in its folder, set
# from the menu bar or `scenic room`); the copies here mustn't eat into it, or the pool's jobs there
# wait. Before a run, other keys' copies not in use go: first those whose worktree is gone (their
# requests named an origin, the calling Mac and its repo, whose latest request no longer lists them
# among its worktrees), then the least recently used, then the other slots' build folders, least
# recently used first, until the disk has the room target free plus what this run needs (its build
# folder as it was, or, an empty one, the largest there: a full build). A copy in use (its lock,
# .busy, held: younger than 6 h, as the takeover rule) is never removed; one is removed by taking
# its lock first (mkdir, atomic), then moved aside and deleted, so no run takes it meanwhile. A
# build folder is removed by taking its slot, as a run does.
agent_home=~/"Library/Application Support/scenic/agent"
free_kb() { df -k $1 | awk 'NR==2 {print $4}'; }  # the free space on <path>'s disk (KB)
room_target_kb() {  # the agent's room target (KB; 0 without one)
  local b=$(plutil -extract bytes raw -o - "$agent_home/room-target.json" 2>/dev/null)
  [[ $b == <-> ]] && print $(( b / 1024 )) || print 0
}
drop_copy() {  # <base> <key folder> <why>: 0 removed, 1 in use
  local base=$1 d=$2 sz gone=$base/.gone-${2:t}-$$
  if ! mkdir $d/.busy 2>/dev/null; then
    # Held, or gone meanwhile: kept. A stale one (older than 6 h) taken over as remote_lock does:
    # moved aside, then taken by mkdir (another taking it meanwhile wins).
    [[ -d $d/.busy && -z $(find $d/.busy -maxdepth 0 -newermt "$stale_after seconds ago" 2>/dev/null) ]] || return 1
    mv $d/.busy $d/.busy.aside-$$ 2>/dev/null && mkdir $d/.busy 2>/dev/null || return 1
  fi
  # (Its owner written at once: a lock without one is taken over by remote_lock after 10 s.)
  print -r -- "make_room $$" > $d/.busy/owner
  mv $d $gone 2>/dev/null || { rm -rf $d/.busy; return 1; }
  sz=$(du -sk $gone 2>/dev/null | cut -f1)
  rm -rf $gone
  print "remote-tests: removed ${d:t} on the test Mac ($3; $(( ${sz:-0} / 1024 )) MB)"
}
drop_build() {  # <base> <build folder> <why>: 0 removed, 1 in use
  local base=$1 d=$2 sz gone=$base/.gone-build-${2:t}-$$ taken=$base/.slots/${2:t}
  mkdir -p $base/.slots
  mkdir $taken 2>/dev/null || return 1
  # (Its owner written at once, this process: a slot without one is freed after 10 s.)
  print -r -- "$(proc_id) make_room" > $taken/owner
  mv $d $gone 2>/dev/null || { rm -rf $taken; return 1; }
  sz=$(du -sk $gone 2>/dev/null | cut -f1)
  rm -rf $gone $taken
  print "remote-tests: removed the build folder of slot ${d:t} on the test Mac ($3; $(( ${sz:-0} / 1024 )) MB)"
}
make_room() {  # <base> <this key> <origin> <live keys> <this run's build folder>
  local base=$1 key=$2 origin=$3 build=$5 d f sz need=0 want
  local -a live=(${=4}) keys=() builds=()
  setopt localoptions nullglob
  rm -rf $base/.gone-*  # (removals cut short)
  if [[ -n $origin ]]; then
    print -r -- "$origin" > $base/$key/origin
    for d in $base/*(/); do
      [[ ${d:t} != $key && $(cat $d/origin 2>/dev/null) == "$origin" && ${live[(Ie)${${d:t}%.check}]} == 0 ]] || continue
      drop_copy $base $d "its worktree is gone"
    done
  fi
  if [[ -d $build/target ]]; then need=$(du -sk $build/target | cut -f1)
  else for d in $base/.build/*/target(/); do sz=$(du -sk $d | cut -f1); (( sz > need )) && need=$sz; done
  fi
  want=$(( $(room_target_kb) + need ))
  for d in $base/*(/); do
    [[ ${d:t} == $key ]] && continue
    f=$d/stamp; [[ -e $f ]] || f=$d
    keys+=("$(stat -f %m $f) $d")
  done
  for d in ${${(on)keys}#* }; do  # the least recently used first
    (( $(free_kb $base) >= want )) && return 0
    drop_copy $base $d "room target $(( $(room_target_kb) / 1048576 )) GB + this run's $(( need / 1024 )) MB not free"
  done
  for d in $base/.build/*(/); do
    [[ $d == $build ]] || builds+=("$(stat -f %m $d/stamp 2>/dev/null || print 0) $d")
  done
  for d in ${${(on)builds}#* }; do
    (( $(free_kb $base) >= want )) && return 0
    drop_build $base $d "room target $(( $(room_target_kb) / 1048576 )) GB + this run's $(( need / 1024 )) MB not free"
  done
  (( $(free_kb $base) >= want )) || print "remote-tests: the test Mac has less free than its room target + this run's $(( need / 1024 )) MB, the other copies in use"
  return 0
}

# The test Mac's capacity (#1539): how many checks' runs it takes at once and how many test threads
# each runs, so together they stay within its cores (20 Nov: three runs of 14 threads each, their
# tests' own threads and builds beside, put the M4's load at 108-229 on 14 cores, and tests of
# 14-17 s alone timed out at 3 min). Derived from its cores and memory: a slot per 4 cores and per
# 12 GB, the fewer (the M4, 14 cores and 48 GB: 3), each run's test threads its share of the cores,
# rounded up (5). Publish's run (key `publish`, publish.sh's alone: SCENIC_PUBLISH in its command)
# has its own slot besides (slot 0, its own build folder: it never waits behind a check) and every
# core: it comes first, and while it runs the test Mac takes more than its cores. Set otherwise by
# ~/scenic-check/capacity on the test Mac, lines `slots=<n>` and `threads=<n>` (either, each a whole
# number from 1 to 999; spaces, CRs and `#` comments left out; a key given twice, the last), which
# every run reads as it starts, printing each line it doesn't take, a key given again, and a folder
# or unreadable file in its place (the capacity then derived). Each run says its slot, the slots and its threads, and where
# each was set; the dev jobs page records them. A caller's own NEXTEST_TEST_THREADS (or nextest's
# --test-threads) is taken as it is. Builds aren't shared out: a run's cargo builds with every core,
# at nice 20 as its tests (a check's compile is the larger part of its time on the test Mac, a
# build's load passes, and its tests are what time out).
capacity() {  # <capacity file> <cores> <memory GB> [<cores the pool's jobs keep busy>]: reply=(<slots> <test threads a run> <where each was set> <publish's test threads, set, or none> <publish's wait for the other runs, s>)
  local f=$1 cores=$2 gb=$3 pool=${4:-0} slots threads line k v n=0 from_s from_t pthreads= pwait=$(( 30 * 60 ))
  local -A at  # (the line each key was read from)
  slots=$(( cores / 4 < gb / 12 ? cores / 4 : gb / 12 )); (( slots < 1 )) && slots=1
  from_s="derived: $cores cores / 4, $gb GB / 12"
  if [[ -e $f && ( -d $f || ! -r $f ) ]]; then
    print -r -- "remote-tests: $f is a folder or can't be read: not read, the capacity derived"
  elif [[ -e $f ]]; then
    while IFS= read -r line || [[ -n $line ]]; do
      (( n++ ))
      line=${${line%%\#*}//$'\r'/}; line=${${line##[[:space:]]#}%%[[:space:]]#}
      [[ -z $line ]] && continue
      k=${${line%%=*}%%[[:space:]]#}; v=${${line#*=}##[[:space:]]#}
      if [[ $line != *=* ]]; then print -r -- "remote-tests: $f line $n: $line isn't <key>=<value>: not read"
      elif [[ $k != (slots|threads|publish_threads|publish_wait) ]]; then print -r -- "remote-tests: $f line $n: $k=$v: unknown key (slots=, threads=, publish_threads=, publish_wait=): not read"
      elif [[ $v != <1-> || ${#v} -gt 3 ]]; then print -r -- "remote-tests: $f line $n: $k=$v isn't a whole number from 1 to 999: not read"
      else
        [[ -n ${at[$k]} ]] && print -r -- "remote-tests: $f line $n: $k given again (line ${at[$k]}): this line's $v read"
        at[$k]=$n
        case $k in
          slots) slots=$(( 10#$v )); from_s="set in $f" ;;
          threads) threads=$(( 10#$v )); from_t="set in $f" ;;
          publish_threads) pthreads=$(( 10#$v )) ;;
          publish_wait) pwait=$(( 10#$v * 60 )) ;;
        esac
      fi
    done < $f
  fi
  if [[ -z $threads ]]; then
    local free=$(( cores - pool < 1 ? 1 : cores - pool ))
    threads=$(( (free + slots - 1) / slots ))
    from_t="derived: $cores cores"; (( pool )) && from_t+=" less $pool the pool's jobs keep busy"; from_t+=" / $slots slots"
  fi
  reply=($slots $threads "slots $from_s; threads $from_t" "$pthreads" $pwait)
}

# The pool's jobs on this Mac (#1563): the cores its agent and every process under it (its jobs:
# scenic-build and theirs) keep busy, from ps's %cpu (a decaying average over about the last
# minute), read only. They run at nice 0 and the tests at nice 20, so what they take the tests don't
# get: a run's share is of the cores they leave. Prints "<whole cores, rounded> <cores, a tenth>
# <processes>". (SCENIC_TESTS_PS names a file of `ps -axo pid=,ppid=,pcpu=,args=` lines read in its
# place: this script's tests'.)
pool_busy() {
  { if [[ -n ${SCENIC_TESTS_PS:-} ]]; then cat $SCENIC_TESTS_PS; else ps -axo pid=,ppid=,pcpu=,args=; fi } | awk '
    { kids[$2] = kids[$2] " " $1; cpu[$1] = $3; a = $0; sub(/^ *[0-9]+ +[0-9]+ +[0-9.]+ +/, "", a)
      if (a ~ /(^|\/)scenic agent( |$)/) q[++n] = $1 }
    END { t = 0; c = 0
      for (i = 1; i <= n; i++) { p = q[i]; if (p in seen) continue; seen[p] = 1; t += cpu[p]; c++
        m = split(kids[p], ks, " "); for (j = 1; j <= m; j++) q[++n] = ks[j] }
      printf "%d %.1f %d\n", int(t / 100 + 0.5), t / 100, c }'
}

# Publish's test threads (#1563): its own setting (publish_threads=), else the cores the pool's jobs
# leave, at least half of them (the lead counts at most half a Mac's cores as busy outside the pool,
# agentcore's coord::sched::outside_cores, so a busy pool's jobs keep the rest). REPLY: why.
publish_share() {  # <cores> <pool's busy cores> <publish_threads set, or empty>: REPLY the threads, reply[1] why
  if [[ -n $3 ]]; then REPLY=$3; reply=("publish_threads set in the capacity file"); return; fi
  local half=$(( ($1 + 1) / 2 )) t=$(( $1 - $2 ))
  (( t < half )) && t=$half
  REPLY=$t; reply=("$1 cores less $2 the pool's jobs keep busy, half of them at least")
}

# A run's share of the test Mac, once it has its slot (remote_main's and a run on the test Mac
# itself): its test threads exported (NEXTEST_TEST_THREADS, RUST_TEST_THREADS; a caller's own,
# exported in its command, after), a check's from the capacity less the pool's jobs (pool_busy); for
# publish's run, every check slot taken (reserve_all, in the background) and SCENIC_TEST_MAC_ALONE
# naming its wait (wait_alone), its tests' threads publish_share's as they start (every core until
# then). REPLY: the threads; reply[1]: where they came from, said on the run's line.
share_out() {  # <base> <slot> <slots> <cores> <owner>
  local base=$1 p=($(pool_busy)) t
  capacity $base/capacity $4 $(( $(sysctl -n hw.memsize) / 1073741824 )) ${p[1]} >/dev/null
  local how=$reply[3] pthreads=$reply[4] pwait=$reply[5]
  if [[ $2 == 0 ]]; then
    ( trap - TERM HUP INT EXIT; reserve_all $base $3 "$5" ) </dev/null >/dev/null 2>&1 &
    typeset -g g_reserver=$!
    local w=$base/.slots/0/alone-wait
    { print -r -- '#!/bin/zsh'; functions loadavg pool_busy publish_share wait_alone
      print -r -- "wait_alone ${(q)base} $3 $pwait $4 ${(q)pthreads} ${(q)5}"; } > $w
    chmod +x $w
    export SCENIC_TEST_MAC_ALONE=$w
    t=$4; how+="; publish's own slot, every check slot taken as it frees (none starts meanwhile), its tests' threads set as they start (publish_wait $(( pwait / 60 )) min)"
  else
    t=$reply[2]; how+="; the pool's jobs keep ${p[2]} cores busy"
  fi
  export NEXTEST_TEST_THREADS=$t RUST_TEST_THREADS=$t
  REPLY=$t; reply=("capacity: $how")
}

publish_holds() {  # <base>: 0 while publish's run holds slot 0 (its process alive); REPLY its owner line
  REPLY=$(cat $1/.slots/0/owner 2>/dev/null)
  [[ -n $REPLY ]] && proc_alive "${REPLY% *}"
}

# The test Mac to publish alone (#1563): publish's run, in slot 0, takes every check slot as it
# frees (mkdir, as any run takes one), so no run starts beside its tests, from a branch with this
# script or one before it (every version since #969 takes a slot so); a run of this version doesn't
# even try while slot 0 is held (take_slot). Runs under way go on to their end: a run can't be
# paused safely mid-test (a stopped test's clock goes on, and nextest's bound and the tests'
# watchdogs fire as it resumes; nextest has no hold between tests that its timer doesn't count).
# Holding them all, it writes .slots/0/alone. In the background for the run's length, in its
# process group; its slots go with slot 0 as the run ends (end_run), or, the run killed outright,
# as any gone run's (free_stale_slot: they name its process).
reserve_all() {  # <base> <slots> <owner>
  local s all
  while true; do
    all=1
    for s in {1..$2}; do
      [[ $(cat $1/.slots/$s/owner 2>/dev/null) == "$3" ]] && continue
      all=0
      if mkdir $1/.slots/$s 2>/dev/null; then print -r -- "$3" > $1/.slots/$s/owner
      else free_stale_slot $1 $s; fi
    done
    (( all )) && { print -r -- $(date +%s) > $1/.slots/0/alone; return 0; }
    sleep 2
  done
}

# Publish's tests start once the test Mac is publish's alone: its command runs this, the program
# SCENIC_TEST_MAC_ALONE names (written into slot 0's folder for the run), between its build and its
# tests, so its build goes on beside the runs it waits for. Says what it waits for (each other run,
# its slot, key and since when) at once and each minute, and how it went; past publish_wait (the
# capacity file's, 30 min by default) its tests go on beside what is left, saying so. Prints the test
# threads its tests take (publish_share, the pool's jobs read as the tests start).
wait_alone() {  # <base> <slots> <bound s> <cores> <publish_threads set, or empty> <owner>
  local base=$1 t0=$SECONDS said=-60 s o left
  while [[ ! -e $base/.slots/0/alone ]]; do
    left=
    for s in {1..$2}; do
      o=$(cat $base/.slots/$s/owner 2>/dev/null)
      [[ -n $o && $o != "$6" ]] && left+=" slot $s ${o##* } (since $(date -r $(stat -f %m $base/.slots/$s) +%H:%M:%S));"
    done
    if (( SECONDS - t0 >= $3 )); then
      print -u2 -r -- "remote-tests: publish's tests went on after $(( SECONDS - t0 )) s (publish_wait) beside the test Mac's other runs:${left%;}"; break
    fi
    (( SECONDS - said >= 60 )) && { print -u2 -r -- "remote-tests: publish's tests wait for the test Mac's other runs to end (none starts meanwhile):${left%;}"; said=$SECONDS; }
    sleep 2
  done
  local p=($(pool_busy))
  publish_share $4 ${p[1]} "$5"
  print -u2 -r -- "remote-tests: the test Mac is publish's after $(( SECONDS - t0 )) s (load $(loadavg); the pool's jobs keep ${p[2]} cores busy, ${p[3]} processes): $REPLY test threads (${reply[1]})"
  print $REPLY
}

# A slot (remote_main's, and a run on the test Mac itself): publish's run slot 0, any other the
# key's last, then the least recently used (slot_order); none free, it waits, 30 min at most, but
# while publish's run holds slot 0 (publish_holds) a run waits for it without trying one, that wait
# not counted in the 30 min. Says what it waits for at once. REPLY: the slot; 1 when none came.
take_slot() {  # <base> <key> <key folder> <slots> <owner>
  local base=$1 key=$2 dir=$3 slots=$4 mine=$5 s waited=0 behind=0 said=
  local -a order
  mkdir -p $base/.slots
  while true; do
    if [[ $key == publish ]]; then order=(0)
    elif publish_holds $base; then
      [[ $said == publish ]] || print "remote-tests: the test Mac is publish's run's (slot 0 since $(date -r $(stat -f %m $base/.slots/0) +%H:%M:%S); its tests run alone): waiting for it to end"
      said=publish; sleep 5; (( behind += 5 )); continue
    else order=($(slot_order $base $slots $dir))
    fi
    for s in $order; do
      if mkdir $base/.slots/$s 2>/dev/null; then
        print -r -- "$mine" > $base/.slots/$s/owner
        # (Stalled past the 10 s below and freed under it: not this run's.)
        [[ $(cat $base/.slots/$s/owner 2>/dev/null) == "$mine" ]] || continue
        REPLY=$s
        (( waited + behind )) && print "remote-tests: got a test-Mac slot after $(( waited + behind )) s"
        return 0
      fi
      free_stale_slot $base $s
    done
    if [[ $said != slots ]] && [[ $key == publish ]]; then
      print "remote-tests: the test Mac's publish slot is taken ($(awk '{print $NF}' $base/.slots/0/owner 2>/dev/null)): waiting"
    elif [[ $said != slots ]]; then
      print "remote-tests: the test Mac's $slots test slots are taken ($(awk '{print $NF}' $base/.slots/<1->/owner 2>/dev/null | tr '\n' ' ')): waiting"
    fi
    said=slots
    (( waited >= 1800 )) && return 1
    sleep 5; (( waited += 5 ))
  done
}

free_stale_slot() {  # <base> <slot>: frees the slot if its run is gone (0), else 1
  local base=$1 s=$2 held pid
  # A slot whose process is gone, or is another process under the same PID (its start time
  # differs), or never wrote its owner (10 s on), is freed: moved aside, then removed only if
  # it still names what was read (another waiter may have freed and retaken it meanwhile).
  # Not while a process of that run's group is left (it was killed outright, no trap ran; its
  # watcher stops them): a build folder is one run's at a time.
  held=$(cat $base/.slots/$s/owner 2>/dev/null); pid=${held%% *}
  if [[ -n $held ]]; then
    proc_alive "${held% *}" && return 1
    [[ -n $(pgrep -g $pid 2>/dev/null) ]] && return 1
  elif [[ -z $(find $base/.slots/$s -maxdepth 0 ! -newermt '10 seconds ago' 2>/dev/null) ]]; then return 1
  fi
  # (Slots set aside so, another's, go after 10 min: their runs count no longer.)
  find $base/.slots -maxdepth 1 -name '.aside-*' ! -newermt '10 minutes ago' -exec rm -rf {} + 2>/dev/null
  if mv $base/.slots/$s $base/.slots/.aside-$s-$$ 2>/dev/null; then
    touch $base/.slots/.aside-$s-$$  # (its 10 min counted from now: mv keeps the old mtime)
    if [[ $(cat $base/.slots/.aside-$s-$$/owner 2>/dev/null) == "$held" ]]; then rm -rf $base/.slots/.aside-$s-$$
    fi  # (another's, retaken meanwhile: left aside, as with a key's lock)
  fi
}

free_mine() {  # <base> <owner> <its slot's folder>: the slots <owner> holds freed, the check slots publish's run took first
  local s
  for s in $1/.slots/<1->(/N); do
    [[ $s != $3 && $(cat $s/owner 2>/dev/null) == "$2" ]] && rm -rf $s
  done
  [[ $(cat $3/owner 2>/dev/null) == "$2" ]] && rm -rf $3
  return 0
}

slot_order() {  # <base> <slots> <key folder>: the slots to try, the key's last, then the least recently used
  # (A check's key, <worktree>.check, with no slot yet: its worktree's key's, where its agent last built.)
  local s last=$(cat $3/slot 2>/dev/null || cat ${3%.check}/slot 2>/dev/null)
  local -a rest
  for s in {1..$2}; do
    [[ $s == $last ]] || rest+=("$(stat -f %m $1/.build/$s/stamp 2>/dev/null || print 0) $s")
  done
  (( ${#rest} < $2 )) && print -r -- $last
  print -rl -- ${${(on)rest}#* }
}

into_build() {  # <key's copy> <build folder's copy>: the tree a slot builds (none other's left there)
  # (Tests that write into the tree are undone too: what differs from the copy is written again.)
  mkdir -p $2 && rsync -rlpD --checksum --delete $1/ $2/
}

remote_main() {  # on the test Mac: <key> <owner> <command> <origin> <live keys>
  local key=$1 owner=$2 cmd=$3 origin=$4 live=$5 base=~/scenic-check
  local dir=$base/$key slot= s mine cores=$(sysctl -n hw.ncpu)
  setopt nullglob
  # (At once: the calling side's first-line bound (120 s) is for connecting, not the prune and
  # disk sweep below.)
  print "remote-tests: connected to the test Mac"
  none() { print -r -- "@@remote-tests-status none $*"; exit 0; }
  [[ $(cat $dir/.busy/owner 2>/dev/null) == "$owner" ]] || none "the key's lock isn't this run's"
  # (Slot 0 and every core are publish.sh's run's alone, which exports SCENIC_PUBLISH: sent so.)
  [[ $key != publish || $cmd == *"export SCENIC_PUBLISH="* ]] || none "key publish is publish.sh's (its own slot and every core): its command exports SCENIC_PUBLISH"
  cd $dir/src 2>/dev/null || none "no tree in $dir"
  [[ -s $dir/files.list ]] || none "no file list in $dir"
  # The copy matches the list: files and symlinks not in it go, then empty folders.
  comm -23 <(find . \( -type f -o -type l \) | sed 's|^\./||' | LC_ALL=C sort) \
           <(tr '\0' '\n' < $dir/files.list | LC_ALL=C sort) | while IFS= read -r f; do rm -f -- "$f"; done
  find . -mindepth 1 -type d -empty -delete 2>/dev/null
  mkdir -p $base/.slots $base/.build
  mine="$(proc_id) $key"
  capacity $base/capacity $cores $(( $(sysctl -n hw.memsize) / 1073741824 ))
  local slots=$reply[1]
  take_slot $base $key $dir $slots "$mine" || none "no test-Mac slot within 30 min"
  slot=$REPLY
  # On any exit, by a signal too (a TERM stops the whole run, cargo's rustc and test binaries, a
  # test's server): the watcher stopped (by its PID), then all else in this connection's process
  # group (only this run is in it, its leader this process), and only then the slot freed, while
  # still this run's: the slot is the build folder's lock. TERM, then KILL past term_wait, this
  # process too: the next run frees the slot, no process of this one left. (Nextest runs each test in
  # a group of its own and stops them on a TERM, 10 s for one ignoring it: it must outlast that.)
  # Publish's run frees the check slots it took too (reserve_all).
  typeset -g g_wd= g_cpid= g_slotdir=$base/.slots/$slot g_mine=$mine g_base=$base
  typeset -g g_pgid=$(ps -o pgid= -p $$ | tr -d ' ')
  end_run() {
    trap '' TERM HUP INT
    [[ -n $g_wd ]] && kill $g_wd 2>/dev/null
    [[ -n ${g_reserver:-} ]] && { kill $g_reserver 2>/dev/null; wait $g_reserver 2>/dev/null; }
    local i; local -a left
    if [[ $g_pgid == $$ ]]; then
      for i in {1..$(( term_wait * 10 + 50 ))}; do
        left=(${(f)"$(pgrep -g $g_pgid 2>/dev/null)"}); left=(${left:#$$})
        (( ${#left} )) || break
        (( i == 1 )) && kill -TERM -- -$g_pgid 2>/dev/null
        (( i == term_wait * 10 )) && kill -KILL -- -$g_pgid 2>/dev/null
        sleep 0.1
      done
    fi
    free_mine $g_base "$g_mine" $g_slotdir
  }
  trap end_run EXIT
  on_signal() { end_run; exit 143 }
  trap on_signal TERM HUP INT
  touch $dir $dir/stamp
  # (The folder a run builds in, the same whichever key: its path is in rustc's incremental state.)
  local build=$base/.build/$slot
  # (The capacity lowered: the build folders of the slots above it go, those not in use; a slot
  # whose run is gone freed first, as when one is taken.)
  for s in $base/.build/<1->(/N); do
    (( ${s:t} > slots )) || continue
    [[ -d $base/.slots/${s:t} ]] && free_stale_slot $base ${s:t}
    drop_build $base $s "above the test Mac's $slots slots"
  done
  make_room $base $key "$origin" "$live" $build
  into_build $dir/src $build/src || none "can't copy the tree into $build"
  print -r -- $slot > $dir/slot
  touch $build/stamp
  cd $build/src || none "no tree in $build"
  # (ssh's non-login shell has only the system's PATH: brew's, where mise is, first.)
  path=(/opt/homebrew/bin ~/.cargo/bin $path)
  mise trust -q 2>/dev/null
  eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
  # The target set by a cargo config in the slot's folder (cargo reads each parent's), not by
  # CARGO_TARGET_DIR: sccache hashes every CARGO_* variable a rustc sees, so a value per folder
  # missed the shared cache (#887: 0 % Rust hits on the mini, 82 % on the M1).
  mkdir -p $build/.cargo
  print -r -- "[build]"$'\n'"target-dir = \"$build/target\"" > $build/.cargo/config.toml
  unset CARGO_TARGET_DIR
  if [[ -x /opt/homebrew/bin/sccache ]]; then
    export RUSTC_WRAPPER=/opt/homebrew/bin/sccache SCCACHE_DIR=~/Library/Caches/scenic-sccache SCCACHE_CACHE_SIZE=10G
  fi
  ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
  # The watcher: this connection's sshd gone (the calling Mac's side stopped), the run's process group goes.
  local p=$$ sshd=
  while [[ $p -gt 1 ]]; do
    [[ $(ps -o comm= -p $p) == *sshd* ]] && { sshd=$p; break; }
    p=$(ps -o ppid= -p $p | tr -d ' ')
  done
  local pgid=$(ps -o pgid= -p $$ | tr -d ' ') wd=
  # (And it keeps the key's lock and stamp fresh, so a run past 6 h isn't taken for stale or
  # evicted.)
  # (This process killed outright, no trap run: what it started goes too, TERM, then KILL.)
  local main=$$
  ( while sleep 3; do
      [[ -z $sshd ]] || kill -0 $sshd 2>/dev/null || { kill -TERM -- -$pgid; exit; }
      kill -0 $main 2>/dev/null || { trap '' TERM; kill -TERM -- -$pgid; sleep $term_wait; kill -KILL -- -$pgid; exit; }
      [[ -d $dir/.busy ]] && touch -c $dir/.busy $dir/stamp 2>/dev/null
    done ) </dev/null >/dev/null 2>&1 &
  wd=$!; g_wd=$wd
  versions "the test Mac"
  # (The test Mac sampled meanwhile, by the tree's own recorder: tools/dev/devjob.zsh.)
  [[ -f tools/dev/devjob.zsh ]] && { source tools/dev/devjob.zsh && dj_sampling } >/dev/null 2>&1
  share_out $base $slot $slots $cores "$mine"
  print "remote-tests: on the test Mac $(scutil --get LocalHostName 2>/dev/null) (load $(loadavg), slot $slot of $slots, $REPLY test threads, $build; ${reply[1]})"
  # (In the background, waited for: a TERM's trap runs at once, not when the command ends.)
  # (Timed: the run's wall and CPU on the test Mac, a line at its end.)
  nice -n 20 /usr/sbin/taskpolicy -c utility zsh -c 'TIMEFMT="remote-tests: the run took %E on the test Mac, cpu %U user + %S sys"; time ( eval "$1" )' zsh "$cmd" </dev/null 2>&1 &
  g_cpid=$!
  wait $g_cpid
  local rc=$?
  [[ -n $wd ]] && kill $wd 2>/dev/null
  print "remote-tests: exit $rc on the test Mac (load $(loadavg))"
  # (A run whose lock was taken over meanwhile, its tree perhaps copied over: no result.)
  [[ $(cat $dir/.busy/owner 2>/dev/null) == "$owner" ]] || none "the key's lock was taken over during the run"
  [[ $(cat $g_slotdir/owner 2>/dev/null) == "$g_mine" ]] || none "the slot was taken during the run"
  print "@@remote-tests-status $rc"
  exit 0
}

remote_drop() {  # on the test Mac: <key>: its folder and <key>.check's
  local base=~/scenic-check key=$1 k
  [[ -n $key && $key != .* && $key != */* ]] || exit 2
  # (By drop_copy: its lock taken first, so no run takes the folder meanwhile.)
  for k in $key $key.check; do
    [[ -d $base/$k ]] || continue
    drop_copy $base $base/$k "remote-tests.sh --drop" || print "in use: $k"
  done
  return 0
}

# --- This side ------------------------------------------------------------------------------------

send_tree() {  # <source dir> <file list (NUL-separated)> <destination>: the copy (#983)
  # By content, times not kept (-a less -t, -o and -g): a file sent is dated when it arrives.
  rsync -rlpD --checksum --from0 --files-from=$2 $1/ $3
}

refuse() {  # <why>: the run kept from the test Mac runs nothing (exit 3)
  print -r -- "remote-tests: NOT RUN: $1. Tests run on the test Mac ($host), never on this Mac in its place: fix that and run again. Only the PM runs them here instead (SCENIC_TESTS_LOCAL=1 in its own command, or its .claude/tests-local for every agent: tools/dev/testing.md)."
  return 3
}

run_local() {  # <source dir> <command>
  ( cd $1 && eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
    ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
    zsh -c "$2" </dev/null 2>&1 )
}

# A run on the test Mac itself (#1563): in its own tree, no copy, but counted as a run from another
# Mac is: a slot taken (take_slot: behind publish's run too), its share of the cores
# (share_out), the slot freed as it ends (by a signal too; killed outright, its slot is a gone
# process's, freed by the next run that tries it).
here_on_test_mac() {  # <key> <source dir> <command>
  local base=~/scenic-check cores=$(sysctl -n hw.ncpu) mine="$(proc_id) $1" rc
  mkdir -p $base/$1
  capacity $base/capacity $cores $(( $(sysctl -n hw.memsize) / 1073741824 ))
  local slots=$reply[1]
  take_slot $base $1 $base/$1 $slots "$mine" || { refuse "no test-Mac slot within 30 min"; return }
  typeset -g g_mine=$mine g_base=$base g_slotdir=$base/.slots/$REPLY g_reserver= g_slot=$REPLY
  trap '[[ -n $g_reserver ]] && { kill $g_reserver 2>/dev/null; wait $g_reserver 2>/dev/null; }; free_mine $g_base "$g_mine" $g_slotdir' EXIT
  trap 'exit 143' INT TERM HUP
  local own=${NEXTEST_TEST_THREADS:-}
  share_out $base $g_slot $slots $cores "$mine"
  # (A caller's own threads, exported here already, stand, as in a run sent from another Mac.)
  [[ -n $own ]] && export NEXTEST_TEST_THREADS=$own
  print "remote-tests: running here (this is the test Mac; load $(loadavg), slot $g_slot of $slots, ${NEXTEST_TEST_THREADS} test threads; ${reply[1]})"
  run_local $2 "$3"
}

on_test_mac() {  # <args...>: this script's test Mac side, over ssh (the shared connection), within 60 s
  bounded 60 ssh -o ConnectTimeout=10 -o BatchMode=yes $host zsh -s -- ${(q)@} < <(cat $lib $self)
}

local_main() {  # <key> <source dir> <command>
  local key=$1 src=${2:A} cmd=$3
  source ${self:h}/test-mac.zsh
  [[ $key == [A-Za-z0-9._-]## && $key != .* ]] || { print "remote-tests: bad key $key"; return 2; }
  [[ $key != publish || ${(t)SCENIC_PUBLISH} == *export* ]] || { print "remote-tests: key publish is publish.sh's (its own slot on the test Mac and every core): SCENIC_PUBLISH isn't exported"; return 2; }
  ( cd $src && versions "this Mac ($(scutil --get LocalHostName 2>/dev/null))" )
  if tests_local; then
    print "remote-tests: running here ($REPLY; load $(loadavg))"
    run_local $src "$cmd"; return
  fi
  if is_test_mac; then here_on_test_mac $key $src "$cmd"; return; fi
  [[ -d $src/data/build ]] && { refuse "the tree has ignored test data in data/build, which the test Mac's copy wouldn't have"; return }
  # (A fresh connection, as the run's own: its key from the agent, which may wait for an approval.)
  if ! bounded $probe_s ssh -o ConnectTimeout=10 -o BatchMode=yes -o ControlPath=none $host true 2>/dev/null; then
    refuse "no fresh ssh connection to $host within $probe_s s (down, or the ssh agent, 1Password, locked or waiting for an approval)"; return
  fi
  # (check.sh's hold of this Mac's shared build folder goes, #1123: the test Mac builds these tests.
  # A refused run builds nothing here.)
  if [[ -n ${SCENIC_CARGO_HOLD:-} && $(ps -o command= -p $SCENIC_CARGO_HOLD 2>/dev/null) == *--scenic-hold* ]]; then
    kill -USR1 $SCENIC_CARGO_HOLD
  fi
  unset SCENIC_CARGO_HOLD
  # The key's lock, held until this script exits.
  local me=$(scutil --get LocalHostName 2>/dev/null) owner held waited=0 out rc
  owner="$me $(proc_id) $RANDOM$RANDOM"
  while true; do
    out=$(on_test_mac --remote-lock $key "$owner" 2>&1); rc=$?
    (( rc == 0 )) && break
    if (( rc != 1 )); then
      refuse "can't take the key's lock on the test Mac ($rc: $out)"; return
    fi
    held=$out
    # Held by a process on this Mac that is gone (or a different process under its PID): taken over.
    if [[ $held == "$me "* ]] && ! proc_alive "${${held#$me }% *}"; then
      on_test_mac --remote-unlock $key "$held" >/dev/null 2>&1; continue
    fi
    (( waited == 0 )) && print "remote-tests: key $key is in use on the test Mac ($held): waiting"
    if (( waited >= 1800 )); then
      refuse "key $key still in use on the test Mac after 30 min"; return
    fi
    sleep 5; (( waited += 5 ))
  done
  (( waited )) && print "remote-tests: got key $key after $waited s"
  trap "on_test_mac --remote-unlock $key ${(q)owner} >/dev/null 2>&1" EXIT
  trap 'exit 143' INT TERM HUP
  local tmp=$(mktemp -d)
  # Tracked and untracked-not-ignored files and symlinks that exist (a deleted tracked file isn't
  # sent, and goes there).
  ( cd $src && git ls-files -co --exclude-standard -z | while IFS= read -r -d '' f; do
      [[ -f $f || -L $f ]] && print -rn -- "$f"$'\0'; done ) > $tmp/files.list
  # (The test Mac's prune compares lists a line a path: a path with a newline can't go; nor an empty
  # list, a tree that isn't a git checkout.)
  if [[ $(<$tmp/files.list) == *$'\n'* ]]; then
    rm -rf $tmp
    refuse "a path in the tree has a newline"; return
  fi
  if [[ ! -s $tmp/files.list ]]; then
    rm -rf $tmp
    refuse "no files to send: $src isn't a git checkout (its tracked and untracked files are what's copied)"; return
  fi
  local t0=$SECONDS
  bounded 60 ssh -o ConnectTimeout=10 -o BatchMode=yes $host "mkdir -p ~/scenic-check/$key/src" &&
  send_tree $src $tmp/files.list ${host}:scenic-check/$key/src/ &&
  rsync -a $tmp/files.list ${host}:scenic-check/$key/files.list
  rc=$?
  if (( rc )); then
    rm -rf $tmp
    refuse "copying to the test Mac failed ($rc)"; return
  fi
  print "remote-tests: tree copied to the test Mac in $(( SECONDS - t0 )) s (this Mac's load $(loadavg))"
  # This side's settings the tests read, exported there too.
  local v envs=
  for v in RUST_LOG RUST_BACKTRACE RUST_TEST_THREADS RUST_MIN_STACK CARGO_TERM_COLOR CARGO_INCREMENTAL \
           CARGO_BUILD_JOBS ${(M)${(k)parameters}:#(CARGO_PROFILE_|NEXTEST_|SCENIC_)*}; do
    [[ $v == SCENIC_(CHECK_LOCAL|TESTS_LOCAL|TEST_HOST|TESTS_TRACE|DEVJOB*) ]] && continue
    [[ ${(Pt)v:-} == *export* ]] && envs+="export $v=${(q)${(P)v}}; "
  done
  # (Its own connection, not the shared ControlMaster one: its sshd ends with it, which the
  # watcher there reads.) The status line is read off the stream, not shown.
  print -rn -- '' > $tmp/status
  local full="$envs$cmd"
  # The key's origin, for the test Mac's disk (make_room): a key naming a linked worktree of this
  # Mac's repo (its name, or check.sh's <name>.check) is that worktree's; sent with the names of the repo's worktrees now, so a gone one's
  # copy goes. (Another key, as main-checkout.check or publish, has none.)
  # (The list read whole, or none sent: it must name this key, or nothing is taken for gone.)
  local origin= live= gd common
  local -a wts
  gd=$(git -C $src rev-parse --absolute-git-dir 2>/dev/null)
  common=$(git -C $src rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
  if [[ -n $gd && $gd != "$common" && ${key%.check} == ${src:t} ]] && wts=(${(f)"$(git -C $src worktree list --porcelain)"}); then
    wts=(${(M)wts:#worktree *}); wts=(${wts##*/})
    if (( ${wts[(Ie)${key%.check}]} )); then origin="$me:$common"; live=${(j: :)wts}; fi
  fi
  # (No MULTIOS here: a coprocess's stdin plus the script's would be joined into one never-ending input.)
  setopt localoptions nomultios
  coproc ssh -o BatchMode=yes -o ControlPath=none -o ServerAliveInterval=30 -o ServerAliveCountMax=6 \
      $host zsh -s -- --remote $key ${(q)owner} ${(q)full} ${(q)origin} ${(q)live} < <(cat $lib $self) 2>&1
  local sp=$! first=-t120 t1=$SECONDS
  trap "kill $sp 2>/dev/null; exit 143" INT TERM HUP
  # (Its first line within 120 s, or the connection is stopped: hung connecting.)
  while IFS= read -r $first -p l || [[ -n $l ]]; do
    first=
    if [[ $l == '@@remote-tests-status '* ]]; then print -r -- ${l#@@remote-tests-status } > $tmp/status
    else print -r -- "$l"; fi
    l=
  done
  if [[ -n $first ]] && (( SECONDS - t1 >= 120 )); then
    kill $sp 2>/dev/null; print -r -- "none no word from the test Mac within 120 s (ssh hung connecting)" > $tmp/status
  fi
  wait $sp 2>/dev/null
  trap 'exit 143' INT TERM HUP
  local st=$(<$tmp/status)
  rm -rf $tmp
  if [[ $st == <-> ]]; then return $st; fi
  refuse "the test Mac run ended without a result (${st:-the connection ended})"
}

# (Sourced, by its tests (tools/app/test/remote-tests.zsh): the functions only.)
[[ $ZSH_EVAL_CONTEXT == *:file ]] && return 0
case ${1:-} in
  -h|--help) print "usage: remote-tests.sh <key> <source dir> <command> | --drop <key>"; exit 0 ;;
  --remote) remote_main $2 $3 $4 "${5:-}" "${6:-}" ;;
  --remote-lock) remote_lock $2 $3 ;;
  --remote-unlock) remote_unlock $2 $3; exit $? ;;
  --remote-drop) remote_drop $2 ;;
  --drop)
    [[ -n ${2:-} ]] || exit 2
    # (On the test Mac itself, which has no ssh alias for itself, the folder is dropped here.)
    if [[ -d ~/scenic-check ]]; then remote_drop $2
    else on_test_mac --remote-drop $2; fi ;;
  *)
    (( $# == 3 )) && [[ $1 != -* ]] || { print -u2 "usage: remote-tests.sh <key> <source dir> <command> | --drop <key>"; exit 2; }
    # (Run again as a recorded span, once: SCENIC_DEVJOB_IN says it's that run.)
    if [[ -z ${SCENIC_DEVJOB_IN:-} ]]; then
      source ${self:h:h}/dev/devjob.zsh
      dj_wrap tests "tests $1" --parse remote,cargo,tests --attr "command=$3"
      (( ${#reply} )) && { export SCENIC_DEVJOB_IN=remote-tests; exec $reply zsh $self "$@" }
    fi
    local_main "$@"; exit $? ;;
esac
