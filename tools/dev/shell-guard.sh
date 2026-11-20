#!/bin/zsh
# Claude Code hook (PreToolUse on Bash; .claude/settings.local.json). Exits 2 with the reason
# (Claude Code blocks the call and shows it) for the owner's shell rules, for every session:
# tools/dev/shell-guard.py (a walk of the whole disk, waits on `pgrep -f`, open-ended
# `until grep` and wait loops with no deadline, `ln -s` without -n, deletes of the session's
# shared scratchpad), then
# tools/dev/git-guard.py (no git write in a checkout that isn't the session's own, #980).
# A crash of either check blocks, showing its traceback.
set -uo pipefail
in=$(cat)
out=$(print -r -- $in | python3 ${0:A:h}/shell-guard.py 2>&1)
[[ -z $out ]] && out=$(print -r -- $in | python3 ${0:A:h}/git-guard.py 2>&1)
[[ -z $out ]] && exit 0
print -r -- "$out" >&2
exit 2
