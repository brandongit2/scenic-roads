#!/bin/zsh
# Claude Code hook (PreToolUse on Bash; .claude/settings.local.json). Exits 2 with the reason
# (Claude Code blocks the call and shows it) for the owner's shell rules, for every session:
# - no `find /` (a walk of the whole disk: hours, and nothing to find there; search the repo, the
#   NAS's project folder or a named folder instead);
# - no waiting on `pgrep -f` in a loop (its pattern is on the waiting shell's own command line, so
#   it matches itself and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`;
# - no open-ended `until grep` wait;
# - no `ln -s` without `-n` or `-h` (#1004): where the link's name is already a link to a folder, it
#   makes the link inside that folder (on the NAS, `inputs/inputs` and 16 more like it, 7 Nov); read
#   as commands (tools/dev/shellwords.py), so quoted text, a comment or a heredoc is none;
# - no git write in a checkout that isn't the session's own (tools/dev/git-guard.py, #980).
set -uo pipefail
in=$(cat)
out=$(print -r -- $in | python3 -c '
import json, os, re, sys
d = json.load(sys.stdin)
if d.get("tool_name") != "Bash": sys.exit()
cmd = (d.get("tool_input") or {}).get("command") or ""
for seg in re.split(r"&&|\|\||;|\||\(|\)|\$\(|`", cmd):
    if re.match(r"\s*(?:sudo\s+)?(?:find|bfs)\s+(?:-\S+\s+)*/(?:\s|$)", seg):
        print("No `find /` (the owner'"'"'s rule): search the repo, the NAS project folder or a named folder."); sys.exit()
sys.path.insert(0, sys.argv[1])
from shellwords import strip_heredocs, segments, substitutions, tokens, command
def ln_follows(text, depth=0):
    """Whether a command of text (its commands, not its text: quotes, comments and heredocs are no
    command) is an `ln -s` without -n or -h; inside substitutions, sh -c and eval too."""
    text, inner = substitutions(strip_heredocs(text))
    if depth < 4 and any(ln_follows(t, depth + 1) for t in inner): return True
    for seg in segments(text):
        _, toks = command(tokens(seg))
        while toks and toks[0] in ("do", "then", "else", "elif", "if", "while", "until", "!", "{", "sudo"):
            _, toks = command(toks[1:])
        if not toks: continue
        c0, rest = os.path.basename(toks[0]), toks[1:]
        if c0 in ("sh", "bash", "zsh", "dash") and depth < 4:
            if any(re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest) and ln_follows(rest[k + 1], depth + 1) for k, t in enumerate(rest)): return True
        elif c0 == "eval" and depth < 4 and ln_follows(" ".join(rest), depth + 1): return True
        elif c0 == "ln":
            flags = set()
            for t in rest:
                if t == "--": break
                if t in ("--symbolic", "--no-dereference"): flags.add(t[2])
                elif re.match(r"^-[A-Za-z]+$", t): flags |= set(t[1:])
            if "s" in flags and not flags & set("nh"): return True
    return False
if ln_follows(cmd):
    print("No `ln -s` without `-n` (the owner'"'"'s rule, #1004): where the link'"'"'s name is already a link to a folder, `ln -s` makes the link inside that folder (on the NAS: `inputs/inputs`, a loop for every walker). Use `ln -sn` (it fails on a name taken by a link; a name that is a folder is still linked into: never name one), or make the link in code."); sys.exit()
if re.search(r"\b(?:until|while)\b[^\n]*\bpgrep\s+(?:-\w*\s+)*-\w*f", cmd):
    print("No waiting on `pgrep -f` (it matches its own shell and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`."); sys.exit()
if re.search(r"\buntil\s+!?\s*grep\b", cmd) and not re.search(r"SECONDS|seq\s|\$\(\(|-gt\s|-ge\s|timeout|kill\s+-0", cmd):
    print("No open-ended `until grep` wait (it never ends if the text never comes): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`) or wait on the PID that writes the log."); sys.exit()
' ${0:A:h} 2>&1)
# (A crash of either check blocks, showing its traceback.)
[[ -z $out ]] && out=$(print -r -- $in | python3 ${0:A:h}/git-guard.py 2>&1)
[[ -z $out ]] && exit 0
print -r -- "$out" >&2
exit 2
