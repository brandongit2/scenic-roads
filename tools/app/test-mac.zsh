#!/bin/zsh
# The test Mac (owner, 31 Oct and 10 Nov: tests run on the M4): its LocalHostName, the one definition
# (SCENIC_TEST_MAC overrides it); sourced by remote-tests.sh's local side (the test Mac's side needs none of it). Everything that decides
# "is this the test Mac?" asks these: remote-tests.sh,
# tools/dev/test-guard.py (the hook that refuses an agent's local test run) and the tools/dev/bin/cargo shim.
# test_mac_name: the test Mac's LocalHostName.
# this_mac: this Mac's LocalHostName (SCENIC_THIS_MAC overrides it: a test's stand-in for another Mac).
# is_test_mac: 0 on the test Mac.
# tests_local: 0 when a test run here was asked for (SCENIC_TESTS_LOCAL=1, or SCENIC_CHECK_LOCAL=1: either, set
# to anything): the PM's fallback while the test Mac is away, or a check on purpose; never an agent's own choice.
test_mac_name() { print -r -- ${SCENIC_TEST_MAC:-Brandons-MacBook-Pro}; }
this_mac() { print -r -- ${SCENIC_THIS_MAC:-$(scutil --get LocalHostName 2>/dev/null)}; }
is_test_mac() { [[ $(this_mac) == $(test_mac_name) ]]; }
tests_local() { [[ -n ${SCENIC_CHECK_LOCAL:-}${SCENIC_TESTS_LOCAL:-} ]]; }
