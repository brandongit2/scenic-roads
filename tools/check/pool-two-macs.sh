#!/bin/zsh
# The pool's two-Mac test (docs/pool.md §13): the real agents, with the pool on, over a scratch
# folder of the NAS (never the project folder), each Mac's in a folder of its own (never the
# app's), its coordinator on a port of its own (never 8090, the real agent's), its jobs played by a
# script that hands off one save within its step's outputs. Run on each Mac, from a build of the app:
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
#   pool-two-macs.sh outside <dir>                  the next fake job's hand-off also writes a name
#                                                   outside every step's outputs: the merge refuses
#                                                   its entry (docs/pool.md §7.3)
#   pool-two-macs.sh measure <folder> [off]         state/pool/by-measure in the scratch folder:
#                                                   the Mac shared by measure and its disk guard
#                                                   (`off`: the switch removed)
#   pool-two-macs.sh fill <dir> <MB>                each fake job writes a file of <MB> at the top
#                                                   of its scratch folder (0: none), for the disk
#                                                   it's measured to take; size it against the
#                                                   Mac's free space less 30 GB to have the disk
#                                                   guard freeze one
#   pool-two-macs.sh extra <dir> <MB>               the agent, from its next start, counts <MB>
#                                                   more free on its disk than there is
#                                                   (SCENIC_TEST_DISK_EXTRA_MB; 0: none): with <dir>
#                                                   on a small disk image (hdiutil, never the Mac's
#                                                   own volume), the image stands for a disk past
#                                                   the 30 GB reserve, and a fill lands in it
#   pool-two-macs.sh budgets <folder> [off]         state/pool/budgets in the scratch folder: every
#                                                   job's connections through its agent's proxy,
#                                                   sandboxed, Wikimedia's metered by the pool's one
#                                                   budget there (`off`: the switch removed)
#   pool-two-macs.sh wiki <dir> <n> [hold-s]        each fake job asks Wikimedia <n> times through
#                                                   its proxy (http://en.wikipedia.org/budget-test,
#                                                   answered on the Mac: `upstream`), noting each
#                                                   answer's time (<dir>/v/app/wiki-<step>-<pid>),
#                                                   and tries to reach the NAS's SMB port around the
#                                                   proxy (denied, with the switch on); `hold-s`: the
#                                                   next job first tells its proxy a 429 of that many
#                                                   seconds (once)
#   pool-two-macs.sh upstream <dir> <port>          a server on 127.0.0.1:<port> answering the fake
#                                                   jobs' requests (python's http.server, in the
#                                                   foreground); the agent's proxy, from its next
#                                                   start, opens every connection to it
#                                                   (SCENIC_TEST_PROXY_TO), so nothing reaches
#                                                   Wikimedia
#   pool-two-macs.sh resume <folder> [off]          state/pool/resume in the scratch folder:
#                                                   resuming on a job's Mac, a pass complete by the
#                                                   records (`off`: the switch removed)
#   pool-two-macs.sh planet <folder> <date>         a planet of <date> in the scratch folder (a
#                                                   stand-in file) and Planetiler's jar: an OSM pass
#                                                   to plan (the fake pass job marks its filter done
#                                                   in its scratch folder, notes the unmerged file
#                                                   it was given, and hands off the pass's marker)
#   pool-two-macs.sh progress <folder> <member> <host> <date> [done]
#                                                   that member's progress file holding the pass of
#                                                   <date>, noted now (as its Mac's agent notes a
#                                                   pass stopped there mid-stage)
#   pool-two-macs.sh stop <dir>                    the launcher and the agent stopped (the agent
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
#
# With state/pool/slots on (2026-10-10, `root … slots`, `region`, `jobs … 300`): the lead handed to
# the member by `scenic lead give` mid-job, neither agent restarting (same pids), the member's
# coordinator up in its process and the old lead's port free, the contact the new lead's, the lead's
# idle-sleep assertion (`caffeinate -i -w <agent>`) moving with the lead; the member's job, granted in
# the term before, ending in the new lead's coordinator, and the old lead's own job ending there by
# HTTP, both entries in the new lead's records; the old lead, a member, given a job by the new lead
# over HTTP, merged; `newest` on the member, then on the lead mid-job: the lead hands the lead over
# once, its job running on and ending at the new lead, then restarts into the update; the lead
# frozen (SIGSTOP) past ten minutes and taken over (`scenic lead take`, unforced), then thawed: it
# stands down to a member in its process, its coordinator stopped and port free, the new lead's
# contact left alone, nothing granted; a lead killed (-9) mid-job relaunched, re-asserting, its
# coordinator up, the job its process left stopped and started again.
#
# With state/pool/resume on (2026-10-15, `resume`, then `progress` naming the M1's member and host
# once its heartbeat is there, its agent restarted so it reads its file, then `planet`): the lead
# waits ("its progress is on <host>'s disk … offered to it alone"), the M1 runs the pass in its own
# scratch folder with its unmerged file (`<scratch>/unmerged-<pid>`), the entry merged, the pass
# complete by the records, the M1's file dropping it; the lead never starts the pass.
#
# For phase 4's fifth batch (planned: before `state/pool/by-measure` goes on for real): `measure`,
# `region`, `jobs … 120`, `fill … 512`, both Macs. What to see: every job's disk floor in the lead's
# `scenic pool floors` (its `disk` lines, the member's too, by its done), with the switch off and
# on; the switch on, the lead's first job of a step alone (a cold start: the status's `beside_why`
# says so), then a second job beside it once its step has run, whatever the steps table's lists
# (a terrain piece beside a slope piece; GC beside a job); a fill past the free space less 30 GB
# freezing the job growing fastest (the log's "disk guard froze"), room made, and the job going on
# or given back (its lease ended at the lead, not failed, kept from that Mac an hour); a handover
# mid-job with the switch on, the floors moving with the term; GC's report naming the entries it
# kept (`unmerged`) while a member's entry waits unmerged.
set -euo pipefail
cmd=${1:?root, region, app, run, older, newer, newest, jobs, outside, measure, fill, extra, budgets, wiki, upstream, resume, planet, progress or stop}
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
# (Its scratch folder, as its arguments name it: a file of the size asked for at its top.)
scratch=""; prev=""
for a in "$@"; do [ "$prev" = "--scratch" ] && scratch="$a"; prev="$a"; done
fill=$(cat "$d/../job-fill-mb" 2>/dev/null || echo 0)
if [ -n "$scratch" ] && [ "$fill" -gt 0 ]; then
  mkdir -p "$scratch"
  dd if=/dev/zero of="$scratch/fill-$$.bin" bs=1048576 count="$fill" 2>/dev/null
fi
# (Wikimedia through the proxy, metered: `wiki`; a 429 told first: its hold.)
wiki=$(cat "$d/../job-wiki" 2>/dev/null || echo 0)
if [ "$wiki" -gt 0 ] && [ -n "$SCENIC_BUDGETS" ]; then
  out="$d/wiki-$step-$$"
  if [ -e "$d/../job-hold" ] && mv "$d/../job-hold" "$d/../job-hold-$$" 2>/dev/null; then
    hs=$(cat "$d/../job-hold-$$"); rm -f "$d/../job-hold-$$"
    echo "hold $hs $(/usr/bin/python3 -c 'import time; print(int(time.time()*1000))') $(/usr/bin/curl -sS --noproxy '*' -X POST "$SCENIC_BUDGETS/hold?s=$hs")" >> "$out"
  fi
  i=0
  while [ $i -lt "$wiki" ]; do
    code=$(/usr/bin/curl -sS -o /dev/null -w '%{http_code}' -m 600 http://en.wikipedia.org/budget-test)
    echo "got $code $(/usr/bin/python3 -c 'import time; print(int(time.time()*1000))')" >> "$out"
    i=$((i + 1))
  done
  /usr/bin/nc -v -z -G 5 "$(cat "$d/../nas-ip" 2>/dev/null || echo 192.168.2.2)" 445 >> "$out" 2>&1; echo "nas-direct $?" >> "$out"
fi
sleep "$(cat "$d/../job-seconds" 2>/dev/null || echo 5)"
[ -n "$scratch" ] && rm -f "$scratch/fill-$$.bin"
if [ -n "$SCENIC_HANDOFF" ]; then
  mkdir -p "$SCENIC_HANDOFF"
  # A save within its step's outputs (crate::agent::steps::writes, which the merge refuses
  # anything outside of): a worldwide layer's root pack, or a global file; a shared step's none (its
  # files are its targets', which these jobs don't name), nor a step's that writes none; an upload
  # checked otherwise. `outside`: the next job also writes a name outside every step's, which
  # the merge refuses.
  case "$step" in
    water|labels|stations|ferries) l="layers/$step/root/0-0-0" ;;
    terrain-root|slope-root) l="layers/${step%-root}/root/0-0-0" ;;
    spoken|roadunits) l="global/$step" ;;
    *) l="" ;;
  esac
  # (The OSM pass: its filter marked done in its scratch folder, and the pass's marker handed off.)
  date=""; prev=""
  for a in "$@"; do [ "$prev" = "--date" ] && date="$a"; prev="$a"; done
  if [ "$step" = osm-pass ] && [ -n "$scratch" ]; then
    mkdir -p "$scratch" && : > "$scratch/filter.done" && echo "${SCENIC_UNMERGED:-none}" > "$scratch/unmerged-$$"
  fi
  case "$step" in
    osm-pass) ch="\"sources/osm/$date/pass\":\"sources/osm/$date/pass.0123456789abcdef.json\"" up="" ;;
    terrain|slope|trees|unit|pois|peaks|bldprep|bldtiles) ch="" up="" ;;
    *) ch=${l:+"\"$l\":\"$l.0123456789abcdef.pack\""} up="\"work/test-$step.0123456789abcdef.json\":\"aa\"" ;;
  esac
  if [ -e "$d/../job-outside" ] && rm "$d/../job-outside" 2>/dev/null; then
    ch="${ch:+$ch,}\"work/test-$step\":\"work/test-$step.0123456789abcdef.json\""
  fi
  printf '{"changes":{%s},"pending":{%s}}' "$ch" "$up" > "$SCENIC_HANDOFF/00000000000000000001-$$.json"
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
      SCENIC_TEST_PROXY_TO=$(cat $d/proxy-to 2>/dev/null) SCENIC_TEST_DISK_EXTRA_MB=$(cat $d/disk-extra-mb 2>/dev/null || echo 0) SCENIC_COORD_PORT=18091 nice -n 19 $d/v/app/current/scenic agent --root $r --home $d/agent >> $d/agent.log 2>&1 < /dev/null &
      print $! > $d/agent.pid
      print "launcher: $(date '+%H:%M:%S') agent pid $!" >> $d/agent.log
      wait $! || true
      sleep 3
    done
    ;;
  jobs) print -n ${3:?seconds} > ${2:?its folder}/v/app/job-seconds ;;
  outside) : > ${2:?its folder}/v/app/job-outside ;;
  extra) print -n ${3:?MB} > ${2:?its folder}/disk-extra-mb ;;
  budgets)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    if [[ ${3:-} == off ]]; then rm -f $r/state/pool/budgets; else mkdir -p $r/state/pool; : > $r/state/pool/budgets; fi
    ;;
  wiki)
    d=${2:?its folder}
    print -n ${3:?requests a job} > $d/v/app/job-wiki
    [[ -n ${4:-} ]] && print -n $4 > $d/v/app/job-hold
    # (The NAS's address, for the job's try around the proxy.)
    route -n get fishandchips.local >/dev/null 2>&1 && dscacheutil -q host -a name fishandchips.local | awk '/ip_address/ {print $2; exit}' | tr -d '\n' > $d/v/app/nas-ip || true
    ;;
  upstream)
    d=${2:?its folder}; d=${d:a} port=${3:?a port}
    print -n 127.0.0.1:$port > $d/proxy-to
    mkdir -p $d/upstream
    cd $d/upstream && exec /usr/bin/python3 -m http.server --bind 127.0.0.1 $port
    ;;
  fill) print -n ${3:?MB} > ${2:?its folder}/v/app/job-fill-mb ;;
  measure)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    if [[ ${3:-} == off ]]; then rm -f $r/state/pool/by-measure; else mkdir -p $r/state/pool; : > $r/state/pool/by-measure; fi
    ;;
  resume)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    if [[ ${3:-} == off ]]; then rm -f $r/state/pool/resume; else mkdir -p $r/state/pool; : > $r/state/pool/resume; fi
    ;;
  planet)
    r=${2:?the scratch folder} date=${3:?its date}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    mkdir -p $r/sources/osm/$date $r/sources/basemap
    print -n planet > $r/sources/osm/$date/planet.osm.pbf
    print -n jar > $r/sources/basemap/planetiler.jar
    ;;
  progress)
    r=${2:?the scratch folder} m=${3:?its member} h=${4:?its host} date=${5:?the pass date}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    mkdir -p $r/state/pool/progress
    done=false; [[ ${6:-} == done ]] && done=true
    print -n "{\"member\":\"$m\",\"worker\":\"$h\",\"jobs\":{\"osm-pass $date\":{\"at\":$(date +%s),\"done\":$done}}}" > $r/state/pool/progress/$m.json
    ;;
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
  *) print -u2 "root, region, app, run, older, newer, newest, jobs, outside, measure, fill, extra, budgets, wiki, upstream, resume, planet, progress or stop"; exit 2 ;;
esac
