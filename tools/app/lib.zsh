# The shell steps publish.sh runs, as functions it sources, so each can be tested in a stand-in tree
# (tools/app/test/shell.zsh, run by tools/dev/check.sh when tools/app changes).

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
# One older than six hours (its Mac gone mid-publish) is moved aside, checked to be the one read
# (one another publish took meanwhile is put back), and taken.
nas_lock() {
  local lock=$1 who=$2 held age
  typeset -g lock_held=
  ( set -o noclobber; print -r -- "$who" > $lock ) 2>/dev/null && return 0
  held=$(cat $lock 2>/dev/null)
  age=$(( $(date +%s) - $(stat -f %m $lock 2>/dev/null || date +%s) ))
  if (( age < 6 * 3600 )); then lock_held="$held, $(( age / 60 )) min ago"; return 1; fi
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
  local d=$1 me="$$ $(ps -o lstart= -p $$)" held pid
  typeset -g lock_held=
  until mkdir $d 2>/dev/null; do
    held=$(cat $d/owner 2>/dev/null); pid=${held%% *}
    if [[ -n $held ]]; then
      kill -0 $pid 2>/dev/null && [[ "$pid $(ps -o lstart= -p $pid)" == "$held" ]] && { lock_held="process $held"; return 1; }
    elif [[ -n $(found $d -maxdepth 0 -newermt '10 seconds ago') ]]; then lock_held="being taken"; return 1
    fi
    mv $d $d.aside-$$ 2>/dev/null || continue
    if [[ $(cat $d.aside-$$/owner 2>/dev/null) != "$held" ]]; then lock_held="taken just now"; return 1; fi
    rm -rf $d.aside-$$
  done
  print -r -- "$me" > $d/owner
}
unhold() { [[ $(cat $1/owner 2>/dev/null) == "$$ $(ps -o lstart= -p $$)" ]] && rm -rf $1; return 0; }

# fast_forward <current.json> <commit>: 0 when nothing is live, or the live version's commit (its
# name's last part) is <commit> or an ancestor of it; else says why and returns 1: a publish never
# puts an older tree, or one without everything published, over the live one. Run in the repository.
fast_forward() {
  [[ -f $1 ]] || return 0
  local v live
  v=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' $1) || { print -r -- "can't read $1"; return 1; }
  live=${v##*-}
  if ! git cat-file -e "$live^{commit}" 2>/dev/null; then print -r -- "the live version $v's commit $live isn't in this repository"; return 1; fi
  git merge-base --is-ancestor $live $2 && return 0
  if git merge-base --is-ancestor $2 $live; then print -r -- "$2 is older than the live version $v: refused"
  else print -r -- "$2 doesn't contain the live version $v (diverged): refused"; fi
  return 1
}
