#!/bin/zsh
# Retired by #915 (agent worktrees share one build folder: tools/dev/bin/cargo). Kept as a no-op only
# because .claude/settings.local.json's PreToolUse hook on Bash still runs it; delete this file once
# that hook entry is gone.
cat >/dev/null 2>&1
exit 0
