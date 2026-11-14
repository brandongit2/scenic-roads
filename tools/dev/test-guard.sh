#!/bin/zsh
# Claude Code hook (PreToolUse on Bash and Agent; .claude/settings.local.json). Enforces
# tools/dev/testing.md (the checks: tools/dev/test-guard.py); exits 2 with the reason, which Claude
# Code shows while blocking the call, when
# - the PM's Agent brief doesn't name tools/dev/agent-rules.md (the standing rules; but for
#   web-fetch, claude-code-guide and statusline-setup agents, which never touch the repo);
# - the PM's Agent brief mentions tests without pointing at tools/dev/testing.md;
# - a subagent's cargo test or nextest run (not --no-run) is on a Mac that isn't the test Mac
#   (tools/app/test-mac.zsh), unless tests_local holds in this hook's environment (Claude Code's:
#   SCENIC_TESTS_LOCAL or SCENIC_CHECK_LOCAL, or the PM's .claude/tests-local); check.sh and
#   tools/app/remote-tests.sh hold their cargo in a quoted argument: never found;
# - a subagent's command sets SCENIC_TESTS_LOCAL, SCENIC_CHECK_LOCAL, SCENIC_TESTS_LOCAL_FILE, SCENIC_THIS_MAC
#   or SCENIC_TEST_MAC, or writes the PM's .claude/tests-local (where tests run is the PM's call);
# - a subagent's cargo test or nextest run breaks the rules: --release without #perf; --workspace
#   without #repro or #perf; a filter that doesn't start with one of the crate's modules (bare words,
#   tests::, :: refused; a single --test/--bin may take bare names); a full suite (no filter, no
#   single --test/--bin) without #repro; in a worktree, anything outside what
#   tools/dev/affected-tests.sh prints for it (same -p and target, filters under its filters).
# Runs are found where a command starts (also after env/time/nice/command/xargs/exec, by a path to
# cargo, cargo flags before the subcommand, cargo t, nextest r, cargo-nextest) and inside the script
# of sh/bash/zsh -c, eval and python -c; other quoted text and heredocs are left alone. Refused:
# a run through a variable ($c test), cargo --config alias.…, cargo -C, make/just test, a run whose
# words hold $( … ), <( … ), >( … ) or backticks outside single quotes (a run inside one is found), and
# sh -c, eval or a command word built from a substitution whose script mentions cargo test.
# A crash of the checks blocks too, showing its traceback, and so does affected-tests.sh failing or
# taking over 120 s.
set -uo pipefail
out=$(python3 ${0:A:h}/test-guard.py 2>&1)
[[ -z $out ]] && exit 0
print -u2 -- "$out"
exit 2
