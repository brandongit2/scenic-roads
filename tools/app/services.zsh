#!/bin/zsh
# The services the Mac mini runs beside the app (docs/macs.md, Services): the dev tools app
# (tools/devtools: the task board and the dev job dashboard, on :8077, and its nightly backup to the
# NAS) and Caddy (tools/caddy: the map's name on the tailnet). Every release ships them in the app's
# services/ folder (tools/app/lib.zsh's services_stage: this script, caddy/Caddyfile, and devtools/
# laid out as in the repo), and the Mac's agent keeps them at the version it runs
# (agentcore::agent::services): once its coordinator listens on this Mac's port, it runs this
# version's `services.zsh update`, so Caddy's routes change only once the coordinator serving them
# is up, and only on a Mac where a service was set up.
#
#   services.zsh setup devtools [--node <path>] [--data <folder>] [--port <n>]
#   services.zsh setup caddy [--caddy <path>] [--name <name>] [--cert <file>] [--key <file>]
#                            [--https-port <n>]
#   services.zsh update [--reload]
#
# setup (once per service, by hand, from the installed app's copy: ~/Library/Application
# Support/scenic/app/current/services/services.zsh) writes the service's settings
# (<home>/services/<service>/setup) and its LaunchAgent, which runs it from <home>/services/, then
# runs update, that service's jobs loaded again from their new plists. Its defaults are what that
# LaunchAgent runs now, if anything: the dev tools' node and data folder (`--data`, else its
# working folder: on the Mac mini, the git checkout's tools/devtools, where the board's data is);
# Caddy's program, name (~/.config/scenic/names), certificate and key (else `caddy` on the PATH,
# ~/certs/map.crt and map.key). Caddy's setup also writes the name to ~/.config/scenic/names, which
# the map server and the agent read (coordwire::net::names).
#
# update, for each service set up, the dev tools first:
# - the dev tools: this version's copy put in <home>/services/devtools/<version>/, `current`
#   pointed at it, the job restarted (launchctl kickstart -k: it stays loaded, so a run stopped part
#   way leaves launchd running one or the other) and checked: a new process answering the board's
#   and the dashboard's APIs within 30 s. Their databases are copied first (SQLite's .backup, while
#   the old one runs: no stop for it) to devtools/backup/. A version that doesn't answer is put
#   back, and when it changed a database's form (its user_version or schema_version: a change of a
#   database's form sets its user_version, README.md) that database's copy goes back too, the job
#   stopped meanwhile (what was written between the copy and the restart, a few seconds, is lost
#   then, and only then). The two versions' copies are kept, the others removed.
# - Caddy: this version's Caddyfile checked (caddy validate, with the job's names), put in
#   <home>/services/caddy/Caddyfile, Caddy restarted and checked (a new process answering HTTPS on
#   its port within 10 s); else the one before put back.
# A service already at this version, running, is left as it is (--reload: every service's jobs
# loaded again from their plists, as setup does for its own). The versions
# done are in <home>/services/version; one that failed in `failed`, which the agent doesn't try
# again (a run by hand does). One run at a time (<home>/services/lock).
#
# SCENIC_SERVICES_SANDBOX=<dir> (tools/app/test/services.zsh, and a trial on the Mac mini): <dir>
# holds the home, the LaunchAgents' plists and the names file, the labels are
# local.scenic-sandbox.<dir's name>.<service>: nothing of the Mac's own services is touched.
set -euo pipefail
usage="usage: services.zsh setup devtools [--node <path>] [--data <folder>] [--port <n>]
       services.zsh setup caddy [--caddy <path>] [--name <name>] [--cert <file>] [--key <file>] [--https-port <n>]
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
      caddy) allowed=(--caddy --name --cert --key --https-port) ;;
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
  logs=$box/logs
else
  H="$HOME/Library/Application Support/scenic/services" agents=$HOME/Library/LaunchAgents
  names_file=$HOME/.config/scenic/names prefix=local.scenic logs=$HOME/Library/Logs/scenic
fi
mkdir -p $H $agents $logs
say() { print -r -- "services: $(strftime %T $EPOCHSECONDS) $*" }

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
# moved in), started at load and kept alive unless PLIST_CALENDAR names a time (hour:minute), its
# log PLIST_LOG.
plist() {
  local label=$1 file=$2 i=0 tmp=$2.tmp; shift 2
  rm -f $tmp
  plutil -create xml1 $tmp
  plutil -insert Label -string $label $tmp
  plutil -insert ProgramArguments -array $tmp
  for a; do plutil -insert ProgramArguments.$i -string "$a" $tmp; (( ++i )); done
  if [[ -n ${PLIST_CALENDAR:-} ]]; then
    plutil -insert StartCalendarInterval -dictionary $tmp
    plutil -insert StartCalendarInterval.Hour -integer ${PLIST_CALENDAR%%:*} $tmp
    plutil -insert StartCalendarInterval.Minute -integer ${PLIST_CALENDAR#*:} $tmp
  else
    plutil -insert RunAtLoad -bool true $tmp
    plutil -insert KeepAlive -bool true $tmp
  fi
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
  label=$prefix.devtools backup_label=$prefix.devtools-backup
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
  plist=$agents/$label.plist backup_plist=$agents/$backup_label.plist
  write_setup devtools label plist backup_label backup_plist node data port
  app="$H/devtools/current/tools/devtools"
  PLIST_LOG=$logs/devtools.log plist $label $plist $node "$app/server.mjs" --host :: --port $port --data $data
  PLIST_LOG=$logs/devtools.log PLIST_CALENDAR=3:30 plist $backup_label $backup_plist "$app/backup.sh" $data
  say "devtools set up: $label (node $node, port $port, data $data), $backup_label (03:30)"
  opt[reload]=$svc cmd=update
elif [[ $cmd == setup && $svc == caddy ]]; then
  label=$prefix.caddy
  typeset -A was
  for k in SCENIC_MAP_NAME SCENIC_CERT SCENIC_KEY SCENIC_HTTPS_PORT; do was[$k]=$(job_key $label EnvironmentVariables.$k || true); done
  old=("${(@f)$(job_args $label || true)}")
  caddy=${opt[--caddy]:-${old[1]:-$(command -v caddy || echo /opt/homebrew/bin/caddy)}}
  name=${opt[--name]:-${was[SCENIC_MAP_NAME]:-$(grep -v '^#' $names_file 2>/dev/null | head -1 || true)}}
  cert=${opt[--cert]:-${was[SCENIC_CERT]:-$HOME/certs/map.crt}}
  key=${opt[--key]:-${was[SCENIC_KEY]:-$HOME/certs/map.key}}
  https_port=${opt[--https-port]:-${was[SCENIC_HTTPS_PORT]:-443}}
  [[ -x $caddy ]] || { say "no caddy at $caddy (brew install caddy)"; exit 1; }
  [[ -n $name ]] || bad "setup caddy: no --name, and none in $names_file"
  [[ -r $cert && -r $key ]] || { say "can't read $cert and $key"; exit 1; }
  [[ $https_port == <1-65535> ]] || bad "setup caddy: --https-port $https_port isn't a port"
  plist=$agents/$label.plist
  write_setup caddy label plist caddy name cert key https_port
  typeset -A PLIST_ENV=(SCENIC_MAP_NAME $name SCENIC_CERT $cert SCENIC_KEY $key SCENIC_HTTPS_PORT $https_port)
  PLIST_LOG=$logs/caddy.log plist $label $plist $caddy run --config "$H/caddy/Caddyfile" --adapter caddyfile
  unset PLIST_ENV
  mkdir -p ${names_file:h}
  printf '# Written by services.zsh setup caddy: the names Caddy serves this Mac under.\n%s\n' $name > $names_file
  say "caddy set up: $label (https://$name:$https_port, $caddy)"
  opt[reload]=$svc cmd=update
fi

# ---- update ----------------------------------------------------------------------------------------
touch $H/lock
zsystem flock -t 0 -f lockfd $H/lock 2>/dev/null || { say "another run holds $H/lock"; exit 1; }
failed=()

# The form of each database in the data folder: its user_version and schema_version, one line each.
forms() {
  local db
  for db in $1/*.db(N); do
    print -r -- "${db:t} $(/usr/bin/sqlite3 $db 'PRAGMA user_version; PRAGMA schema_version' | tr '\n' ' ')"
  done
}
answers() {  # answers <port>: the board's and the dashboard's APIs answer
  curl -sf --max-time 2 -o /dev/null http://127.0.0.1:$1/api/c/meta/status \
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

update_devtools() {
  local label plist backup_label backup_plist node data port
  source $H/devtools/setup
  local d=$H/devtools v=$version prev was f0 f1 db
  prev=$(readlink $d/current 2>/dev/null || true)
  if [[ ! -d $d/$v ]]; then
    rm -rf $d/$v.tmp; cp -RX $from/devtools $d/$v.tmp && mv $d/$v.tmp $d/$v
  fi
  if [[ $prev == $v ]] && ! reloads devtools && [[ -n $(pid_of $label) ]] && answers $port; then
    say "devtools: at $v already"; return 0
  fi
  # (A copy of each database, consistent, while the old version runs.)
  mkdir -p $d/backup
  for db in $data/*.db(N); do
    /usr/bin/sqlite3 $db ".backup '$d/backup/${db:t}.tmp'" && mv -f $d/backup/${db:t}.tmp $d/backup/${db:t}
  done
  f0=$(forms $data)
  was=$(pid_of $label || true)
  link $d $v
  restart devtools $label $plist
  if up $label "$was" 150 answers $port; then
    say "devtools: $v answers on :$port (was ${prev:-none})"
    loaded $backup_label && ! reloads devtools || restart devtools $backup_label $backup_plist
    for f in $d/<->-*(N/); do [[ ${f:t} == ($v|$prev) ]] || rm -rf $f; done
    return 0
  fi
  say "devtools: $v doesn't answer on :$port within 30 s: back to ${prev:-nothing}"
  f1=$(forms $data)
  if [[ -n $prev ]]; then link $d $prev; else rm -f $d/current; fi
  if [[ $f1 != $f0 ]]; then
    # (It changed a database's form: the copies back, with the job stopped.)
    gone $label
    for db in $d/backup/*.db(N); do
      [[ $(grep "^${db:t} " <<< $f0) == $(grep "^${db:t} " <<< $f1) ]] && continue
      cp -X $db $data/${db:t}.restore && rm -f $data/${db:t}-wal $data/${db:t}-shm && mv -f $data/${db:t}.restore $data/${db:t}
      say "devtools: ${db:t} put back as it was before $v (its form had changed)"
    done
    [[ -n $prev ]] && launchctl bootstrap $domain $plist
  elif [[ -n $prev ]]; then
    was=$(pid_of $label || true)
    launchctl kickstart -k $domain/$label
  fi
  if [[ -n $prev ]]; then
    up $label "$was" 150 answers $port && say "devtools: $prev answers again" || say "devtools: $prev doesn't answer either"
  fi
  return 1
}

update_caddy() {
  local label plist caddy name cert key https_port
  source $H/caddy/setup
  local d=$H/caddy new=$from/caddy/Caddyfile was code p
  check_env() { SCENIC_MAP_NAME=$name SCENIC_CERT=$cert SCENIC_KEY=$key SCENIC_HTTPS_PORT=$https_port "$@" }
  if ! check_env $caddy validate --config $new --adapter caddyfile > $d/validate.log 2>&1; then
    say "caddy: $version's Caddyfile doesn't validate: $(tail -3 $d/validate.log)"; return 1
  fi
  if cmp -s $new $d/Caddyfile && ! reloads caddy && [[ -n $(pid_of $label) ]]; then
    say "caddy: at $version's Caddyfile already"; return 0
  fi
  tls() { [[ $(curl -sk --max-time 2 -o /dev/null -w '%{http_code}' --resolve $name:$https_port:127.0.0.1 https://$name:$https_port/) != 000 ]] }
  [[ -f $d/Caddyfile ]] && cp -X $d/Caddyfile $d/Caddyfile.before
  cp -X $new $d/Caddyfile.tmp && mv -f $d/Caddyfile.tmp $d/Caddyfile
  was=$(pid_of $label || true)
  restart caddy $label $plist
  if up $label "$was" 50 tls; then
    code=()
    for p in / /build/ /dev/; do
      code+=("$p $(curl -sk --max-time 5 -o /dev/null -w '%{http_code}' --resolve $name:$https_port:127.0.0.1 https://$name:$https_port$p)")
    done
    say "caddy: $version's Caddyfile serves https://$name:$https_port (${(j:, :)code})"
    return 0
  fi
  say "caddy: no HTTPS on :$https_port within 10 s with $version's Caddyfile: the one before back"
  if [[ -f $d/Caddyfile.before ]]; then
    mv -f $d/Caddyfile.before $d/Caddyfile
    was=$(pid_of $label || true)
    launchctl kickstart -k $domain/$label
    up $label "$was" 50 tls && say "caddy: the Caddyfile before serves again" || say "caddy: the Caddyfile before doesn't serve either"
  fi
  return 1
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
