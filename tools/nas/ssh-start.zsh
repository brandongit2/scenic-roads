# Sourced by tools/nas's scripts: an ssh that can't hang authenticating (#638). A fresh connection's
# key comes from the SSH agent, 1Password's, which waits for an approval while it's locked, past
# ConnectTimeout. `ssh_start <host>` opens a shared connection within 60 s (else exits 1, saying
# why); the script's later `$SSH <host> …` calls ride it, with no wait on the agent: long transfers
# stay unbounded.
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=20 -o ControlMaster=auto -o 'ControlPath=~/.ssh/cm-%r@%h:%p' -o ControlPersist=12h -o LogLevel=ERROR)

bounded() {  # <seconds> <command...>: the command, stopped (by its PID) past <seconds>, then 124
  "${@:2}" <&0 &
  local p=$! w rc
  ( trap 'kill $s 2>/dev/null; exit 1' TERM; sleep $1 & s=$!; wait $s && kill $p ) </dev/null >/dev/null 2>&1 &
  w=$!
  wait $p; rc=$?
  kill $w 2>/dev/null; wait $w && rc=124
  return $rc
}

ssh_start() {  # <host>
  bounded 60 $SSH $1 true </dev/null && return 0
  local rc=$?
  if (( rc == 124 )); then print -ru2 -- "no ssh connection to $1 within 60 s (connecting, or its SSH agent waiting for an approval: 1Password locked?)"
  else print -ru2 -- "ssh to $1 failed ($rc)"
  fi
  exit 1
}
