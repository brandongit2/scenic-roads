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
