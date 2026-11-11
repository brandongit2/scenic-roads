# The shell steps publish.sh runs, as functions it sources, so each can be tested in a stand-in tree
# (tools/app/test/shell.zsh, run by tools/dev/check.sh when tools/app changes); install.sh and
# remote-tests.sh (on both of its sides) source it too.

# The app's programs, built by publish.sh (target/release) and installed beside each other in the
# NAS's app/<version>/ (install.sh copies them to each Mac).
app_programs=(server scenic scenic-build)

# proc_id [<pid>]: a process's owner id, "<pid> <its start time>" (ps's lstart, as ps prints it): a
# lock names its holder so, as the PID alone may be another process's later. This process by default.
# proc_alive <id>: 0 while the process an owner id names still runs (its PID, started then).
proc_id() { local p=${1:-$$}; print -r -- "$p $(ps -o lstart= -p $p 2>/dev/null)"; }
proc_alive() { kill -0 ${1%% *} 2>/dev/null && [[ $(proc_id ${1%% *}) == "$1" ]]; }

# stop_program <executable>: a TERM to each process running exactly <executable> (its whole path, as
# ps names it), by its PID: never by a pattern (`pkill -f` also matches a process whose arguments
# merely name it). None running is no failure.
stop_program() {
  local pid exe
  ps -axo pid=,comm= | while read -r pid exe; do
    if [[ $exe == "$1" ]]; then kill $pid 2>/dev/null || true; fi
  done
}

# at_end <function>: runs <function> once as this script ends, however it ends: an exit, a command
# failing under errexit (inside a function too), a failed subshell, or INT, TERM or HUP (exit 130).
# Inside it, $? is the script's exit status; it runs with errexit off, and the script exits with
# its status. (zsh 5.9 runs no EXIT trap, TRAPEXIT nor zshexit when errexit stops the script inside
# a function: it exits from there at once. The ZERR trap, which runs just before that exit, still
# does; in a subshell it does nothing, the subshell's failure stopping the script as it returns.
# The EXIT trap is set here, as this file is sourced: one set inside a function runs as that
# function returns.)
at_end() {
  typeset -g _at_end_fn=$1 _at_end_ran=
  trap _at_end ZERR
  trap 'exit 130' INT TERM HUP
}
trap _at_end EXIT
_at_end() {
  local rc=$?
  (( ZSH_SUBSHELL )) || [[ -z ${_at_end_fn:-} || -n $_at_end_ran ]] && return $rc
  _at_end_ran=1
  setopt localoptions noerrexit
  trap - ZERR
  ( exit $rc ); $_at_end_fn
  exit $?
}

# found <find arguments...>: find's output, never failing: a start folder that is missing or
# unreadable is nothing found, not a failure that errexit would end the script on.
found() { find "$@" 2>/dev/null || true; }

# prune_target [<target folder>]: of each profile's deps/, build/, incremental/ and .fingerprint/
# (target/<profile>, and target/<triple>/release of the WebAssembly builds), an entry not read nor
# written for 2 days goes: files only by their own times (a folder's access time changes when it is
# listed, by this sweep too), a folder by its newest file's (macOS updates a file's access time
# when it is read, at most once a day). Prints what it removed and what is left.
prune_target() {
  local t=${1:-target} p e stale=() kb=0
  for p in $t/*/(N/) $t/*-*/release(N/); do
    stale+=(${(f)"$(found $p/deps -mindepth 1 -maxdepth 1 -type f ! -newerat '2 days ago' ! -newermt '2 days ago')"})
    for e in $p/{build,incremental,.fingerprint}/*(ND/); do
      if [[ -z $(found $e -type f \( -newerat '2 days ago' -o -newermt '2 days ago' \) -print -quit) ]]; then stale+=($e); fi
    done
  done
  if (( $#stale )); then
    kb=$(print -rN -- $stale | xargs -0 du -sk 2>/dev/null | awk '{s+=$1} END {print s+0}')
    print -rN -- $stale | xargs -0 rm -rf
  fi
  print -r -- "build folder: $(( kb / 1024 )) MB of old builds removed, $(du -sh $t 2>/dev/null | cut -f1) left"
}

# nas_lock <lock> <who>: takes publish.sh's lock on the NAS (app/publish.lock), made with create-new
# (zsh's noclobber opens with O_EXCL, which the share does atomically) and holding <who> (its Mac,
# process and time). 0 when taken; 1 when another holds it, its holder and age then in $lock_held.
# One older than six hours (its Mac gone mid-publish), or held by a process of this Mac that is gone
# (a publish killed here mid-install), is moved aside, checked to be the one read (one another
# publish took meanwhile is put back), and taken.
nas_lock() {
  local lock=$1 who=$2 held age hp
  typeset -g lock_held=
  ( set -o noclobber; print -r -- "$who" > $lock ) 2>/dev/null && return 0
  held=$(cat $lock 2>/dev/null)
  age=$(( $(date +%s) - $(stat -f %m $lock 2>/dev/null || date +%s) ))
  hp=${${held#* }%% *}
  if (( age < 6 * 3600 )) && ! { [[ -n $held && ${held%% *} == ${who%% *} && $hp == <-> ]] && ! kill -0 $hp 2>/dev/null; }; then
    lock_held="$held, $(( age / 60 )) min ago"; return 1
  fi
  mv $lock $lock.stale-$$ 2>/dev/null || { lock_held="taken just now"; return 1; }
  if [[ $(cat $lock.stale-$$ 2>/dev/null) != $held ]]; then
    ( set -o noclobber; cat $lock.stale-$$ > $lock ) 2>/dev/null || true  # (one made meanwhile kept)
    rm -f $lock.stale-$$; lock_held="taken just now"; return 1
  fi
  rm -f $lock.stale-$$
  ( set -o noclobber; print -r -- "$who" > $lock ) 2>/dev/null || { lock_held="taken just now"; return 1; }
  print -r -- "publish: took over a lock $(( age / 3600 )) h old ($held)"
}

# nas_unlock <lock> <who>: removes the lock only while it is still <who>'s.
nas_unlock() { [[ $(cat $1 2>/dev/null) == "$2" ]] && rm -f $1; return 0; }

# hold <dir>: a lock of this Mac's (mkdir <dir>, its owner file naming this process and its start
# time), for a publish's build tree and its test run. 0 taken; 1 held by a live process, named in
# $lock_held. One whose process is gone (or is another under a reused PID), or whose owner was never
# written (10 s on), is moved aside, checked unchanged, and taken.
# unhold <dir>: removes it while it is still this process's.
hold() {
  local d=$1 me=$(proc_id) held
  typeset -g lock_held=
  until mkdir $d 2>/dev/null; do
    held=$(cat $d/owner 2>/dev/null)
    if [[ -n $held ]]; then
      proc_alive $held && { lock_held="process $held"; return 1; }
    elif [[ -n $(found $d -maxdepth 0 -newermt '10 seconds ago') ]]; then lock_held="being taken"; return 1
    fi
    mv $d $d.aside-$$ 2>/dev/null || continue
    if [[ $(cat $d.aside-$$/owner 2>/dev/null) != "$held" ]]; then
      # (Another's, taken meanwhile: put back unless a third has made one since, a move onto which
      # would nest inside it.)
      [[ -e $d ]] || mv $d.aside-$$ $d 2>/dev/null
      rm -rf $d.aside-$$; lock_held="taken just now"; return 1
    fi
    rm -rf $d.aside-$$
  done
  print -r -- "$me" > $d/owner
}
unhold() { [[ $(cat $1/owner 2>/dev/null) == "$(proc_id)" ]] && rm -rf $1; return 0; }

# claim_tree <cache>: takes the first free of <cache>/tree and tree-2 (hold, above) into $snap,
# waiting while both are held: says so once, naming both holders, and polls every $tree_poll
# seconds (2). A holder that died frees its tree (hold's stale rule), so the wait ends. Its unholds
# are put on $cleanup first, so a TERM at any point leaves nothing held (unhold frees only this
# process's own).
claim_tree() {
  local cache=$1 t said= who=
  typeset -g snap=
  cleanup+=("unhold $cache/tree.busy" "unhold $cache/tree-2.busy")
  while :; do
    who=
    for t in tree tree-2; do
      if hold $cache/$t.busy; then snap=$cache/$t; return 0; fi
      who+="${who:+; }$t: $lock_held"
    done
    [[ -n $said ]] || { echo "two publishes are already running on this Mac ($who): waiting for one to end"; said=1; }
    sleep ${tree_poll:-2}
  done
}

# fast_forward <current.json> <commit> [<live version when the publish began>]: 0 when nothing is
# live, or the live version's commit (its name's last part) is <commit> or an ancestor of it; else
# says why and returns 1: a publish never puts an older tree, or one without everything published,
# over the live one. A live commit this repository lacks (published from another Mac after this
# one's fetch) is fetched once. With the third argument: also refused when the live version changed
# since then by a rollback (previous.json's commit not in the live one's), so a publish begun before
# a rollback doesn't put back what was rolled back. Run in the repository.
fast_forward() {
  [[ -f $1 ]] || return 0
  local v live pv
  v=$(live_version $1) || { print -r -- "can't read $1"; return 1; }
  live=${v##*-}
  if ! git cat-file -e "$live^{commit}" 2>/dev/null; then
    git fetch -q origin main 2>/dev/null
    if ! git cat-file -e "$live^{commit}" 2>/dev/null; then print -r -- "the live version $v's commit $live isn't in this repository"; return 1; fi
  fi
  if ! git merge-base --is-ancestor $live $2; then
    if git merge-base --is-ancestor $2 $live; then print -r -- "$2 is older than the live version $v: refused"
    else print -r -- "$2 doesn't contain the live version $v (diverged): refused"; fi
    return 1
  fi
  if [[ -n ${3:-} && $v != $3 && -f ${1:h}/previous.json ]]; then
    pv=$(live_version ${1:h}/previous.json) || pv=
    pv=${pv##*-}
    if [[ -n $pv ]] && ! git merge-base --is-ancestor $pv $live 2>/dev/null; then
      print -r -- "rolled back to $v since this publish began: refused (publish again to go past it)"; return 1
    fi
  fi
  return 0
}
# rollback_ok <current.json> <previous.json>: 0 when the previous version's tree has the live one's
# rollback floor (the first line of tools/app/rollback-floor; none on either side counts as one
# floor); else says why and returns 1: a rollback never goes back past a release after which an
# older app is unsafe. A commit this repository lacks is fetched once; still missing, refused. Run in
# the repository.
rollback_ok() {
  local v pv c f=tools/app/rollback-floor live prev
  v=$(live_version $1) || { print -r -- "can't read $1"; return 1; }
  pv=$(live_version $2) || { print -r -- "can't read $2"; return 1; }
  for c in ${v##*-} ${pv##*-}; do
    git cat-file -e "$c^{commit}" 2>/dev/null || git fetch -q origin main 2>/dev/null
    git cat-file -e "$c^{commit}" 2>/dev/null || { print -r -- "the version's commit $c isn't in this repository: refused"; return 1; }
  done
  live=$(git show "${v##*-}:$f" 2>/dev/null | head -1)
  prev=$(git show "${pv##*-}:$f" 2>/dev/null | head -1)
  if [[ $live != $prev ]]; then
    print -r -- "$pv is before the live version $v's rollback floor ${live:-(none)} (its ${prev:-none}): refused. Why, from $f:"
    git show "${v##*-}:$f" 2>/dev/null | sed -n 's/^# \{0,1\}//p'
    return 1
  fi
  return 0
}
# live_version <current.json>: its version's name.
live_version() { python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' $1 2>/dev/null; }

# bounded <seconds> <command...>: the command, stopped (by its PID) past <seconds>, then 124.
bounded() {
  "${@:2}" <&0 &  # (its stdin this function's, not the /dev/null a background job gets)
  local p=$! w rc
  # (The watchdog's sleep is its own child, stopped with it: no sleep left behind.)
  ( trap 'kill $s 2>/dev/null; exit 1' TERM; sleep $1 & s=$!; wait $s && kill $p ) </dev/null >/dev/null 2>&1 &
  w=$!
  wait $p; rc=$?
  kill $w 2>/dev/null; wait $w && rc=124  # (the watchdog exits 0 only once it fired, maybe still exiting)
  return $rc
}

# The NAS's Mac (#1147): the Mac mini, on the NAS's LAN. publish.sh reads and writes the NAS's app/
# only there, whichever Mac publishes, so a publish from away sends the app in one stream rather than
# writing its files through its own mount (the NAS's Tailscale, ~1 MB/s, and stale after a network
# change). Known by its LocalHostName ($nas_mac), so on it they run here, with no ssh to itself;
# reached from another Mac by ssh ($nas_host, an alias), in its checkout ($nas_repo, from its home),
# whose git checks the order.
nas_mac=Mac-mini-de-Brandon nas_host=m6 nas_repo=projets/scenic-roads
_lib=${${(%):-%x}:A}

# on_nas_mac <command...>: runs <command> (a function of this file's, or any command) on the NAS's
# Mac, in its checkout, under errexit, this file sourced; its stdin and output this side's. Over ssh
# a fresh connection (no shared one) with its own keepalives: a dead link ends it within a minute.
# Its status is the command's (255: ssh's own failure).
on_nas_mac() {
  local script="set -euo pipefail"$'\n'"$(<$_lib)"$'\n'"cd ~ && cd ${(q)nas_repo}"$'\n'"${(j: :)${(q)@}}"
  if [[ $(scutil --get LocalHostName 2>/dev/null) == $nas_mac ]]; then zsh -c $script
  else
    ssh -o BatchMode=yes -o ConnectTimeout=10 -o ControlPath=none -o ServerAliveInterval=15 \
      -o ServerAliveCountMax=4 -o LogLevel=ERROR $nas_host "zsh -c ${(qqqq)script}"
  fi
}

# nas_current <nas>: app/current.json's content, nothing when no version is live; 2 when the NAS
# isn't mounted. (On the NAS's Mac.)
nas_current() {
  [[ -d $1/app ]] || { print -ru2 -- "the NAS isn't mounted at $1 on $(scutil --get LocalHostName 2>/dev/null)"; return 2; }
  [[ ! -f $1/app/current.json ]] || cat $1/app/current.json
}

# nas_fonts <nas>: the NAS's app/fonts, as a tar on stdout. (On the NAS's Mac.)
nas_fonts() {
  [[ -d $1/app/fonts ]] || { print -ru2 -- "no fonts folder at $1/app/fonts"; return 1; }
  COPYFILE_DISABLE=1 tar -c --no-xattrs --no-acls --no-fflags --no-mac-metadata -C $1/app/fonts .
}

# app_files <folder>: the app's files in <folder>, sorted, .DS_Store and AppleDouble files left out.
app_files() { (cd $1 && find . -type f ! -name .DS_Store ! -name '._*' | sed 's|^\./||' | sort); }
# app_manifest <folder> <version>: app/current.json's content for the app in <folder>: its version,
# and every file (app_files) with its SHA-256, which every Mac's
# updater checks its copy against.
app_manifest() {
  app_files $1 | (cd $1 && python3 -c "
import hashlib, json, sys
files = [l.strip() for l in sys.stdin if l.strip()]
sha = {f: hashlib.sha256(open(f, 'rb').read()).hexdigest() for f in files}
print(json.dumps({'version': sys.argv[1], 'files': files, 'sha256': sha}))" "$2")
}

# nas_send <nas> <version> <commit> <live version when the publish began> <archive> <from>: publish.sh's
# install, the publishing Mac's side. <archive> (a gzipped tar of app/, the version's folder;
# manifest.json, its app_manifest; fetch-planet.sh) goes in one stream to the NAS's Mac, whose
# nas_install (below) writes it to the NAS; its output is shown, its markers read: once it says the
# version is ready, the go-ahead is sent. In $nas_state: "" when nothing went live, "live", or
# "unknown" (the go-ahead sent, then no answer, nor from app/current.json read again). Returns 0
# when live, 3 when unknown, else 1.
nas_send() {
  local nas=$1 version=$2 bytes sum t0 t1 l rc=0 cp
  typeset -g nas_state=
  zmodload zsh/datetime; setopt localoptions localtraps nomultios
  trap '' PIPE  # (the other side gone: a write here fails, it doesn't end the publish unannounced)
  bytes=$(stat -f %z $5); sum=$(shasum -a 256 $5); sum=${sum%% *}
  # (A fresh connection within 60 s, as the stream's own: its key from the agent, 1Password, which
  # may wait for an approval.)
  if ! bounded 60 on_nas_mac true </dev/null; then
    print -r -- "can't reach the NAS's Mac ($nas_host) within 60 s: nothing sent"; return 1
  fi
  t0=$EPOCHREALTIME
  coproc on_nas_mac nas_install $nas $version $3 "$4" $bytes $sum "$6" 2>&1
  cp=$!
  cat $5 >&p 2>/dev/null || true  # (a side that ended early says why below)
  while IFS= read -rp l; do
    case $l in
      @@received) t1=$EPOCHREALTIME
        printf 'sent %.1f MB to %s in %.1f s (%.2f MB/s)\n' $(( bytes / 1e6 )) $nas_host $(( t1 - t0 )) $(( bytes / 1e6 / (t1 - t0) )) ;;
      @@ready) print -p go 2>/dev/null && nas_state=go ;;
      "@@installed $version") nas_state=live ;;
      *) print -r -- "$l" ;;
    esac
  done
  wait $cp 2>/dev/null || rc=$?
  [[ $nas_state == live ]] && return 0
  [[ $nas_state == go ]] || { print -r -- "the NAS's Mac installed nothing (exit $rc)"; return 1; }
  # (The go-ahead sent, the answer lost: the NAS's Mac goes on with it, whatever the connection.)
  if [[ $(bounded 60 on_nas_mac nas_current $nas </dev/null 2>/dev/null | live_version /dev/stdin) == $version ]]; then
    nas_state=live; return 0
  fi
  nas_state=unknown; return 3
}

# nas_install <nas> <version> <commit> <began> <bytes> <sha256> <from>: publish.sh's install, the
# NAS's Mac's side (nas_send above, the other). The archive read from stdin whole into a local file
# (~/Library/Caches/scenic-publish/incoming/<pid>.tgz) and checked by its size and SHA-256 (an
# upload cut short installs nothing), then unpacked beside it (<pid>/) and checked against its
# manifest; the order checked (fast_forward); the app written beside its folder on the NAS
# (app/<version>.tmp) and its files listed back; then under the NAS's lock (nas_lock, waited for
# while another publish holds it) the order checked again, "@@ready", and the go-ahead read (within
# $nas_go_wait s, 300), else nothing goes in; then, whatever the connection and the signals,
# app/<version>/, nas/fetch-planet.sh when it differs, previous.json and current.json, in that
# order, and "@@installed <version>".
nas_install() {
  local nas=$1 version=$2 head=$3 began=$4 bytes=$5 sum=$6 from=$7
  local inc=~/Library/Caches/scenic-publish/incoming lock=$1/app/publish.lock dest=$1/app/$2
  local who="$(scutil --get LocalHostName 2>/dev/null || hostname -s) $$ $(date -u +%Y-%m-%dT%H:%M:%SZ) for $from"
  local in=$inc/$$.tgz loc=$inc/$$ f got go said=
  _nas_begin
  [[ -d $nas/app ]] || { print -r -- "the NAS isn't mounted at $nas on ${who%% *}"; return 2; }
  for f in $inc/*(N); do kill -0 ${${f:t}%%.*} 2>/dev/null || rm -rf $f; done  # (gone receivers')
  _nas_cleanup+=("rm -rf ${(q)in} ${(q)loc}")
  head -c $bytes > $in
  got=$(stat -f %z $in)
  (( got == bytes )) || { print -r -- "the upload ended short ($got of $bytes bytes): nothing installed"; return 1; }
  [[ $(shasum -a 256 $in) == "$sum "* ]] || { print -r -- "the upload's SHA-256 isn't the one sent: nothing installed"; return 1; }
  print -r -- @@received
  # (The manifest checked on this Mac's disk, not by reading the NAS's copy back: on 11 Nov that
  # read took up to 89 s, the write 5-9 s.)
  mkdir $loc && tar -xzf $in -C $loc
  [[ $(app_manifest $loc/app $version) == "$(<$loc/manifest.json)" ]] \
    || { print -r -- "the app sent isn't its manifest's: nothing installed"; return 1; }
  git cat-file -e "$head^{commit}" 2>/dev/null || git fetch -q origin main 2>/dev/null || true
  git cat-file -e "$head^{commit}" 2>/dev/null || { print -r -- "$head isn't in ${who%% *}'s $PWD, nor its origin's main"; return 1; }
  # (Checked first unlocked, so a publish that can't go in writes nothing; again under the lock.)
  fast_forward $nas/app/current.json $head "$began" || return 1
  [[ ! -e $dest ]] || { print -r -- "$dest exists already"; return 1; }
  mkdir $dest.tmp || { print -r -- "$dest.tmp exists already (another install of this version)"; return 1; }
  _nas_cleanup+=("rm -rf ${(q)dest}.tmp")
  # (-m: no file's time set, a round trip a file on the share: the write took 3-7 times as long
  # with them on 11 Nov; the Macs' copies don't keep them.)
  tar -xzmf $in -C $dest.tmp --strip-components 1 --no-same-owner --no-xattrs --no-acls --no-fflags --no-mac-metadata app
  [[ $(app_files $dest.tmp) == "$(app_files $loc/app)" ]] \
    || { print -r -- "the app written to the NAS isn't the one sent (its files differ): nothing installed"; return 1; }
  # The lock held only from here: the slow write above runs beside another publish's install.
  until nas_lock $lock "$who"; do
    [[ -n $said ]] || { print -r -- "waiting for the publish holding $lock ($lock_held)"; said=1; }
    sleep ${nas_lock_poll:-10}
  done
  _nas_cleanup+=("nas_unlock ${(q)lock} ${(q)who}")
  # (Again, under the lock: the publish that went in meanwhile may be newer, or a rollback since.)
  fast_forward $nas/app/current.json $head "$began" || return 1
  [[ ! -e $dest ]] || { print -r -- "$dest exists already"; return 1; }
  # (Signals ignored from before the go-ahead's read, so none lands between it and the writes; the
  # publishing Mac gone ends the read all the same: its end of the stream closes.)
  trap '' INT TERM HUP
  print -r -- @@ready
  IFS= read -r -t ${nas_go_wait:-300} go || go=
  [[ $go == go ]] || { print -r -- "no go-ahead from $from: nothing installed"; return 1; }
  mv $dest.tmp $dest
  # The NAS's own planet fetch (DSM's Task Scheduler runs it daily): the repository's copy, when it
  # differs. (Before the version goes live: a failure here publishes nothing.)
  if ! cmp -s $loc/fetch-planet.sh $nas/nas/fetch-planet.sh; then
    mkdir -p $nas/nas
    cp -X $loc/fetch-planet.sh $nas/nas/fetch-planet.sh.tmp && chmod 755 $nas/nas/fetch-planet.sh.tmp
    mv $nas/nas/fetch-planet.sh.tmp $nas/nas/fetch-planet.sh
    print -r -- "updated nas/fetch-planet.sh" || true
  fi
  [[ ! -f $nas/app/current.json ]] || cp -X $nas/app/current.json $nas/app/previous.json
  cp -X $loc/manifest.json $nas/app/current.json.tmp
  mv $nas/app/current.json.tmp $nas/app/current.json
  print -r -- "@@installed $version" || true
}
# _nas_begin: nas_install's and nas_rollback's start. Their cleanup (_nas_end, as the script ends:
# its commands carry their values, the function's locals gone) put in place; SIGPIPE ignored, so a
# write to a closed connection fails, ending the script under errexit, cleaned up; and their errors
# kept in a file (incoming/<pid>.err), shown as they end: zsh exits at once, cleaning nothing up,
# on a write to a stderr whose reader is gone, SIGPIPE ignored or not (exit 141: on 11 Nov, the
# lock left held and the version not made live after the go-ahead).
_nas_begin() {
  typeset -ga _nas_cleanup=()
  typeset -g _nas_err=~/Library/Caches/scenic-publish/incoming/$$.err
  at_end _nas_end
  trap '' PIPE
  mkdir -p ${_nas_err:h}
  exec 2>$_nas_err
  _nas_cleanup+=("cat ${(q)_nas_err} || true; rm -f ${(q)_nas_err}")
}
_nas_end() { local rc=$? c; for c in $_nas_cleanup; do eval $c; done; return $rc; }

# nas_rollback <nas> <from>: publish.sh --rollback, on the NAS's Mac: app/current.json and
# previous.json swapped under the NAS's lock (not waited for), unless rollback_ok refuses.
nas_rollback() {
  local nas=$1 lock=$1/app/publish.lock
  local who="$(scutil --get LocalHostName 2>/dev/null || hostname -s) $$ $(date -u +%Y-%m-%dT%H:%M:%SZ) for $2"
  _nas_begin
  [[ -d $nas/app ]] || { print -r -- "the NAS isn't mounted at $nas on ${who%% *}"; return 2; }
  nas_lock $lock "$who" || { print -r -- "another publish holds $lock ($lock_held): try again once it's over"; return 1; }
  _nas_cleanup+=("nas_unlock ${(q)lock} ${(q)who}")
  [[ -f $nas/app/previous.json ]] || { print -r -- "no previous version to roll back to"; return 1; }
  rollback_ok $nas/app/current.json $nas/app/previous.json || return 1
  cp -X $nas/app/current.json $nas/app/current.json.tmp.old
  cp -X $nas/app/previous.json $nas/app/current.json.tmp && mv $nas/app/current.json.tmp $nas/app/current.json
  mv $nas/app/current.json.tmp.old $nas/app/previous.json
  print -r -- "rolled back to $(live_version $nas/app/current.json)"
}
