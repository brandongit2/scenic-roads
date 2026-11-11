# Records dev jobs for the dev job dashboard (tools/devtools, /jobs/; tools/dev/testing.md, "Dev
# jobs"), sourced by the scripts that run them (check.sh, publish.sh, remote-tests.sh, bin/cargo).
# Each dj_* function appends one event to this Mac's spool (devjob.py's: a JSON line, the same
# shapes), which devjob.py's sampler sends to the dev tools app; nothing here waits on the network.
#   dj_start <kind> <name> [<attr> <value>]…   a span begins: its id in $REPLY. It is live while
#                                                this shell is (the sampler ends it "gone" after).
#   dj_end <id> <status> [<attr> <value>]…     it ends: ok, fail, stopped, skip or reuse
#   dj_span <kind> <name> <t0> <t1> <status> [<attr> <value>]…   a whole span (times: $EPOCHREALTIME's), its id in $REPLY
#   dj_attr <id> [<attr> <value>]…              more of its attributes
#   dj_wrap                                      the command line prefix that runs a command as a
#                                                span (devjob.py wrap), in $reply
# A span is part of $SCENIC_DEVJOB_PARENT (a job exports its own id for what it runs: a check's part
# for the cargo it starts); one of no other gets devjob.py who's attributes (checkout, branch, task…).
# SCENIC_DEVJOB=0: nothing is recorded, the functions do nothing (the tests of these scripts).
# Recording never stops its caller: a write that fails (a full disk, a folder it can't make) is
# dropped, and every function returns 0, so a caller under errexit (publish.sh) goes on.
# A name past devjob.py's NAME_MAX (2000 characters: the app refuses a longer one, and with it every
# span under it) is cut there, ending in "…" (#1123: a cargo's name is its command line).
zmodload zsh/datetime zsh/system
typeset -g DJ_PY=${${(%):-%x}:A:h}/devjob.py
typeset -g DJ_DIR=${SCENIC_DEVJOB_DIR:-$HOME/Library/Logs/scenic/devjobs}
typeset -gi DJ_N=0
dj_on() { [[ ${SCENIC_DEVJOB:-1} != 0 ]] }
if dj_on && [[ -z ${SCENIC_DEVJOB_MAC:-} ]]; then
  export SCENIC_DEVJOB_MAC=$(scutil --get LocalHostName 2>/dev/null || hostname -s)
fi

dj_name() {  # dj_name <name>: it as a JSON string, cut at 2000 characters, in $REPLY
  local n=$1; (( ${#n} > 2000 )) && n="${n[1,1999]}…"
  dj_q "$n"
}
dj_q() {  # dj_q <string>: it as a JSON string, in $REPLY (control characters but tab and newline dropped)
  local s=$1
  s=${s//\\/\\\\}; s=${s//\"/\\\"}; s=${s//$'\n'/\\n}; s=${s//$'\t'/\\t}; s=${s//[$'\x01'-$'\x1f']/}
  REPLY="\"$s\""
}
dj_attrs() {  # dj_attrs <attr> <value>…: a JSON object, in $REPLY
  local o= k
  while (( $# >= 2 )); do
    dj_q "$1"; k=$REPLY; dj_q "$2"; o+="${o:+,}$k:$REPLY"; shift 2
  done
  REPLY="{$o}"
}
dj_emit() {  # one event, one line, appended in one write
  { mkdir -p $DJ_DIR && print -r -- $1 >> $DJ_DIR/spool.ndjson } 2>/dev/null || :
}
dj_sampler() {  # devjob.py's sampler, started unless one runs (its lock held), detached, out of the checkout
  local f
  { mkdir -p $DJ_DIR && : >> $DJ_DIR/sampler.lock } 2>/dev/null || return 0
  zsystem flock -t 0 -f f $DJ_DIR/sampler.lock 2>/dev/null || return 0
  zsystem flock -u $f
  { ( cd / && exec python3 $DJ_PY sampler ) </dev/null >> $DJ_DIR/sampler.log 2>&1 &! } 2>/dev/null || :
}
dj_start() {
  dj_on || { REPLY=; return 0 }
  local kind=$1 name=$2 id parent=${SCENIC_DEVJOB_PARENT:-} who= k n p a in ls
  shift 2
  id=$SCENIC_DEVJOB_MAC-${EPOCHREALTIME/./}-$$-z$(( ++DJ_N ))
  [[ -z $parent ]] && { who=$(python3 $DJ_PY who 2>/dev/null) || who= }
  dj_attrs "$@"; a=$REPLY
  # (A run of its own: who ran it, then its own attributes.)
  if [[ $who == '{'*'}' && $who != '{}' ]]; then in=${${a#\{}%\}}; a="${who%\}}${in:+, $in}}"; fi
  dj_q "$kind"; k=$REPLY; dj_name "$name"; n=$REPLY; dj_q "$parent"; p=$REPLY
  dj_emit "{\"op\":\"start\",\"id\":\"$id\",\"parent\":$p,\"kind\":$k,\"name\":$n,\"mac\":\"$SCENIC_DEVJOB_MAC\",\"t\":$EPOCHREALTIME,\"attrs\":$a}"
  # (Live while this shell is, as it started: a pid used again later isn't it.)
  ls=$(ps -o lstart= -p $$ 2>/dev/null) || ls=; ls=${(j: :)${=ls}}
  { mkdir -p $DJ_DIR/live && print -r -- "$$ $ls" > $DJ_DIR/live/$id } 2>/dev/null || :
  dj_sampler
  REPLY=$id
}
dj_end() {
  dj_on && [[ -n ${1:-} ]] || return 0
  local id=$1 st=$2; shift 2
  dj_attrs "$@"
  dj_emit "{\"op\":\"end\",\"id\":\"$id\",\"t\":$EPOCHREALTIME,\"status\":\"$st\",\"attrs\":$REPLY}"
  rm -f $DJ_DIR/live/$id 2>/dev/null || :
  dj_sampler
}
dj_span() {
  dj_on || { REPLY=; return 0 }
  local kind=$1 name=$2 t0=$3 t1=$4 st=$5 k n p id=$SCENIC_DEVJOB_MAC-${EPOCHREALTIME/./}-$$-z$(( ++DJ_N ))
  shift 5
  dj_q "$kind"; k=$REPLY; dj_name "$name"; n=$REPLY; dj_q "${SCENIC_DEVJOB_PARENT:-}"; p=$REPLY
  dj_attrs "$@"
  dj_emit "{\"op\":\"span\",\"id\":\"$id\",\"parent\":$p,\"kind\":$k,\"name\":$n,\"mac\":\"$SCENIC_DEVJOB_MAC\",\"t0\":$t0,\"t1\":$t1,\"status\":\"$st\",\"attrs\":$REPLY}"
  dj_sampler
  REPLY=$id
}
dj_attr() {
  dj_on && [[ -n ${1:-} ]] || return 0
  local id=$1; shift
  dj_attrs "$@"
  dj_emit "{\"op\":\"attr\",\"id\":\"$id\",\"attrs\":$REPLY}"
  return 0
}
dj_sampling() {  # this Mac sampled while this shell lives, with no span of its own (the test Mac's side of a run)
  dj_on || return 0
  local ls; ls=$(ps -o lstart= -p $$ 2>/dev/null) || ls=; ls=${(j: :)${=ls}}
  { mkdir -p $DJ_DIR/live && print -r -- "$$ $ls sample" > $DJ_DIR/live/sampling-$$ } 2>/dev/null && dj_sampler
  return 0
}
dj_wrap() {  # dj_wrap <kind> <name> [<devjob.py wrap option>…]: in $reply, the prefix to put before a command
  if dj_on; then reply=( python3 $DJ_PY wrap --kind $1 --name $2 "${@:3}" -- )
  else reply=(); fi
}
