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
