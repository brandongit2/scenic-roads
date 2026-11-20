# tools/dev/shell-guard.sh's own rules (the owner's shell rules, for every session). Reads the
# PreToolUse input on stdin; prints the reason to block, or nothing.
# - no `find /` (a walk of the whole disk: hours, and nothing to find there; search the repo, the
#   NAS's project folder or a named folder instead);
# - no waiting on `pgrep -f` in a loop (its pattern is on the waiting shell's own command line, so
#   it matches itself and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`;
# - no open-ended `until grep` wait, nor any `while`/`until` loop that sleeps with no deadline
#   (#1525: `$SECONDS`, `timeout`, a counted `seq` or arithmetic, `-gt`/`-ge`), but one on a PID
#   (`kill -0`) or reading lines (`while read`); read with heredocs' text left out, so a script
#   written out passes;
# - no `ln -s` without `-n` or `-h` (#1004): where the link's name is already a link to a folder, it
#   makes the link inside that folder (on the NAS, `inputs/inputs` and 16 more like it, 7 Nov);
# - no delete of the session's shared scratchpad (#1521): every subagent of a session gets the same
#   scratchpad, <tmp>/claude-<uid>/<project>/<session>/scratchpad, and `rm -rf <scratchpad>/*` by
#   one wiped the others' files (17 times in 30 h). Refused: `rm` (or `trash`) of the scratchpad, of a folder
#   above it, or of a glob directly in it (`<scratchpad>/*`, `<scratchpad>/t14*`); `find` from any
#   of those with -delete, or -exec/-execdir/-ok rm, or piped to `xargs rm`. A delete inside a
#   folder of the scratchpad (an agent's own, tools/dev/agent-rules.md, "Scratch") passes.
#   A path is read as the shell would: variables set or exported earlier on the line, `~`, the
#   folder a `cd`/`pushd` earlier on the line went to, and the hook's own environment, which is
#   Claude Code's (HOME, TMPDIR, CLAUDE_CODE_SESSION_ID; never a variable an earlier call
#   exported). A path that can't be told (a substitution, a variable unknown here) passes; Claude
#   Code's own check on `rm` asks about such a target. Not seen: a list piped to xargs from anything but a find, for-loops,
#   typeset/local/declare assignments, a variable set from another (`U=$T`), `find -exec sh -c 'rm …'`.
# The ln and scratchpad rules read commands (tools/dev/shellwords.py): quoted text, a comment or a
# heredoc is no command; the scripts of substitutions, sh/bash/zsh -c and eval are read too.
import fnmatch, json, os, re, sys
from shellwords import strip_heredocs, segments, substitutions, tokens, command

d = json.load(sys.stdin)
if d.get("tool_name") != "Bash": sys.exit()
cmd = (d.get("tool_input") or {}).get("command") or ""


def block(m):
    print(m + " (tools/dev/shell-guard.py)"); sys.exit()


for seg in re.split(r"&&|\|\||;|\||\(|\)|\$\(|`", cmd):
    if re.match(r"\s*(?:sudo\s+)?(?:find|bfs)\s+(?:-\S+\s+)*/(?:\s|$)", seg):
        block("No `find /` (the owner's rule): search the repo, the NAS project folder or a named folder.")
if re.search(r"\b(?:until|while)\b[^\n]*\bpgrep\s+(?:-\w*\s+)*-\w*f", cmd):
    block("No waiting on `pgrep -f` (it matches its own shell and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`.")
BOUND = re.compile(r"SECONDS|seq\s|\$\(\(|-gt\s|-ge\s|timeout|kill\s+-0")
if re.search(r"\buntil\s+!?\s*grep\b", cmd) and not BOUND.search(cmd):
    block("No open-ended `until grep` wait (it never ends if the text never comes): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`) or wait on the PID that writes the log.")
# (#1525: every wait loop has a deadline. A `while`/`until` loop that sleeps, other than one reading
# lines, which ends with its input, needs a bound: #1361's `until ssh … grep …; do sleep 20; done`
# ran two days once its log's folder was deleted.)
_plain = strip_heredocs(cmd)
if re.search(r"\b(?:while|until)\b(?!\s+(?:IFS=\S*\s+)?read\b)[\s\S]*?\bdo\b[\s\S]*?\bsleep\b", _plain) and not BOUND.search(_plain):
    block("No wait loop without a deadline (it never ends if what it waits for never comes, or is deleted): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`), or wait on the PID with `while kill -0 <pid>; do sleep 30; done`; better, run the command in the background and wait for its notification (tools/dev/agent-rules.md, \"Turns\").")

LEAD = ("do", "then", "else", "elif", "if", "while", "until", "!", "{", "sudo")
GLOB = re.compile(r"[*?\[{]")
# The scratchpad's path, /tmp or /private/tmp on, by level: a literal must match the level's regex,
# a glob must be able to match the level's example.
LEVELS = [(r"tmp$", "tmp"), (r"claude-\d+$", "claude-%d" % os.getuid()), (r".+$", "p"), (r".+$", "s"), (r"scratchpad$", "scratchpad")]


def level_ok(i, c):
    return fnmatch.fnmatchcase(LEVELS[i][1], c) if GLOB.search(c) else bool(re.match(LEVELS[i][0], c))


def shared(p):
    """Whether deleting the absolute path p (globs kept) takes the shared scratchpad's files: p is
    the scratchpad or a folder above it, or a glob directly in it."""
    c = [x for x in p.split("/") if x]
    if c[:1] == ["private"]: c = c[1:]
    if not c: return False
    n = min(len(c), len(LEVELS))
    if not all(level_ok(i, c[i]) for i in range(n)): return False
    return len(c) <= len(LEVELS) or (len(c) == len(LEVELS) + 1 and bool(GLOB.search(c[-1])))


def path(p, base, env):
    """p as an absolute, normalized path from base (None: unknown), or None when it can't be told."""
    env = {**env, "PWD": base or "${UNKNOWN}"}   # (the shell's $PWD: where the command runs)
    p = re.sub(r"\$(\w+)|\$\{(\w+)(?:[:#%/?=+-][^}]*)?\}", lambda m: env.get(m.group(1) or m.group(2), m.group(0)), p)
    p = os.path.expanduser(p)
    if "$" in p or "`" in p or p.startswith("~") or (base is None and not p.startswith("/")): return None
    return os.path.normpath(os.path.join(base or "/", p))


def refuse(p):
    block("No delete of the session's shared scratchpad, %s: every agent of the session uses it, and this takes the others' files too. Keep your files in a folder of your own in it, named after your worktree (or made with `mktemp -d <scratchpad>/t<task>-XXXXXX`), and delete only that folder (tools/dev/agent-rules.md, \"Scratch\")." % p)


def operands(args):
    out, opts = [], True
    for a in args:
        if opts and a == "--": opts = False
        elif opts and a.startswith("-") and a != "-": continue
        else: out.append(a)
    return out


def find(rest, base, env, piped_rm):
    """A `find` (its words after the name): refused when it starts at a shared path and deletes."""
    k = 0
    while k < len(rest) and re.match(r"^-[HLPEXdsx]+$", rest[k]): k += 1
    starts = []
    while k < len(rest) and not re.match(r"^[-(!]", rest[k]): starts.append(rest[k]); k += 1
    expr = rest[k:]
    deletes = piped_rm or "-delete" in expr or any(
        t in ("-exec", "-execdir", "-ok", "-okdir") and j + 1 < len(expr) and os.path.basename(expr[j + 1]) in ("rm", "rmdir", "unlink")
        for j, t in enumerate(expr))
    if not deletes: return
    for s in starts:
        p = path(s, base, env)
        if p and shared(p): refuse(p)


def runs_rm_by_xargs(toks):
    """Whether a command's words are an `xargs` running rm or trash (`xargs -0 rm -rf`, not
    `xargs grep -l rm`)."""
    while toks and (toks[0] in LEAD or re.match(r"^\w+=", toks[0])): toks = toks[1:]
    if not (toks and os.path.basename(toks[0]) == "xargs"): return False
    _, run = command(toks)   # (command() takes xargs and its options off)
    return bool(run) and os.path.basename(run[0]) in ("rm", "trash")


def scan(text, base, env, depth=0):
    """Whether a command of text is an `ln -s` without -n/-h (returned); refuses a shared-scratchpad delete."""
    ln = False
    text, inner = substitutions(strip_heredocs(text))
    for t in inner:
        if depth < 4: ln |= scan(t, base, env, depth + 1)
    segs = segments(text)
    xargs_rm = any(runs_rm_by_xargs(tokens(s)) for s in segs)
    stack = []
    for seg in segs:
        sets, toks = command(tokens(seg))
        while toks and toks[0] in LEAD:
            more, toks = command(toks[1:]); sets = {**sets, **more}
        if not toks:
            env = {**env, **sets}; continue
        c0, rest = os.path.basename(toks[0]), toks[1:]
        here = {**env, **sets}
        if c0 in ("sh", "bash", "zsh", "dash") and depth < 4:
            for k, t in enumerate(rest):
                if re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest): ln |= scan(rest[k + 1], base, here, depth + 1); break
        elif c0 == "eval" and depth < 4: ln |= scan(" ".join(rest), base, here, depth + 1)
        elif c0 in ("cd", "pushd"):
            a = [r for r in rest if not r.startswith("-") or r == "-"]
            if c0 == "pushd": stack.append(base)
            base = None if a[:1] == ["-"] else path(a[0], base, env) if a else os.path.expanduser("~")
        elif c0 == "popd": base = stack.pop() if stack else None
        elif c0 == "export": env = {**env, **dict(r.split("=", 1) for r in rest if re.match(r"^\w+=", r))}
        elif c0 in ("rm", "trash"):
            for a in operands(rest):
                p = path(a, base, here)
                if p and shared(p): refuse(p)
        elif c0 in ("find", "bfs"): find(rest, base, here, xargs_rm)
        elif c0 == "ln":
            flags = set()
            for t in rest:
                if t == "--": break
                if t in ("--symbolic", "--no-dereference"): flags.add(t[2])
                elif re.match(r"^-[A-Za-z]+$", t): flags |= set(t[1:])
            if "s" in flags and not flags & set("nh"): ln = True
    return ln


if scan(cmd, d.get("cwd") or None, dict(os.environ)):
    block("No `ln -s` without `-n` (the owner's rule, #1004): where the link's name is already a link to a folder, `ln -s` makes the link inside that folder (on the NAS: `inputs/inputs`, a loop for every walker). Use `ln -sn` (it fails on a name taken by a link; a name that is a folder is still linked into: never name one), or make the link in code.")
