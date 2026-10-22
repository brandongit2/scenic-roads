#!/bin/zsh
# Claude Code hook (PreToolUse on Bash; .claude/settings.local.json). Exits 2 with the reason
# (Claude Code blocks the call and shows it) for the owner's two shell rules, for every agent:
# - no `find /` (a walk of the whole disk: hours, and nothing to find there; search the repo, the
#   NAS's project folder or a named folder instead);
# - no waiting on `pgrep -f` in a loop (its pattern is on the waiting shell's own command line, so
#   it matches itself and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`.
out=$(python3 -c '
import json, re, sys
d = json.load(sys.stdin)
if d.get("tool_name") != "Bash": sys.exit()
cmd = (d.get("tool_input") or {}).get("command") or ""
for seg in re.split(r"&&|\|\||;|\||\(|\)|\$\(|`", cmd):
    if re.match(r"\s*(?:sudo\s+)?(?:find|bfs)\s+(?:-\S+\s+)*/(?:\s|$)", seg):
        print("No `find /` (the owner'"'"'s rule): search the repo, the NAS project folder or a named folder."); sys.exit()
if re.search(r"\b(?:until|while)\b[^\n]*\bpgrep\s+(?:-\w*\s+)*-\w*f", cmd):
    print("No waiting on `pgrep -f` (it matches its own shell and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`."); sys.exit()
if re.search(r"\buntil\s+!?\s*grep\b", cmd) and not re.search(r"SECONDS|seq\s|\$\(\(|-gt\s|-ge\s|timeout|kill\s+-0", cmd):
    print("No open-ended `until grep` wait (it never ends if the text never comes): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`) or wait on the PID that writes the log."); sys.exit()
')
[[ -z $out ]] && exit 0
print -r -- "$out" >&2
exit 2
