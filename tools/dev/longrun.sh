#!/bin/zsh
# Supervised long runs (task #274). Each run is its own launchd job, local.scenic.run.<name>:
# restarted on a crash or non-zero exit (with a back-off), not after a clean finish; caffeinated;
# its log ends with `longrun: done` or `longrun: failed: <why>`, written here. The command must be
# resumable: a restart runs it again from the start.
#
#   longrun.sh start <name> <workdir> <log> [--quiet <min>] -- <command…>   (run by `zsh -l -c`)
#   longrun.sh stop <name>        stop it and remove its job (the log and state stay)
#   longrun.sh forget <name>      stop, and drop its state too (the log stays)
#   longrun.sh status <name>      its state, flags and the log's last lines
#   longrun.sh list               every run, one line each
#   longrun.sh check              what tools/dev/health.sh prints: every run and its flags
#
# Flags: "gone without a final line" (no process, the log doesn't end with a longrun line),
# "log quiet over N min" (N per run, default 30), "restarting repeatedly" (3+ starts in 30 min).
# After 6 starts in 30 min a run gives up: `longrun: failed: restarting repeatedly, gave up`.
# State: ~/Library/Logs/scenic/longrun/<name>/; jobs: ~/Library/LaunchAgents/local.scenic.run.<name>.plist.
setopt no_unset pipe_fail
me=${0:A}
state_root=$HOME/Library/Logs/scenic/longrun
agents=$HOME/Library/LaunchAgents
uid=$(id -u)
label() { print local.scenic.run.$1 }
die() { print -u2 "longrun: $*"; exit 2 }
valid() { [[ $1 =~ '^[A-Za-z0-9_.-]+$' ]] || die "bad name: $1" }

# The job's pid, or nothing.
job_pid() {
  launchctl print gui/$uid/$(label $1) 2>/dev/null | awk '$1=="pid"{print $3; exit}'
}
loaded() { launchctl print gui/$uid/$(label $1) >/dev/null 2>&1 }

# The last `longrun:` line of a log, and whether it is the log's last line.
last_mark() { grep -a '^longrun: ' $1 2>/dev/null | tail -1 }
ends_final() {
  local l; l=$(tail -1 $1 2>/dev/null)
  [[ $l == 'longrun: done'* || $l == 'longrun: failed: '* ]]
}

cmd_start() {
  local name=$1 wd=$2 log=$3; shift 3
  valid $name
  local quiet=30
  [[ ${1-} == --quiet ]] && { quiet=$2; shift 2 }
  [[ ${1-} == -- ]] && shift
  (( $# )) || die "no command"
  [[ -d $wd ]] || die "no such folder: $wd"
  wd=${wd:A}; [[ $log == /* ]] || log=$wd/$log
  loaded $name && die "$name is already loaded; stop it first"
  local d=$state_root/$name; mkdir -p $d
  local cmd="$*"
  { typeset -p wd log quiet cmd } > $d/env
  : > $d/starts; rm -f $d/end
  local plist=$agents/$(label $name).plist
  cat > $plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Label</key><string>$(label $name)</string>
  <key>ProgramArguments</key><array>
    <string>/bin/zsh</string><string>$me</string><string>_run</string><string>$name</string>
  </array>
  <key>WorkingDirectory</key><string>$wd</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$d/launchd.log</string>
  <key>StandardErrorPath</key><string>$d/launchd.log</string>
</dict></plist>
EOF
  launchctl bootstrap gui/$uid $plist || die "launchctl bootstrap failed"
  print "started $(label $name): $cmd  (in $wd, log $log, quiet flag after $quiet min)"
}

# What launchd runs: one attempt. Exit 0 ends the job (done, or given up); non-zero restarts it.
cmd_run() {
  local name=$1 d=$state_root/$1
  local wd log quiet cmd; source $d/env
  rm -f $d/child
  # A finished or given-up run stays finished (e.g. when launchd loads its job again at login).
  [[ -f $d/end && $(cat $d/end) == (done|gave-up)* ]] && exit 0
  local now=$(date +%s)
  print $now >> $d/starts
  local recent=$(awk -v t=$((now-1800)) '$1>t' $d/starts | wc -l | tr -d ' ')
  if (( recent > 6 )); then
    print "longrun: failed: restarting repeatedly, gave up ($recent starts in 30 min)" >> $log
    print "gave-up $now" > $d/end
    exit 0
  fi
  # Back-off beyond launchd's 30 s throttle: 0, 30, 60, 120, 240, 480 s by recent starts.
  if (( recent > 1 )); then
    local wait=$(( 15 * (1 << (recent - 1)) )); (( wait > 480 )) && wait=480
    print "longrun: restart $recent in 30 min, backing off ${wait}s" >> $log
    sleep $wait
  fi
  rm -f $d/end
  print "longrun: start $(date '+%F %T') pid $$: $cmd" >> $log
  caffeinate -is -w $$ &
  local child
  trap 'print "longrun: failed: stopped (signal) $(date "+%F %T")" >> $log; print "stopped $(date +%s)" > $d/end; [[ -n ${child-} ]] && kill -TERM $child 2>/dev/null; exit 0' TERM INT HUP
  cd $wd || { print "longrun: failed: no folder $wd" >> $log; exit 1 }
  zsh -l -c "$cmd" >> $log 2>&1 &
  child=$!
  print $child > $d/child
  wait $child; local rc=$?
  rm -f $d/child
  if (( rc == 0 )); then
    print "longrun: done $(date '+%F %T')" >> $log
    print "done $(date +%s)" > $d/end
    exit 0
  fi
  local why="exit $rc"; (( rc > 128 )) && why="killed by signal $((rc - 128))"
  print "longrun: failed: $why $(date '+%F %T'), restarting" >> $log
  print "failed $(date +%s)" > $d/end
  exit 1
}

cmd_stop() {
  valid $1
  local plist=$agents/$(label $1).plist
  if loaded $1; then launchctl bootout gui/$uid/$(label $1) && print "stopped $(label $1)"
  else print "$(label $1) not loaded"; fi
  rm -f $plist
}

# One line: name, state, flags. Sets $flags for check.
describe() {
  local name=$1 d=$state_root/$1
  local wd log quiet cmd; source $d/env
  local pid=$(job_pid $name) now=$(date +%s)
  local starts=$(awk -v t=$((now-1800)) '$1>t' $d/starts 2>/dev/null | wc -l | tr -d ' ')
  local total=$(wc -l < $d/starts 2>/dev/null | tr -d ' ')
  local mt=$(stat -f %m $log 2>/dev/null || echo 0) st
  local age=$(( (now - mt) / 60 ))
  local mark=$(last_mark $log)
  flags=()
  if [[ -n $pid ]]; then
    if [[ ! -f $d/child && $mark == *backing\ off* ]]; then st="backing off before a restart (pid $pid)"
    else st="running (pid $pid, child $(cat $d/child 2>/dev/null || print -n -))"; fi
    (( age >= quiet )) && flags+="log quiet over $quiet min (last write $age min ago)"
  elif ends_final $log; then
    case $mark in
      'longrun: done'*) st=done ;;
      *gave\ up*) st="failed, gave up"; flags+="gave up: $mark" ;;
      *stopped*) st=stopped ;;
      *) if loaded $name; then st="failed, waiting to restart"; else st="failed, not loaded"; flags+="ended failed: $mark"; fi ;;
    esac
  elif loaded $name && [[ $mark == *backing\ off* ]]; then
    st="backing off"
  else
    st="gone"; flags+="gone without a final line"
  fi
  (( starts >= 3 )) && [[ $st != done ]] && flags+="restarting repeatedly ($starts starts in 30 min)"
  print -n "$name: $st, $total start(s), log $age min old ($log)"
  (( ${#flags} )) && print -n "  <-- ${(j:; :)flags}"
  print
}

names() { for d in $state_root/*(N/); do [[ -f $d/env ]] && print ${d:t}; done }

case ${1-} in
  start) shift; (( $# >= 4 )) || die "usage: start <name> <workdir> <log> [--quiet min] -- <command…>"; cmd_start "$@" ;;
  _run) cmd_run $2 ;;
  stop) cmd_stop $2 ;;
  forget) cmd_stop $2; rm -rf $state_root/$2 ;;
  status) valid $2; [[ -f $state_root/$2/env ]] || die "no run $2"; describe $2
          local wd log quiet cmd; source $state_root/$2/env; print "command: $cmd"; tail -5 $log ;;
  list|check) typeset -a flags; n=0
          for x in $(names); do n=1; [[ $1 == check ]] && print -n "  "; describe $x; done
          (( n )) || print "  none" ;;
  *) sed -n '2,17p' $me | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
