# A folder for programs a script builds or copies and then runs (#1131; tools/dev/testing.md, "A
# program that ran is kept 10 minutes"). Sourced:
#   dir=$(ran_dir <name>)
# prints a new folder ${TMPDIR:-/tmp}/scenic-ran/<pid>.<name>.XXXXXX, <pid> the script's ($$). The
# script never removes it: Gatekeeper scans a program's file after it starts, and syspolicyd crashed
# whenever the file was gone by then (no new process starts on the Mac while it restarts, up to
# minutes). Each ran_dir first cleans the folder (ran_clean); store::testkit::ran_dir (Rust tests)
# does the same there, and the cargo shim in its .scenic-runs (tools/dev/test/ran.zsh checks
# testkit says the same time).
RAN_KEEP_S=600
ran_dir() {
  emulate -L zsh
  local root=${TMPDIR:-/tmp}; root=${root%/}/scenic-ran
  mkdir -p $root || return 1
  ran_clean $root
  mktemp -d $root/$$.$1.XXXXXX
}
# ran_clean <folder>: of its folders named <pid> or <pid>.<anything>, those whose process is gone
# are marked (.scenic-gone) when first seen so, and removed once marked RAN_KEEP_S: at least that
# long after the last program its process ran from it. The folder's own time says nothing of that:
# a script running 15 minutes may run a program from it in its last one, writing nothing in the
# folder itself after its first.
ran_clean() {
  emulate -L zsh
  local d m
  local -a old
  for d in $1/<->(|.*)(N/); do
    kill -0 ${${d:t}%%.*} 2>/dev/null && continue
    m=$d/.scenic-gone
    if [[ ! -e $m ]]; then touch $m 2>/dev/null; continue; fi
    old=( $m(N.ms+$(( RAN_KEEP_S - 1 ))) )
    (( $#old )) || continue
    chmod -R u+rwx $d 2>/dev/null; rm -rf $d 2>/dev/null
  done
  return 0
}
