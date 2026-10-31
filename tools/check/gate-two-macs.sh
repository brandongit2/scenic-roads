#!/bin/zsh
# The gate's two-Mac trial (docs/inputs.md §4.10): the real agents, in the pool, over a
# scratch folder of the NAS (never the project folder), each Mac's in a folder of its own (never the
# app's), its coordinator on a port of its own ($GATE_PORT, 18092 unless set: never 8090, the real
# agent's, nor pool-two-macs.sh's 18091), checking the
# test unit `_gate-test` with the real `scenic-build inputs`. Run on each Mac, from one build of the
# app (the same on both: a member gets jobs only from a lead on its own app):
#
#   gate-two-macs.sh root <folder>              the scratch folder: the pool's records as it begins
#                                               (no term yet: the first Mac in makes term 1), the
#                                               test unit on the gate, its inputs folder empty (once, on
#                                               one Mac)
#   gate-two-macs.sh app <dir> <bin>            <dir>/v/app/20261010-0000-gatetest/: scenic and
#                                               scenic-build copied from <bin> (target/release);
#                                               <dir>/agent the agent's folder
#   gate-two-macs.sh run <dir> <folder>         the launcher: the agent until <dir>/v/app/stop, again
#                                               whenever it exits (on the M1: inside a persistent ssh
#                                               session, as for pool-two-macs.sh)
#   gate-two-macs.sh stop <dir>                 the launcher and the agent stopped
#   gate-two-macs.sh add <folder> <name> <line>…  a file added (lines, written by a temporary
#                                               name and renamed in, as the owner's tools do)
#   gate-two-macs.sh big <folder> <name> <n>    a file of n good lines (a check that takes a while)
#   gate-two-macs.sh touch <folder> <name>      a file's time changed, its bytes not
#   gate-two-macs.sh remove <folder> <name>     a file removed
#   gate-two-macs.sh state <dir> <folder>       the gate as `scenic inputs` says it, and the records'
#                                               entries
#   gate-two-macs.sh wait <dir> <folder> <regex> [<s>]  waits (300 s) until `scenic inputs` says it
#   gate-two-macs.sh page-accept <folder> <unit> [<id>]  the build page's Accept (Accept All with no
#                                               id): its POST to the lead's coordinator, as a page
#                                               (PAGE_PORT=<port>: to that Mac's own page, as its
#                                               menu bar's panel)
#   gate-two-macs.sh menu-accept <dir> <unit> <id>…  the menu bar item's Accept: its ask file in this
#                                               Mac's agent's folder, as the item writes it (or run
#                                               the item itself: SCENIC_HOME=<dir> scenic-status,
#                                               its status from <dir>/app/current/scenic menu)
#   gate-two-macs.sh lead <folder> <member>     the lead moved: the scratch folder's declaration
#                                               (state/build/lead-member.json) names <member>
#
# The proof, in order (M4 and M1 each with `app` and `run`; <F> the scratch folder; check each step
# with `wait`, keep each Mac's <dir>/agent.log):
#   1. An update taken in: `add <F> a.jsonl '{"k": "x", "v": 1}'`; wait 'taken in' and a version.
#   2. An error held, the last good version kept: `add <F> a.jsonl '{"k": "x", "v": 1}' 'oops'`;
#      wait 'error gt-line'; `state` shows the version of step 1.
#   3. A warning accepted from the build page: `add <F> a.jsonl '{"k": "x", "v": -1}'`; wait
#      'warning gt-neg'; `page-accept <F> _gate-test`; wait 'taken in' with a new version.
#   4. A warning accepted from the other Mac's menu: `add <F> b.jsonl '{"k": "y", "v": -2}'`; on the
#      Mac that doesn't lead, `menu-accept <dir> _gate-test <id>` (the id `state` shows); wait
#      'taken in'.
#   5. An unaccept: `scenic inputs unaccept _gate-test <id>` (with --root <F> --home <dir>/agent);
#      `state` lists the acceptance gone, the version as it was (never the version already in).
#   6. A touch changing nothing: `touch <F> a.jsonl`; the version stays the same (the agent's log
#      shows the check ran, `inputs: _gate-test: 1 file read of 2`).
#   7. A removal held and accepted: `remove <F> b.jsonl`; wait 'warning gt-removed'; accept it (any
#      way); wait 'taken in', b.jsonl gone from the version.
#   8. The lead moved mid-check: `big <F> c.jsonl 2000000`; once `state` says 'checking',
#      `lead <F> <the other member>`; the check's hand-off reaches the new lead's records
#      through the journal: wait 'taken in' with c.jsonl in the version (`state` on either Mac).
#      (To have it end under the new lead, stop the check by its exact pid as it starts,
#      `kill -STOP <pid>` of `<dir>/v/app/current/scenic-build inputs`, move the lead, and
#      `kill -CONT <pid>` once the other Mac leads.)
# Then `scenic inputs test off --root <F>` and the scratch folder removed.
#
# What a run shows (2026-10-10, the M4 and the M1 over a NAS scratch folder, the pool on, its
# parts changing in process, one release build, the same shasum on both; logs kept with the task #134 run):
# 1. taken in at the first listing (2 min), then a check reading nothing ("0 files read of 1").
# 2. the bad line held as `error gt-line`, the version of step 1 kept, on both Macs' `state`.
# 3. `page-accept` wrote the acceptance (by "the build page (<address>)") and it was taken in 36 s
#    later: the agent asks its listing again after an ask, without the two minutes.
# 4. the M1's `menu-accept` wrote the acceptance as the M1's member; taken in at the lead's next
#    listing (129 s): a member's acceptance waits for that listing.
# 5. the unaccept checked again, "0 files read of 2", the version as it was.
# 6. the touch read once, "1 file read of 2", the version the same, only @listed new.
# 7. the removal held as `gt-removed`, accepted with `scenic inputs accept` on the M1; the version
#    went back to step 3's own content name, and the acceptance is listed stale at once.
# 8. not run yet with the lead moved by its declaration.
# A lead new to the term runs each unit's full check once (its own daily timer), here 0.5 s.
# GC (`scenic gc`, 14 days, with a catalogue there: with none GC sweeps nothing), every copy aged 30
# days, then a change: kept the current index and its files, the index it replaced (touched by the
# check, and again with its touch undone, by @listed's `replaced`) and the current @listed; old
# indexes, listings, held reports and copies no version names went. The nested unit
# (`_gate-nest/inner`, never on the gate: `scenic-build inputs` run by hand on its own scratch
# folder without the pool) checked, held, accepted, taken in and swept the same way. After `test
# off` no check runs; the status still lists the unit while its records are there.
set -euo pipefail
usage="usage: tools/check/gate-two-macs.sh root|app|run|stop|add|big|touch|remove|state|wait|page-accept|menu-accept|lead … (see the top of this file)"
case ${1:-} in
  -h|--help) print -r -- $usage; exit 0 ;;
  ""|-*) print -ru2 -- "gate-two-macs.sh: unexpected ${1:-end}"$'\n'"$usage"; exit 2 ;;
esac
cmd=$1
V=20261010-0000-gatetest
U=_gate-test
# An inputs folder's file, written whole by a temporary name (passed over: names starting with "."), then
# renamed in.
put() {
  local f=$1; shift
  local tmp=${f:h}/.${f:t}.tmp
  : > $tmp
  for l in "$@"; do print -r -- "$l" >> $tmp; done
  mv $tmp $f
}
scenic_of() { print ${1:a}/v/app/current/scenic; }
case $cmd in
  root)
    r=${2:?the scratch folder}
    [[ $r == */projects/scenic-roads* ]] && { print -u2 "not the project folder"; exit 1; }
    [[ -e $r ]] && { print -u2 "$r exists"; exit 1; }
    mkdir -p $r/catalog $r/sources $r/state/build $r/state/pool $r/state/inputs $r/inputs/$U
    print -n '{}' > $r/state/build/manifest.json
    print -n '{}' > $r/state/build/jobs.json
    print -n '{}' > $r/state/build/pending.json
    print "turned on by gate-two-macs.sh" > $r/state/inputs/gate-test
    ;;
  app)
    d=${2:?its folder}; d=${d:a} bin=${3:?the folder of scenic and scenic-build}
    mkdir -p $d/agent $d/v/app/$V
    cp $bin/scenic $bin/scenic-build $d/v/app/$V/
    ln -sfn $V $d/v/app/current
    ;;
  run)
    d=${2:?its folder}; d=${d:a} r=${3:?the scratch folder}
    rm -f $d/v/app/stop
    while [[ ! -e $d/v/app/stop ]]; do
      print "launcher: $(date '+%H:%M:%S') starting $(readlink $d/v/app/current)" >> $d/agent.log
      SCENIC_COORD_PORT=${GATE_PORT:-18092} nice -n 10 $d/v/app/current/scenic agent --root $r --home $d/agent >> $d/agent.log 2>&1 < /dev/null || true
      sleep 3
    done
    ;;
  stop)
    d=${2:?its folder}; d=${d:a}
    touch $d/v/app/stop
    # (The agent by the pid its status says, its own process alone.)
    for f in status.json helper.json; do
      [[ -f $d/agent/$f ]] || continue
      pid=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["pid"])' $d/agent/$f)
      [[ $(ps -o command= -p $pid 2>/dev/null) == "$d/v/app/current/scenic agent"* ]] && kill $pid
    done
    ;;
  add)
    r=${2:?the scratch folder}; n=${3:?a file name}; shift 3
    put $r/inputs/$U/$n "$@"
    ;;
  big)
    r=${2:?the scratch folder}; n=${3:?a file name}; k=${4:?how many lines}
    tmp=$r/inputs/$U/.$n.tmp
    python3 -I -c 'import sys; w = sys.stdout.write
for i in range(int(sys.argv[1])): w("{\"k\": \"k%d\", \"v\": %d}\n" % (i, i))' $k > $tmp
    mv $tmp $r/inputs/$U/$n
    ;;
  touch) touch ${2:?the scratch folder}/inputs/$U/${3:?a file name} ;;
  remove) rm ${2:?the scratch folder}/inputs/$U/${3:?a file name} ;;
  state)
    d=${2:?its folder}; r=${3:?the scratch folder}
    $(scenic_of $d) inputs --root $r --home ${d:a}/agent
    # (The records as the build's readers have them: the current term's brief or snapshot, else the newest
    # of a term before it; docs/pool.md §6.2.)
    python3 -I -c 'import json, os, sys
b = sys.argv[1] + "/state/build/"
e = json.load(open(b + "lead.json"))["term"]
while os.path.exists(f"{b}terms/{e + 1}.json"): e += 1
f = next(p for t in range(e, 0, -1) for p in [f"term/{t}/brief.json", f"term/{t}/records.json"] + (["term/1/first.json"] if t == 1 else []) if os.path.exists(b + p))
[print(f"records: {k} -> {v}") for k, v in sorted(json.load(open(b + f))["manifest"].items()) if k.startswith("sources/inputs/")]' $r
    ;;
  wait)
    d=${2:?its folder}; r=${3:?the scratch folder}; re=${4:?what to wait for}; secs=${5:-300}
    t0=$SECONDS
    until $(scenic_of $d) inputs --root $r --home ${d:a}/agent 2>/dev/null | grep -Eq -- "$re"; do
      (( SECONDS - t0 > secs )) && { print -u2 "not in $secs s: $re"; $(scenic_of $d) inputs --root $r --home ${d:a}/agent; exit 1; }
      sleep 5
    done
    print "after $(( SECONDS - t0 )) s: $re"
    ;;
  page-accept)
    r=${2:?the scratch folder}; u=${3:?the unit}; id=${4:-}
    # (A Mac's own page, as the menu bar's panel loads it, PAGE_PORT=<its agent's port>: on a member
    # its agent passes the ask on; else the lead's coordinator's, as a page elsewhere asks.)
    if [[ -n ${PAGE_PORT:-} ]]; then url=http://127.0.0.1:$PAGE_PORT
    else url=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["urls"][0])' $r/state/coordinator.json); fi
    body=$([[ -n $id ]] && print -r -- "{\"unit\": \"$u\", \"accept\": [\"$id\"]}" || print -r -- "{\"unit\": \"$u\", \"all\": true}")
    curl -sS -X POST -H 'Content-Type: application/json' -d "$body" $url/work/inputs
    print
    ;;
  menu-accept)
    d=${2:?its folder}; d=${d:a} u=${3:?the unit}; shift 3
    (( $# )) || { print -u2 "which finding ids?"; exit 2; }
    ids=$(python3 -I -c 'import json, sys; print(json.dumps(sys.argv[1:]))' "$@")
    now=$(date +%s)
    mkdir -p $d/agent/inputs-asks
    f=$(printf '%020d-menu-%d.json' $now $$)
    print -r -- "{\"unit\": \"$u\", \"accept\": $ids, \"by\": \"the menu bar on $(scutil --get ComputerName)\", \"at\": $now}" > $d/agent/inputs-asks/$f.menu.tmp
    mv $d/agent/inputs-asks/$f.menu.tmp $d/agent/inputs-asks/$f
    ;;
  lead)
    r=${2:?the scratch folder} m=${3:?the member id}
    mkdir -p $r/state/build
    print -r -- "{\"member\": \"$m\", \"since\": $(date +%s)}" > $r/state/build/lead-member.json.tmp
    mv $r/state/build/lead-member.json.tmp $r/state/build/lead-member.json
    ;;
  *) print -u2 "root, app, run, stop, add, big, touch, remove, state, wait, page-accept, menu-accept or lead"; exit 2 ;;
esac
