# A folder for programs a script builds or copies and then runs (#1131; tools/dev/testing.md, "A
# program that ran is kept 10 minutes"). Sourced:
#   dir=$(ran_dir <name>)
# prints a new folder ${TMPDIR:-/tmp}/scenic-ran/<pid>.<name>.XXXXXX, <pid> the script's ($$). The
# script never removes it: Gatekeeper scans a program's file after it starts, and syspolicyd crashed
# whenever the file was gone by then (no new process starts on the Mac while it restarts, up to
# minutes). Each ran_dir first removes the folders whose script is gone and that are unchanged for
# RAN_KEEP_S; store::testkit::ran_dir (Rust tests) does the same in the same folder, and the cargo
# shim keeps its run clones as long (tools/dev/test/ran.zsh checks the three say the same).
RAN_KEEP_S=600
ran_dir() {
  local root=${TMPDIR:-/tmp}; root=${root%/}/scenic-ran
  local d
  mkdir -p $root || return 1
  for d in $root/<->.*(N/ms+$(( RAN_KEEP_S - 1 ))); do
    kill -0 ${${d:t}%%.*} 2>/dev/null && continue
    chmod -R u+rwx $d 2>/dev/null; rm -rf $d
  done
  mktemp -d $root/$$.$1.XXXXXX
}
