#!/bin/zsh
# The test Mac (owner, 31 Oct and 10 Nov: tests run on the M4): its LocalHostName, the one definition
# (SCENIC_TEST_MAC overrides it). Everything that decides "is this the test Mac?" sources this file:
# tools/app/remote-tests.sh's local side, tools/dev/test-guard.py (the hook that refuses an agent's
# test run off the test Mac) and the tools/dev/bin/cargo shim.
# test_mac_name: the test Mac's LocalHostName.
# this_mac: this Mac's LocalHostName (SCENIC_THIS_MAC overrides it: a test's stand-in for another Mac).
# is_test_mac: 0 on the test Mac.
# tests_local: 0 when tests were asked to run on this Mac (#1398), REPLY saying how: SCENIC_TESTS_LOCAL or
#   SCENIC_CHECK_LOCAL set (to anything) in the environment, as the PM or check.sh's caller sets it; or the
#   PM's marker, the main checkout's .claude/tests-local (SCENIC_TESTS_LOCAL_FILE in tests), which lets every
#   agent's tests on this Mac run here while the test Mac is away, until the PM deletes it. Only the PM sets
#   either: the hook refuses an agent's command that sets one of these variables or writes the marker.
_scenic_test_mac_dir=${${(%):-%x}:A:h}
test_mac_name() { print -r -- ${SCENIC_TEST_MAC:-Brandons-MacBook-Pro}; }
this_mac() { print -r -- ${SCENIC_THIS_MAC:-$(scutil --get LocalHostName 2>/dev/null)}; }
is_test_mac() { [[ $(this_mac) == $(test_mac_name) ]]; }
tests_local_file() {  # the PM's marker (the main checkout's, also from a worktree's or publish's copy of this file)
  local c
  if [[ -n ${SCENIC_TESTS_LOCAL_FILE:-} ]]; then print -r -- $SCENIC_TESTS_LOCAL_FILE; return; fi
  c=$(git -C $_scenic_test_mac_dir rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || return 1
  print -r -- ${c:h}/.claude/tests-local
}
tests_local() {
  local f
  if [[ -n ${SCENIC_TESTS_LOCAL:-} ]]; then REPLY="SCENIC_TESTS_LOCAL set"
  elif [[ -n ${SCENIC_CHECK_LOCAL:-} ]]; then REPLY="SCENIC_CHECK_LOCAL set"
  elif f=$(tests_local_file) && [[ -e $f ]]; then REPLY="the PM's $f"
  else return 1; fi
}
