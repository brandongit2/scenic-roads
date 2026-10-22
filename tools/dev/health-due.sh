#!/bin/zsh
# Claude Code hook (PostToolUse, Stop; .claude/settings.local.json). Exits 2 with a message, which
# Claude Code hands to the PM (and a Stop is blocked), when either is over 20 minutes old:
# - the last health check (tools/dev/health.sh stamps .claude/health-stamp);
# - the task board's status line (`meta/status`'s `updated`, written with every board update).
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
# Subagents' tool calls carry an agent_id in the hook's input: the check is the PM's, not theirs.
input=$(cat)
[[ $input == *'"agent_id"'* ]] && exit 0
now=$(date +%s)
last=$(cat $repo/.claude/health-stamp 2>/dev/null || echo 0)
upd=$(sqlite3 -readonly $repo/tools/taskboard/taskboard.db "select json_extract(v,'$.updated') from meta where k='status'" 2>/dev/null)
[[ -z $upd ]] && upd=$(sqlite3 -readonly $repo/tools/taskboard/taskboard.db "select json_extract(value,'$.updated') from meta where key='status'" 2>/dev/null)
board=$(date -j -u -f '%Y-%m-%dT%H:%M:%S' "${upd%%.*}" +%s 2>/dev/null || echo 0)
msg=
(( now - last >= 1200 )) && msg+="HEALTH CHECK OVERDUE (last $(( (now - last) / 60 )) min ago): run tools/dev/health.sh now, act on what it flags, check every running agent. "
(( now - board >= 1200 )) && msg+="TASK BOARD OVERDUE (status line last updated $(( (now - board) / 60 )) min ago): update every changed task's stage and history, and PATCH the status line (meta/status {now, updated}). "
[[ -z $msg ]] && exit 0
print -u2 "$msg"
exit 2
