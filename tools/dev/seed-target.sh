#!/bin/zsh
# Retired by #915 (agent worktrees share one build folder: tools/dev/bin/cargo). Kept as a no-op only
# because .claude/settings.local.json's PreToolUse hook on Bash still runs it; delete this file once
# that hook entry is gone.
case $#:${1:-} in
  0:) cat >/dev/null 2>&1; exit 0 ;;
  1:-h|1:--help) print "seed-target.sh: retired (#915), a no-op hook until settings drop it"; exit 0 ;;
  *) print -u2 "seed-target.sh takes no arguments (retired, #915)"; exit 2 ;;
esac
