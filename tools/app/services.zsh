#!/bin/zsh
# The services the Mac mini runs beside the app (docs/macs.md, Services): the dev tools app
# (tools/devtools: the task board and the dev job dashboard, on :8077) and the map's site in the
# Mac's Caddy (tools/caddy/scenic.caddy: the map's name on the tailnet). Every release ships them in
# the app's services/ folder (tools/app/lib.zsh's services_stage: this script, caddy/scenic.caddy,
# and devtools/ laid out as in the repo), and the Mac's agent keeps them at the version it runs
# (agentcore::agent::services): once its coordinator listens on this Mac's port, it runs this
# version's `services.zsh update`, so Caddy's routes change only once the coordinator serving them
# is up, and only on a Mac where a service was set up.
#
#   services.zsh setup devtools [--node <path>] [--data <folder>] [--port <n>]
#   services.zsh setup caddy [--caddy <path>] [--config <Caddyfile>] [--name <name>]
#                            [--cert <file>] [--key <file>] [--https-port <n>]
#   services.zsh update [--reload]
#
# setup (once per service, by hand, from the installed app's copy, run as a program:
# ~/scenic/app/current/services/services.zsh) writes the service's settings
# (<home>/services/<service>/setup), then runs update for that service as --reload does.
# - The dev tools': also their LaunchAgent, which runs them from <home>/services/, loaded again from
#   its new plist. Its defaults are what that LaunchAgent runs now, if anything: its node and data
#   folder (`--data`, else its working folder: on the Mac mini, the git checkout's tools/devtools,
#   where the board's data is).
# - Caddy's: no LaunchAgent. Caddy is the Mac's own, run by a LaunchAgent of the Mac's
#   (`local.caddy`), whose Caddyfile (`--config`, ~/srv/caddy/Caddyfile) imports every project's
#   site file from the sites/ folder beside it and enables Caddy's admin API, so a site changes by
#   a reload. Its defaults are its setup's before, else `caddy` on the PATH, that Caddyfile, the
#   first name in ~/.config/scenic/names, ~/certs/map.crt and map.key, and port 443. It also writes
#   the name first in ~/.config/scenic/names (its other names kept), which the map server and the
#   agent read (coordwire::net::names).
#
# update, for each service set up, the dev tools first:
# - the dev tools: left as they are when they answer at this version, or at a copy whose files are
#   the same (most releases don't change them). Else their databases copied first, while the old version
#   runs (VACUUM INTO, consistent, checked; to devtools/backup/; a copy failing, nothing changes);
#   this version's copy put in <home>/services/devtools/<version>/, `current` pointed at it, the job
#   restarted (launchctl kickstart -k: it stays loaded, so a run stopped part way leaves launchd
#   running one or the other) and checked: a new process answering the board's and the dashboard's
#   APIs within 30 s. A version that doesn't answer is put back, and when it changed a database's
#   form (its user_version or schema_version: a change of a database's form sets its user_version,
#   README.md) that database's copy goes back too, the job stopped meanwhile (what was written
#   between the copy and the restart, seconds, is lost then, and only then). A run cut short between
#   the switch and its end (the agent's job stopped, a power cut) leaves devtools/backup/pending (the
#   version it came from) beside the copies: the next run goes on from them. The two versions'
#   copies are kept, the others removed.
# - Caddy: this version's site file, its @name@, @cert@, @key@ and @https_port@ filled in from the
#   setup, put in the Caddyfile's sites/ as scenic.caddy; the whole Caddyfile checked (caddy
#   validate), Caddy reloaded (caddy reload, through its admin API) and checked (HTTPS answering
#   for the name within 10 s). A step failing, the site file last seen serving
#   (<home>/services/caddy/scenic.caddy.before; none, no site file) is put back and Caddy reloaded
#   again. Caddy itself, and the other projects' sites, are never touched.
# A service at this version, running (Caddy answering HTTPS), is left as it is (--reload: the dev
# tools' job loaded again from its plist, Caddy reloaded with the site file). The versions done are
# in <home>/services/version; one that failed in `failed`, which the agent doesn't try again (a run
# by hand does). One run at a time, setup's too (<home>/services/lock).
#
# SCENIC_SERVICES_SANDBOX=<dir> (tools/app/test/services.zsh, and a trial on the Mac mini): <dir>
# holds the home, the LaunchAgents' plists, the names file and the Caddyfile's default
# (<dir>/caddy/Caddyfile), the labels are local.scenic-sandbox.<dir's name>.<service>: nothing of
# the Mac's own services is touched.
set -euo pipefail
usage="usage: services.zsh setup devtools [--node <path>] [--data <folder>] [--port <n>]
       services.zsh setup caddy [--caddy <path>] [--config <Caddyfile>] [--name <name>] [--cert <file>] [--key <file>] [--https-port <n>]
       services.zsh update [--reload]"
bad() { print -ru2 -- "services.zsh: $*"$'\n'"$usage"; exit 2 }
typeset -A opt
cmd=${1:-} svc=
case $cmd in
  -h|--help) print -r -- $usage; exit 0 ;;
  setup)
    svc=${2:-}
    case $svc in
      devtools) allowed=(--node --data --port) ;;
      caddy) allowed=(--caddy --config --name --cert --key --https-port) ;;
      *) bad "setup: devtools or caddy, not ${svc:-nothing}" ;;
    esac
    shift 2
    while (( $# )); do
      (( ${allowed[(Ie)$1]} )) || bad "setup $svc: unexpected $1"
      (( $# >= 2 )) || bad "setup $svc: $1 needs a value"
      opt[$1]=$2; shift 2
    done ;;
  update)
    shift
    case $#:${1:-} in 0:|1:--reload) ;; *) bad "update: unexpected $*" ;; esac
    opt[reload]=${1:+all} ;;
  *) bad "unexpected ${cmd:-nothing}" ;;
esac

zmodload zsh/system zsh/datetime
from=${0:A:h}                 # this version's services/
version=${from:h:t}           # its app folder's name, the version
uid=$(id -u) domain=gui/$(id -u)
if [[ -n ${SCENIC_SERVICES_SANDBOX:-} ]]; then
  box=${SCENIC_SERVICES_SANDBOX:A}
  H=$box/services agents=$box/LaunchAgents names_file=$box/names prefix=local.scenic-sandbox.${box:t}
  logs=$box/logs caddy_conf=$box/caddy/Caddyfile
else
  H="$HOME/Library/Application Support/scenic/services" agents=$HOME/Library/LaunchAgents
  names_file=$HOME/.config/scenic/names prefix=local.scenic logs=$HOME/Library/Logs/scenic
  caddy_conf=$HOME/srv/caddy/Caddyfile
fi
mkdir -p $H $agents $logs
say() { print -r -- "services: $(strftime %T $EPOCHSECONDS) $*" }

# One run at a time, setup's included (its plist and settings written under the lock too).
touch $H/lock
zsystem flock -t 0 -f lockfd $H/lock 2>/dev/null || { say "another run holds $H/lock"; exit 1; }

# ---- launchd ---------------------------------------------------------------------------------------
loaded() { launchctl print $domain/$1 >/dev/null 2>&1 }
pid_of() { { launchctl print $domain/$1 2>/dev/null || true } | awk '$1 == "pid" && $2 == "=" { print $3; exit }' }
gone() {  # gone <label>: unloaded, waited for until launchd has let it go
  local i
  launchctl bootout $domain/$1 2>/dev/null || true
  for (( i = 0; i < 100; i++ )); do loaded $1 || return 0; sleep 0.1; done
  say "$1 still loaded after 10 s"; return 1
}
# reloads <service>: its jobs are to be loaded again from their plists (setup's of it, --reload's).
reloads() { [[ ${opt[reload]:-} == (all|$1) ]] }
# restart <service> <label> <plist>: the job running again from its plist: kickstarted (killed and
# started again) when loaded, else loaded; when the service reloads, unloaded and loaded again.
restart() {
  if reloads $1 && loaded $2; then gone $2; fi
  if loaded $2; then launchctl kickstart -k $domain/$2
  else launchctl bootstrap $domain $3; fi
}
# plist <label> <plist> <program> <arg>…: a LaunchAgent's plist written (to a file of its own, then
# moved in), started at load and kept alive, its log PLIST_LOG.
plist() {
  local label=$1 file=$2 i=0 tmp=$2.tmp; shift 2
  rm -f $tmp
  plutil -create xml1 $tmp
  plutil -insert Label -string $label $tmp
  plutil -insert ProgramArguments -array $tmp
  for a; do plutil -insert ProgramArguments.$i -string "$a" $tmp; (( ++i )); done
  plutil -insert RunAtLoad -bool true $tmp
  plutil -insert KeepAlive -bool true $tmp
  if (( ${+PLIST_ENV} )); then
    plutil -insert EnvironmentVariables -dictionary $tmp
    local k; for k in ${(k)PLIST_ENV}; do plutil -insert EnvironmentVariables.$k -string "${PLIST_ENV[$k]}" $tmp; done
  fi
  plutil -insert StandardOutPath -string $PLIST_LOG $tmp
  plutil -insert StandardErrorPath -string $PLIST_LOG $tmp
  mv -f $tmp $file
}
# The program and arguments a loaded job runs (one per line), and its working folder and an
# environment variable's value: what setup's defaults are read from.
job_args() { [[ -f $agents/$1.plist ]] && plutil -extract ProgramArguments json -o - $agents/$1.plist | python3 -c 'import json, sys; print("\n".join(json.load(sys.stdin)))' }
job_key() { [[ -f $agents/$1.plist ]] && plutil -extract $2 raw -o - $agents/$1.plist 2>/dev/null }

# ---- setup -----------------------------------------------------------------------------------------
# A setup file: the service's settings, as `name=value` lines zsh reads back (values quoted).
write_setup() {  # write_setup <service> <name>…: those variables' values
  local f=$H/$1/setup n; shift
  mkdir -p ${f:h}
  { print -r -- "# Written by services.zsh setup ${f:h:t} ($(strftime %Y-%m-%dT%H:%M:%S $EPOCHSECONDS), app $version)."
    for n; do print -r -- "$n=${(q)${(P)n}}"; done } > $f.tmp
  mv -f $f.tmp $f
}

if [[ $cmd == setup && $svc == devtools ]]; then
  label=$prefix.devtools
  old=("${(@f)$(job_args $label || true)}")
  node=${opt[--node]:-${old[1]:-}}
  [[ -n $node ]] || bad "setup devtools: no --node, and no LaunchAgent $label to take it from"
  case $node in */shims/*) bad "setup devtools: $node is a shim; give the real node" ;; esac
  [[ -x $node ]] || { say "no node at $node"; exit 1; }
  $node -e 'require("node:sqlite")' 2>/dev/null || { say "$node lacks node:sqlite (needs 22.5+)"; exit 1; }
  i=${old[(Ie)--data]}
  data=${opt[--data]:-${${i:#0}:+${old[i+1]}}}
  data=${data:-$(job_key $label WorkingDirectory || true)}
  [[ -n $data ]] || bad "setup devtools: no --data, and no LaunchAgent $label to take it from"
  [[ -d $data ]] || { say "no data folder $data"; exit 1; }
  data=${data:A}
  i=${old[(Ie)--port]}
  port=${opt[--port]:-${${${i:#0}:+${old[i+1]}}:-8077}}
  [[ $port == <1-65535> ]] || bad "setup devtools: --port $port isn't a port"
  plist=$agents/$label.plist
  write_setup devtools label plist node data port
  app="$H/devtools/current/tools/devtools"
  PLIST_LOG=$logs/devtools.log plist $label $plist $node "$app/server.mjs" --host :: --port $port --data $data
  say "devtools set up: $label (node $node, port $port, data $data)"
  opt[reload]=$svc cmd=update
elif [[ $cmd == setup && $svc == caddy ]]; then
  # (Its setup before, if any, gives the defaults.)
  caddy= config= name= cert= key= https_port=
  [[ -f $H/caddy/setup ]] && source $H/caddy/setup
  caddy=${opt[--caddy]:-${caddy:-$(command -v caddy || echo /opt/homebrew/bin/caddy)}}
  config=${opt[--config]:-${config:-$caddy_conf}}
  name=${opt[--name]:-${name:-$(grep -v '^#' $names_file 2>/dev/null | head -1 || true)}}
  cert=${opt[--cert]:-${cert:-$HOME/certs/map.crt}}
  key=${opt[--key]:-${key:-$HOME/certs/map.key}}
  https_port=${opt[--https-port]:-${https_port:-443}}
  [[ -x $caddy ]] || { say "no caddy at $caddy (brew install caddy)"; exit 1; }
  [[ -f $config ]] || { say "no Caddyfile at $config: the Mac's Caddy isn't set up (docs/macs.md, Services)"; exit 1; }
  config=${config:A}
  [[ -n $name ]] || bad "setup caddy: no --name, and none in $names_file"
  [[ -r $cert && -r $key ]] || { say "can't read $cert and $key"; exit 1; }
  [[ $https_port == <1-65535> ]] || bad "setup caddy: --https-port $https_port isn't a port"
  write_setup caddy caddy config name cert key https_port
  # (Its first line; the other names it held stay after it.)
  others=(${(f)"$(grep -v '^#' $names_file 2>/dev/null | grep -vxF -- $name || true)"})
  mkdir -p ${names_file:h}
  { print -r -- "# Written by services.zsh setup caddy: the names Caddy serves this Mac under."
    print -rl -- $name $others } > $names_file.tmp && mv -f $names_file.tmp $names_file
  say "caddy set up: https://$name:$https_port, the site in ${config:h}/sites ($caddy)"
  opt[reload]=$svc cmd=update
fi

# ---- update ----------------------------------------------------------------------------------------
failed=()

# The form of each database in the data folder: its user_version and schema_version, one line each.
forms() {
  local db
  for db in $1/*.db(N); do
    print -r -- "${db:t} $(/usr/bin/sqlite3 $db 'PRAGMA user_version; PRAGMA schema_version' | tr '\n' ' ')"
  done
}
answers() {  # answers <port>: the board's and the dashboard's APIs answer
  curl -sf --max-time 2 -o /dev/null "http://127.0.0.1:$1/api/c/items?limit=1" \
    && curl -sf --max-time 2 -o /dev/null http://127.0.0.1:$1/api/jobs/kinds
}
up() {  # up <label> <pid before> <tries, 0.2 s apart> <check…>: a new process of the job passes the check
  # (One that ended meanwhile fails it at once: a job kept alive doesn't end of its own accord.)
  local label=$1 was=$2 n=$3 i p seen=; shift 3
  for (( i = 0; i < n; i++ )); do
    p=$(pid_of $label || true)
    if [[ -n $p && $p != "$was" ]]; then
      seen=$p
      "$@" && return 0
    elif [[ -n $seen ]]; then
      return 1
    fi
    sleep 0.2
  done
  return 1
}
link() {  # link <folder> <target>: <folder>/current pointed at <target> (a new link renamed over)
  ln -sfh $2 $1/current.tmp && mv -fh $1/current.tmp $1/current
}

# (Each step's failure is checked where it's run: a function called under `||`, as each update is,
# runs with errexit off.)
update_devtools() {
  local label plist node data port
  source $H/devtools/setup
  local d=$H/devtools v=$version b=$H/devtools/backup cur prev was f0 f1 db
  cur=$(readlink $d/current 2>/dev/null || true)
  prev=$cur
  # A run cut short after its switch (the agent's job stopped, a power cut) left `pending`: the
  # version it switched from, its databases' copies and their forms in backup/ taken before it.
  # This run goes on from there: that version the one to go back to, those copies the ones to put
  # back.
  [[ -f $b/pending ]] && prev=$(<$b/pending)
  if ! reloads devtools && [[ -n $cur && -n $(pid_of $label) ]] && answers $port \
      && { [[ $cur == $v ]] || diff -rq $from/devtools $d/$cur >/dev/null 2>&1 }; then
    rm -f $b/pending
    [[ $cur == $v ]] && say "devtools: at $v already" || say "devtools: $v's are $cur's, running: left as they are"
    return 0
  fi
  if [[ ! -d $d/$v ]]; then
    rm -rf $d/$v.tmp && cp -RX $from/devtools $d/$v.tmp && mv $d/$v.tmp $d/$v \
      || { say "devtools: $v not copied to $d/$v"; rm -rf $d/$v.tmp; return 1 }
  fi
  if [[ ! -f $b/pending ]]; then
    # A copy of each database, consistent, while the old version runs: VACUUM INTO reads it in one
    # transaction (.backup starts again at each write by another process, and the board's are
    # written all the time), checked. Any failing, nothing is switched.
    mkdir -p $b && rm -f $b/*.db(N) $b/*.tmp(N) $b/forms || { say "devtools: can't clear $b"; return 1 }
    for db in $data/*.db(N); do
      if ! /usr/bin/sqlite3 $db "VACUUM INTO '$b/${db:t}.tmp'" \
          || [[ $(/usr/bin/sqlite3 $b/${db:t}.tmp 'PRAGMA quick_check') != ok ]] \
          || ! mv -f $b/${db:t}.tmp $b/${db:t}; then
        say "devtools: no good copy of ${db:t} in $b: $v not installed"; rm -f $b/*.tmp(N); return 1
      fi
    done
    forms $data > $b/forms.tmp && mv -f $b/forms.tmp $b/forms \
      && print -r -- $prev > $b/pending.tmp && mv -f $b/pending.tmp $b/pending \
      || { say "devtools: can't write $b's forms: $v not installed"; return 1 }
  fi
  f0=$(<$b/forms)
  was=$(pid_of $label || true)
  link $d $v && restart devtools $label $plist
  if up $label "$was" 150 answers $port; then
    say "devtools: $v answers on :$port (was ${prev:-none})"
    rm -f $b/pending
    for f in $d/<->-*(N/); do [[ ${f:t} == ($v|$prev) ]] || rm -rf $f; done
    return 0
  fi
  say "devtools: $v doesn't answer on :$port within 30 s: back to ${prev:-nothing}"
  f1=$(forms $data)
  if [[ -n $prev ]]; then link $d $prev; else rm -f $d/current; fi
  if [[ $f1 != $f0 ]]; then
    # (It changed a database's form: the copies back, with the job stopped.)
    gone $label || { say "devtools: $label won't stop: its databases not put back ($b/pending kept)"; return 1 }
    for db in $b/*.db(N); do
      [[ $(grep "^${db:t} " <<< $f0) == $(grep "^${db:t} " <<< $f1) ]] && continue
      if cp -X $db $data/${db:t}.restore && rm -f $data/${db:t}-wal $data/${db:t}-shm && mv -f $data/${db:t}.restore $data/${db:t}; then
        say "devtools: ${db:t} put back as it was before $v (its form had changed)"
      else
        say "devtools: ${db:t} NOT put back ($b/pending kept: the next run tries again)"; return 1
      fi
    done
  fi
  rm -f $b/pending
  if [[ -n $prev ]]; then
    was=$(pid_of $label || true)
    restart devtools $label $plist
    up $label "$was" 150 answers $port && say "devtools: $prev answers again" || say "devtools: $prev doesn't answer either"
  else
    gone $label  # (No version before it to run: stopped, not left failing.)
  fi
  return 1
}

update_caddy() {
  local caddy config name cert key https_port
  source $H/caddy/setup
  local d=$H/caddy t i code p
  local site=${config:h}/sites/scenic.caddy new=$d/scenic.caddy.new before=$d/scenic.caddy.before
  tls() { [[ $(curl -sk --max-time 2 -o /dev/null -w '%{http_code}' --resolve $name:$https_port:127.0.0.1 https://$name:$https_port/) != 000 ]] }
  serves() {  # Caddy reloaded with the Caddyfile as it is now, answering HTTPS within 10 s
    $caddy reload --config $config --adapter caddyfile > $d/reload.log 2>&1 || return 1
    for (( i = 0; i < 50; i++ )); do tls && return 0; sleep 0.2; done
    return 1
  }
  back() {  # the site file last seen serving put back (none, none), Caddy reloaded with it
    if [[ -f $before ]]; then mv -f $before $site; else rm -f $site; fi
    serves && say "caddy: the site before serves again" || say "caddy: the site before doesn't serve either"
  }
  # This version's site file, filled in from the setup.
  t=$(<$from/caddy/scenic.caddy) || { say "caddy: no site file in $from/caddy"; return 1 }
  t=${t//@name@/$name} t=${t//@cert@/$cert} t=${t//@key@/$key} t=${t//@https_port@/$https_port}
  print -r -- $t > $new || { say "caddy: can't write $new"; return 1 }
  if cmp -s $new $site && ! reloads caddy && tls; then
    rm -f $new $before
    say "caddy: at $version's site already"; return 0
  fi
  # (The one to go back to: the site file last seen serving. A .before is left only by a run cut
  # short, whose own site file wasn't checked: that one stays.)
  if [[ -f $site && ! -f $before ]]; then
    cp -X $site $before.tmp && mv -f $before.tmp $before || { say "caddy: can't keep the site before"; return 1 }
  fi
  mkdir -p ${site:h} && mv -f $new $site || { say "caddy: can't write $site"; return 1 }
  if ! $caddy validate --config $config --adapter caddyfile > $d/validate.log 2>&1; then
    say "caddy: $config doesn't validate with $version's site: $(tail -3 $d/validate.log)"
    back; return 1
  fi
  if ! serves; then
    say "caddy: no HTTPS on :$https_port within 10 s of a reload with $version's site ($(tail -1 $d/reload.log)): the one before back"
    back; return 1
  fi
  rm -f $before
  code=()
  for p in / /build/ /dev/; do
    code+=("$p $(curl -sk --max-time 5 -o /dev/null -w '%{http_code}' --resolve $name:$https_port:127.0.0.1 https://$name:$https_port$p)")
  done
  say "caddy: $version's site serves https://$name:$https_port (${(j:, :)code})"
}

any=
for s in devtools caddy; do
  [[ -f $H/$s/setup ]] || continue
  any=1
  update_$s || failed+=($s)
done
[[ -n $any ]] || { say "nothing set up in $H"; exit 1; }
if (( $#failed )); then
  print -r -- $version > $H/failed
  say "FAILED at $version: ${(j:, :)failed}"
  exit 1
fi
print -r -- $version > $H/version
rm -f $H/failed
say "at $version"
