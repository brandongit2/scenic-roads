#!/bin/zsh
# The dev jobs' recorder (tools/dev/devjob.zsh and devjob.py): devjob.py's own tests (test_devjob.py:
# reading outputs and timing reports, the wrapper, the sampler), then devjob.zsh's events, each a
# JSON line devjob.py and the app read, whatever the names and values hold. Into a scratch spool; no
# app is reached (SCENIC_BOARD names a closed port). Run by check.sh when tools/dev/devjob* or this
# changes, and by publish's other tests.
here=${0:A:h}
fail() { print -u2 "FAIL: $*"; exit 1; }
python3 -m unittest discover -s $here -p 'test_devjob.py' || fail "test_devjob.py"
s=$(mktemp -d); trap 'rm -rf $s' EXIT
export SCENIC_DEVJOB_DIR=$s SCENIC_BOARD=http://127.0.0.1:9 SCENIC_DEVJOB_MAC=toy
unset SCENIC_DEVJOB SCENIC_DEVJOB_PARENT
source $here/../devjob.zsh
weird=$'a "quoted" \\ back\tslash\nnew line \x01 and ünïcode'
dj_start check $weird task 946 note $weird; root=$REPLY
[[ -n $root && -e $s/live/$root ]] || fail "dj_start: no id or no live file"
read -r pid rest < $s/live/$root
[[ $pid == $$ ]] || fail "live file names $pid, not this shell ($$)"
SCENIC_DEVJOB_PARENT=$root dj_span part fmt $EPOCHREALTIME $EPOCHREALTIME ok why $weird
dj_attr $root more yes
dj_end $root fail exit 1
[[ ! -e $s/live/$root ]] || fail "dj_end left the live file"
python3 - $s/spool.ndjson*(N) $s/sending-*(N) "$weird" $root <<'PY' || fail "devjob.zsh's events"
import json, sys
*files, weird, root = sys.argv[1:]
ev = [json.loads(l) for f in files for l in open(f)]
ev = [e for e in ev if e["op"] != "sample"]
ops = [e["op"] for e in ev]
assert ops[:4] == ["start", "span", "attr", "end"], ops
s, p, a, e = ev[:4]
assert s["id"] == root and s["parent"] == "" and s["kind"] == "check" and s["mac"] == "toy", s
assert s["name"] == weird.replace("\x01", ""), repr(s["name"])
assert s["attrs"]["task"] == "946" and s["attrs"]["note"] == s["name"], s["attrs"]
assert "by" in s["attrs"], "a run of its own says who ran it"
assert p["parent"] == root and p["t0"] <= p["t1"] and p["attrs"]["why"] == s["name"], p
assert a == {"op": "attr", "id": root, "attrs": {"more": "yes"}}, a
assert e["status"] == "fail" and e["attrs"] == {"exit": "1"} and e["t"] >= s["t"], e
PY
# (The sampler these started ends once nothing is live: waited for, so the scratch spool goes after it.)
for (( i = 0; i < 600; i++ )); do zsystem flock -t 0 -f f $s/sampler.lock 2>/dev/null && break; sleep 0.1; done
# A caller under set -u (check.sh, publish.sh): empty names, values and parents, an empty id.
( setopt nounset; unset SCENIC_DEVJOB_PARENT
  dj_start part "" k ""; id=$REPLY
  SCENIC_DEVJOB_PARENT= dj_span part "" $EPOCHREALTIME $EPOCHREALTIME ok "" ""
  dj_attr "" k v; dj_end "" ok; dj_end $id ok ) || fail "the functions under set -u"
# A caller under errexit (publish.sh) goes on when nothing can be written (the spool's folder a file).
( setopt errexit; SCENIC_DEVJOB_DIR=$s/kept DJ_DIR=$s/kept; : > $s/kept; unset SCENIC_DEVJOB_PARENT
  dj_start part x; id=$REPLY; dj_attr $id k v; dj_span part y $EPOCHREALTIME $EPOCHREALTIME ok
  dj_sampling; dj_end $id ok; print -r -- went > $s/went ) 2>/dev/null
[[ -e $s/went ]] || fail "the functions stopped a caller under errexit"
# A wrapped command keeps the files its caller passed on (check.sh's SCENIC_CARGO_SAY is its fd).
exec {keep}>$s/kept
dj_wrap part fd; $reply zsh -c "print -u $keep through" > /dev/null || fail "the wrapped command"
exec {keep}>&-
[[ $(<$s/kept) == through ]] || fail "the wrapped command lost its caller's fd: $(<$s/kept)"
# A name past the app's 2000 characters is cut there, not sent whole to be refused with its spans (#1123).
rm -rf $s/*(N)
long=${(l:5000::x:)}
dj_start cargo $long; id=$REPLY; dj_span wait $long $EPOCHREALTIME $EPOCHREALTIME ok; dj_end $id ok
python3 -c '
import json, sys
ev = [json.loads(l) for f in sys.argv[1:] for l in open(f)]
names = [e["name"] for e in ev if e["op"] in ("start", "span")]
assert [(len(n), n[-1]) for n in names] == [(2000, "…")] * 2, [(len(n), n[-3:]) for n in names]
' $s/spool.ndjson*(N) $s/sending-*(N) || fail "a long name not cut to the app's limit"
for (( i = 0; i < 600; i++ )); do zsystem flock -t 0 -f f $s/sampler.lock 2>/dev/null && break; sleep 0.1; done
# SCENIC_DEVJOB=0: nothing.
rm -rf $s/*(N)
SCENIC_DEVJOB=0 dj_start check x; [[ -z $REPLY && ! -e $s/spool.ndjson ]] || fail "SCENIC_DEVJOB=0 recorded"
dj_wrap part x; (( ${#reply} )) || fail "dj_wrap gave nothing"
SCENIC_DEVJOB=0 dj_wrap part x; (( ! ${#reply} )) || fail "dj_wrap with SCENIC_DEVJOB=0: ${reply[*]}"
print "devjob: ok"
