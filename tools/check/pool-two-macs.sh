#!/bin/zsh
# The pool's two-Mac test (docs/pool.md §13): the real agents, in the pool, over a scratch
# folder of the NAS (never the project folder), each Mac's in a folder of its own (never the
# app's), its coordinator on a port of its own (never 8090, the real agent's), its jobs played by a
# script that hands off one save within its step's outputs. Run on each Mac, from a build of the app:
#
#   pool-two-macs.sh root <folder> [slots]          the scratch folder, as the pool begins: today's
#                                                   records, no term yet (the first Mac in makes
#                                                   term 1) (`slots`: state/pool/slots on, a part
#                                                   changing in its process)
#   pool-two-macs.sh region <folder> [scenic-build]  a pass (its marker alone) and a region (a 20 km
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
#   pool-two-macs.sh fill <dir> <MB>                each fake job writes a file of <MB> at the top
#                                                   of its scratch folder (0: none), for the disk
#                                                   it's measured to take; size it against the
#                                                   Mac's free space less 30 GB to have the disk
#                                                   guard freeze one
#   pool-two-macs.sh mem <dir> <MB> [<noted MB>]    each fake job holds <MB> of memory while it
#                                                   runs (every page touched; 0: none), and notes
#                                                   <noted MB> as its targets' peak (50 else): noted
#                                                   below what it holds, its Mac packs more of them
#                                                   than fit, for the memory guard to stop
#   pool-two-macs.sh noted-secs <dir> [<s>]         each fake job notes <s> as each target's time
#                                                   (none: its own time shared among its targets)
#   pool-two-macs.sh as <dir> <worker> <port> [<MB>]
#                                                   the agent, from its next start, works under the
#                                                   name <worker> (SCENIC_TEST_WORKER: a member
#                                                   beside the lead on one Mac), its coordinator on
#                                                   <port> (18091 else), counting its Mac's memory
#                                                   as <MB> (SCENIC_TEST_MEM_MB: its limit squeezed)
#   pool-two-macs.sh packing <folder> [off]         state/pool/packing in the scratch folder: a
#                                                   member asks for and runs several jobs at once
#   pool-two-macs.sh extra <dir> <MB>             the agent, from its next start, counts <MB>
#                                                   more free on its disk than there is
#                                                   (SCENIC_TEST_DISK_EXTRA_MB; 0: none): with <dir>
#                                                   on a small disk image (hdiutil, never the Mac's
#                                                   own volume), the image stands for a disk past
#                                                   the 30 GB reserve, and a fill lands in it
#   pool-two-macs.sh wiki <dir> <n> [hold-s]        each fake job asks Wikimedia <n> times through
#                                                   its proxy (http://en.wikipedia.org/budget-test,
#                                                   answered on the Mac: `upstream`), noting each
#                                                   answer's time (<dir>/v/app/wiki-<step>-<pid>),
#                                                   and tries to reach the NAS's SMB port around the
#                                                   proxy (denied); `hold-s`: the
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
#   pool-two-macs.sh brokers <folder> [off]         state/pool/brokers in the scratch folder: each
#                                                   member brokers its own jobs' tasks, pages call
#                                                   the members directly (`off`: the switch removed)
#   pool-two-macs.sh placement <folder> [off]       state/pool/placement in the scratch folder:
#                                                   members' asks and tasks placed by how crucial
#                                                   and by measured speed, jobs offering ahead
#                                                   (`off`: the switch removed)
#   pool-two-macs.sh task <dir> [off]               each fake job offers a task through its Mac's
#                                                   coordinator or broker (SCENIC_COORD) before it
#                                                   sleeps, and notes the task's state as it ends
#                                                   (<dir>/v/app/task-<step>-<pid>)
#   pool-two-macs.sh page <base> <origin> <file>    a page (curl, its Origin <origin>): asks <base>
#                                                   where work is, then each broker it names for a
#                                                   task (its lease and broker kept in <file>), a
#                                                   broker that doesn't answer in 8 s passed over
#   pool-two-macs.sh page-done <origin> <file>      that page beats its lease and hands its task
#                                                   back, where it took it
#   pool-two-macs.sh publish <folder>               after `root`: a region done that the map hasn't
#                                                   (the agent's test `seed_a_scratch_publish`, from
#                                                   this repository), so the lead publishes it, its
#                                                   map tiles shared with `every`; publishing's
#                                                   switches (plan.md §8), each `: > state/pool/<s>`
#   pool-two-macs.sh stop <dir>                    the launcher and the agent stopped (the agent
#                                                   by its pid, <dir>/agent.pid, which the launcher
#                                                   writes, and logs, at each start)
#
# On the M1, run its launcher inside a persistent ssh session, kept open (from the lead:
# `ssh brandontsang@macbookpro '<this script> run …'` left running), not under `nohup` nor a
# `screen -dm` started by an ssh command that then ends (both seen 2026-10-10): macOS's
# local-network privacy then cuts the agent off the LAN (the NAS's LAN name doesn't answer), so it
# reports itself away from home, can't lead (`able` false: a handover to it is offered and given up
# after a minute), and never takes over by itself.
#
# What a run shows (2026-10-08, the M4 the lead, the M1 a member): the lead makes term 1
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
# With state/pool/every-step on (2026-10-15, `root`, `region`, `every`): the lead running its
# pass's sets, alone, its free room none, the region's terrain and tree cover pieces (never run:
# cold starts) went to the M1 one a job, both merged; no claim file made. (The fake jobs note what
# each target took, SCENIC_COSTS, for the lead's offers by the measures.)
#
set -euo pipefail
cmd=${1:?root, region, publish, app, run, older, newer, newest, jobs, outside, fill, extra, wiki, upstream, resume, every, brokers, placement, task, page, page-done, planet, progress or stop}
case $cmd in
  root)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    [[ -e $r ]] && { print -u2 "$r exists"; exit 1; }
    mkdir -p $r/catalog $r/inputs/regions $r/state/pool $r/state/build
    print -n '{"base/6-1-1": "base/6-1-1.1111111111111111.base"}' > $r/state/build/manifest.json
    print -n '{"unit": {"6/1/1": "k1"}}' > $r/state/build/jobs.json
    print -n '{}' > $r/state/build/pending.json
    [[ ${3:-} == slots ]] && : > $r/state/pool/slots
    ;;
  region)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    mkdir -p $r/sources/osm/2026-09-28 $r/inputs/regions
    print -n '{}' > $r/sources/osm/2026-09-28/pass.0000000000000000.json
    # (The regions' accepted version, a 20 km circle round 7° E 46° N: the build reads no drop box.)
    ${3:-scenic-build} regions-fixture --root $r b4test 7.0,46.0,20
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
# (A task offered through its Mac's coordinator or broker: `task`.)
tid=""
if [ -e "$d/../job-task" ] && [ -n "$SCENIC_COORD" ]; then
  troot="$d/../tasks/$step-$$"; mkdir -p "$troot/u"; printf in > "$troot/u/in.bin"
  tid=$(/usr/bin/curl -sS -X POST -H "Authorization: Bearer $SCENIC_COORD_TOKEN" -H 'Content-Type: application/json' -d "{\"owner\":$$,\"kind\":\"tail\",\"spec\":{\"unit\":\"$step-$$\",\"runs\":[]},\"root\":\"$troot\",\"inputs\":{\"u/in.bin\":2},\"mem_mb\":100}" "$SCENIC_COORD/task/offer" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  echo "offered $tid at $SCENIC_COORD $(date +%s)" >> "$d/task-$step-$$"
fi
secs=$(cat "$d/../job-seconds" 2>/dev/null || echo 5)
# (The memory it holds while it runs, every page touched (`mem`), else none.)
mem=$(cat "$d/../job-mem-mb" 2>/dev/null || echo 0)
if [ "$mem" -gt 0 ]; then
  /usr/bin/python3 -c "import time; b = bytearray(b'\x01') * ($mem << 20); time.sleep($secs)"
else
  sleep "$secs"
fi
if [ -n "$tid" ]; then
  echo "ended $(/usr/bin/curl -sS -X POST -H "Authorization: Bearer $SCENIC_COORD_TOKEN" -H 'Content-Type: application/json' -d '{}' "$SCENIC_COORD/task/$tid") $(date +%s)" >> "$d/task-$step-$$"
fi
# (What each target took, as a real step notes it (SCENIC_COSTS): the job's time shared among its
# targets, 50 MB, the way its step runs now; for the lead's offers by the measures.)
if [ -n "$SCENIC_COSTS" ]; then
  ts=""; prev=""
  for a in "$@"; do
    case "$prev" in --root|--scratch|--pass|--dem|--raw|--cache|--cache-dir|--chm|--mirror|--date|--planet|--expect-same|--coarse-threads) prev="$a"; continue ;; esac
    case "$a" in [0-9]*/[0-9]*/[0-9]*) ts="$ts $a" ;; esac
    prev="$a"
  done
  n=$(echo $ts | wc -w | tr -d ' ')
  case "$step" in terrain|slope) v=3 ;; trees) v=2 ;; unit|trees-lo|terrain-lo|slope-lo) v=1 ;; *) v=0 ;; esac
  for t in $ts; do
    if [ "$step" = unit ]; then k="$t"; else k="$step $t"; fi
    # (Its peak as `mem` notes it, 50 MB else; the threads it was given, for packing's cores.)
    peak=$(cat "$d/../job-noted-mb" 2>/dev/null || echo 50)
    each=$(cat "$d/../job-noted-secs" 2>/dev/null || echo $((secs / n)))
    echo "{\"unit\":\"$k\",\"peak_mb\":$peak,\"secs\":$each,\"v\":$v,\"disk_mb\":1,\"sizes\":{\"threads\":${RAYON_NUM_THREADS:-1}}}" >> "$SCENIC_COSTS"
  done
fi
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
      SCENIC_TEST_PROXY_TO=$(cat $d/proxy-to 2>/dev/null) SCENIC_TEST_DISK_EXTRA_MB=$(cat $d/disk-extra-mb 2>/dev/null || echo 0) SCENIC_TEST_WORKER=$(cat $d/worker 2>/dev/null) SCENIC_TEST_MEM_MB=$(cat $d/mem-mb 2>/dev/null) SCENIC_COORD_PORT=$(cat $d/coord-port 2>/dev/null || echo 18091) nice -n 19 $d/v/app/current/scenic agent --root $r --home $d/agent >> $d/agent.log 2>&1 < /dev/null &
      print $! > $d/agent.pid
      print "launcher: $(date '+%H:%M:%S') agent pid $!" >> $d/agent.log
      wait $! || true
      sleep 3
    done
    ;;
  publish)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    SCENIC_SCRATCH_PUBLISH=$r cargo test -p pipeline --lib -- --ignored agent::tests::seed_a_scratch_publish
    ;;
  jobs) print -n ${3:?seconds} > ${2:?its folder}/v/app/job-seconds ;;
  outside) : > ${2:?its folder}/v/app/job-outside ;;
  extra) print -n ${3:?MB} > ${2:?its folder}/disk-extra-mb ;;
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
  mem)
    d=${2:?its folder}
    print -n ${3:?MB} > $d/v/app/job-mem-mb
    if [[ -n ${4:-} ]]; then print -n $4 > $d/v/app/job-noted-mb; else rm -f $d/v/app/job-noted-mb; fi
    ;;
  noted-secs)
    # (Each target's time as the fake jobs note it, whatever they ran: a job's worth, so the lead
    # offers a target a job and a member packs several.)
    d=${2:?its folder}
    if [[ -n ${3:-} ]]; then print -n $3 > $d/v/app/job-noted-secs; else rm -f $d/v/app/job-noted-secs; fi
    ;;
  as)
    d=${2:?its folder}
    print -n ${3:?a worker name} > $d/worker
    print -n ${4:?a port} > $d/coord-port
    if [[ -n ${5:-} ]]; then print -n $5 > $d/mem-mb; else rm -f $d/mem-mb; fi
    ;;
  packing)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    mkdir -p $r/state/pool
    if [[ ${3:-} == off ]]; then print off > $r/state/pool/packing; else print on > $r/state/pool/packing; fi
    ;;
  brokers)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    if [[ ${3:-} == off ]]; then rm -f $r/state/pool/brokers; else mkdir -p $r/state/pool; : > $r/state/pool/brokers; fi
    ;;
  placement)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    if [[ ${3:-} == off ]]; then rm -f $r/state/pool/placement; else mkdir -p $r/state/pool; : > $r/state/pool/placement; fi
    ;;
  task)
    d=${2:?its folder}
    if [[ ${3:-} == off ]]; then rm -f $d/v/app/job-task; else : > $d/v/app/job-task; fi
    ;;
  page)
    base=${2:?the member of the page} origin=${3:?its origin} f=${4:?its lease file}
    ask='{"worker":"page two-macs","kind":"web","can":["tail"],"mem_mb":1000,"cores":2}'
    w=$(/usr/bin/curl -sS -m 8 -X POST -H "Origin: $origin" -H 'Content-Type: application/json' -H 'X-Worker: page two-macs' -d '{"can":["tail"],"mem_mb":1000}' $base/work/where)
    print "where: $w"
    for b in $(print -r -- $w | /usr/bin/python3 -c 'import json,sys; [print(b["base"]) for b in json.load(sys.stdin)["brokers"]]'); do
      g=$(/usr/bin/curl -sS -m 8 -D $f.h -X POST -H "Origin: $origin" -H 'Content-Type: application/json' -H 'X-Worker: page two-macs' -d $ask $b/work/ask) || { print "$b: no answer in 8 s, passed over"; continue; }
      print "$b: $(grep -i '^access-control-allow-origin' $f.h | tr -d '\r') $g"
      [[ $g == *'"lease"'* ]] || continue
      print -r -- "$b $(print -r -- $g | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["lease"])')" > $f
      break
    done
    ;;
  page-done)
    origin=${2:?its origin} f=${3:?its lease file}
    read b lease < $f
    print "beat at $b: $(/usr/bin/curl -sS -m 8 -X POST -H "Origin: $origin" -H 'Content-Type: application/json' -H 'X-Worker: page two-macs' -d "{\"worker\":\"page two-macs\",\"lease\":$lease}" $b/work/beat)"
    print "done at $b: $(/usr/bin/curl -sS -m 8 -o /dev/null -w '%{http_code}' -X POST -H "Origin: $origin" -H 'Content-Type: application/json' -H 'X-Worker: page two-macs' -d "{\"worker\":\"page two-macs\",\"lease\":$lease,\"outputs\":[],\"secs\":1,\"peak_mb\":10}" $b/work/done)"
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
  *) print -u2 "root, region, app, run, older, newer, newest, jobs, outside, fill, extra, wiki, upstream, resume, every, brokers, placement, task, page, page-done, planet, progress or stop"; exit 2 ;;
esac
