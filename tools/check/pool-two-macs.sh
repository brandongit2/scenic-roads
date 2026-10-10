#!/bin/zsh
# The pool's two-Mac test (docs/pool.md §13): the real agents, with the pool on, over a scratch
# folder of the NAS (never the project folder), each Mac's in a folder of its own (never the
# app's), its coordinator on a port of its own (never 8090, the real agent's), its jobs played by a
# script that hands off one save of its step's. Run on each Mac, from a build of the app:
#
#   pool-two-macs.sh root <folder> [slots]          the scratch folder, as the pool begins: today's
#                                                   records, the build Mac (this Mac) their writer,
#                                                   an older helper's hand-off to drain, switched on
#                                                   (`slots`: state/pool/slots on too, a part
#                                                   changing in its process)
#   pool-two-macs.sh region <folder>                a pass (its marker alone) and a region (a 20 km
#                                                   circle) in the scratch folder: the lead plans
#                                                   the pass's worldwide jobs for itself, and offers
#                                                   the region's terrain and tree cover to members
#   pool-two-macs.sh app <dir> <scenic> [helper]    <dir>/v/app/{20261008-0000-aaaaaaa,
#                                                   20261009-0000-bbbbbbb}: the agent (<scenic>,
#                                                   as scenic-real) and the fake jobs, `current` the
#                                                   newer; <dir>/agent its folder (a helper's: with
#                                                   an outbox job from before the pool to drain)
#   pool-two-macs.sh run <dir> <folder>             the launcher: the agent of <dir>'s current app
#                                                   until <dir>/v/app/stop, again whenever it exits
#   pool-two-macs.sh older|newer <dir>              its current app the older or the newer: its
#                                                   agent restarts into it between jobs
#   pool-two-macs.sh newest <dir>                   a third app installed, 20261010-0000-ccccccc (the
#                                                   newer's copy), its current: an update to take
#   pool-two-macs.sh jobs <dir> <seconds>           how long each fake job runs from now on (5 s
#                                                   until set): long enough to span a handover
#   pool-two-macs.sh stop <dir>                     the launcher and the agent stopped (the agent
#                                                   by its pid, <dir>/agent.pid, which the launcher
#                                                   writes, and logs, at each start)
#
# On the M1, run its launcher inside a persistent ssh session, kept open (from the build Mac:
# `ssh brandontsang@macbookpro '<this script> run …'` left running), not under `nohup` nor a
# `screen -dm` started by an ssh command that then ends (both seen 2026-10-10): macOS's
# local-network privacy then cuts the agent off the LAN (the NAS's LAN name doesn't answer), so it
# reports itself away from home, can't lead (`able` false: a handover to it is offered and given up
# after a minute), and never takes over by itself.
#
# What a run shows (2026-10-08, the M4 the build Mac, the M1 a member): the build Mac makes term 1
# and leads; both drain what they held from before the pool into the journal; the lead's jobs hand
# off under its term's leases, its sweep after a re-assertion; the member's entry reaches the lead's
# records through the journal and its mail, acknowledged; the lead restarted re-asserts; the lead
# moved to the older app stands down, the member takes over after two minutes, restarts into the
# lead, its coordinator up; the old lead, newer again, works as a member of it.
set -euo pipefail
cmd=${1:?root, region, app, run, older, newer, newest, jobs or stop}
case $cmd in
  root)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    [[ -e $r ]] && { print -u2 "$r exists"; exit 1; }
    mkdir -p $r/catalog $r/state/build/handoff/old-helper $r/inputs/regions $r/state/pool
    print -n '{"base/6-1-1": "base/6-1-1.1111111111111111.base"}' > $r/state/build/manifest.json
    print -n '{"unit": {"6/1/1": "k1"}}' > $r/state/build/jobs.json
    print -n '{}' > $r/state/build/pending.json
    scutil --get LocalHostName | tr -d '\n' > $r/state/build/writer
    print -n '{"done": ["unit", [["6/9/1", "k91"]]]}' > $r/state/build/handoff/old-helper/00000000000000000001-1.json
    : > $r/state/pool/enabled
    [[ ${3:-} == slots ]] && : > $r/state/pool/slots
    ;;
  region)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    mkdir -p $r/sources/osm/2026-09-28 $r/inputs/regions
    print -n '{}' > $r/sources/osm/2026-09-28/pass.0000000000000000.json
    print -n 'id = "b4test"\nname = "B4 test"\noutline = ["place:7.0,46.0,20"]\n' > $r/inputs/regions/b4test.toml
    ;;
  app)
    d=${2:?its folder}; d=${d:a} scenic=${3:?the scenic program}
    mkdir -p $d/agent
    for v in 20261008-0000-aaaaaaa 20261009-0000-bbbbbbb; do
      mkdir -p $d/v/app/$v
      cp $scenic $d/v/app/$v/scenic-real
      cat > $d/v/app/$v/scenic <<'EOF'
#!/bin/sh
# The agent is the real program; the jobs it runs (scenic backup, scenic gc, scenic-build <step>)
# note their environment and hand off one save of their step's.
d=$(cd "$(dirname "$0")" && pwd)
case "$1" in
  agent) exec "$d/scenic-real" "$@" ;;
esac
step="$1"
env | sort > "$d/env-$step-$$"
sleep "$(cat "$d/../job-seconds" 2>/dev/null || echo 5)"
if [ -n "$SCENIC_HANDOFF" ]; then
  mkdir -p "$SCENIC_HANDOFF"
  printf '{"changes":{"work/test-%s":"work/test-%s.0123456789abcdef.json"}}' "$step" "$step" > "$SCENIC_HANDOFF/00000000000000000001-$$.json"
fi
exit 0
EOF
      cp $d/v/app/$v/scenic $d/v/app/$v/scenic-build
      chmod +x $d/v/app/$v/scenic $d/v/app/$v/scenic-build
    done
    ln -sfn 20261009-0000-bbbbbbb $d/v/app/current
    [[ ${4:-} == helper ]] || exit 0
    o=$d/agent/outbox/1791000000000
    mkdir -p $o
    print -n '{"step":"unit","targets":[["6/9/3","k93"]]}' > $o/work.json
    print -n '{"changes":{"base/6-9-3":"base/6-9-3.9393939393939393.base"}}' > $o/00000000000000000001-1.json
    print -n '{"ok":true,"done":["unit",[["6/9/3","k93"]]]}' > $o/result.json
    ;;
  run)
    d=${2:?its folder}; d=${d:a} r=${3:?the scratch folder}
    rm -f $d/v/app/stop
    while [[ ! -e $d/v/app/stop ]]; do
      print "launcher: $(date '+%H:%M:%S') starting $(readlink $d/v/app/current)" >> $d/agent.log
      SCENIC_COORD_PORT=18091 nice -n 19 $d/v/app/current/scenic agent --root $r --home $d/agent >> $d/agent.log 2>&1 < /dev/null &
      print $! > $d/agent.pid
      print "launcher: $(date '+%H:%M:%S') agent pid $!" >> $d/agent.log
      wait $! || true
      sleep 3
    done
    ;;
  jobs) print -n ${3:?seconds} > ${2:?its folder}/v/app/job-seconds ;;
  newest)
    d=${2:?its folder}
    [[ -e $d/v/app/20261010-0000-ccccccc ]] || cp -R $d/v/app/20261009-0000-bbbbbbb $d/v/app/20261010-0000-ccccccc
    rm -f $d/v/app/20261010-0000-ccccccc/env-*
    ln -sfn 20261010-0000-ccccccc $d/v/app/current
    ;;
  older) ln -sfn 20261008-0000-aaaaaaa ${2:?its folder}/v/app/current ;;
  newer) ln -sfn 20261009-0000-bbbbbbb ${2:?its folder}/v/app/current ;;
  stop)
    d=${2:?its folder}; d=${d:a}
    touch $d/v/app/stop
    [[ -s $d/agent.pid ]] && kill $(<$d/agent.pid) 2>/dev/null || true
    ;;
  *) print -u2 "root, region, app, run, older, newer, newest, jobs or stop"; exit 2 ;;
esac
