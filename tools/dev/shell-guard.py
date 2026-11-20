# tools/dev/shell-guard.sh's own rules (the owner's shell rules, for every session). Reads the
# PreToolUse input on stdin; prints the reason to block, or nothing.
# - no `find /` (a walk of the whole disk: hours, and nothing to find there; search the repo, the
#   NAS's project folder or a named folder instead);
# - no waiting on `pgrep -f` in a loop (its pattern is on the waiting shell's own command line, so
#   it matches itself and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`;
# - no open-ended `until grep` wait, nor any `while`/`until` loop that sleeps with no deadline
#   (#1525). The loops are read as the shell reads them (shellwords), and the bound is looked for in
#   the loop's own condition and body: `$SECONDS`, `timeout`, a test comparing with -gt/-ge/-lt/-le
#   (a counter) or an arithmetic comparison; a loop on a PID (`kill -0`) or reading lines
#   (`while read`) passes. Both wait rules read the same scripts: the command line, substitutions,
#   `sh -c`, `eval`, ssh's remote command, and heredocs fed to a shell or ssh (any other heredoc's
#   text, quoted words and comments are no commands);
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

# ---- wait loops (#1525) ----------------------------------------------------------------------------
# A `while`/`until` loop is read as the shell reads it (shellwords), its condition and its body:
# words in quotes, a comment or a heredoc fed to a program that isn't a shell are no commands. Its
# bound is looked for in the loop itself: `$SECONDS` in a word, a `timeout`, a test comparing with
# -gt/-ge/-lt/-le (a counter, `[ $i -lt 90 ]`) or an arithmetic comparison (`(( SECONDS > 60 ))`), or
# `kill -0` (a PID's wait, ended with the process). A loop under `timeout` is bounded. Read too: the
# scripts of substitutions, `sh|bash|zsh|dash -c`, `eval`, ssh's remote command, and a heredoc fed to a
# shell or to ssh.
SHELLS = ("sh", "bash", "zsh", "dash")
SSH_TAKES = set("bcDEeFIiJLlmOoopQRSWw")
LEADS = ("then", "else", "elif", "if", "!", "{", "}", "fi", "sudo")
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n(.*?)\n\s*\2[ \t]*(?=\n|$)", re.S)
COMPARE = ("-gt", "-ge", "-lt", "-le")
UNTIL_GREP = "No open-ended `until grep` wait (it never ends if the text never comes): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`) or wait on the PID that writes the log."
PGREP = "No waiting on `pgrep -f` (it matches its own shell and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`."
NO_DEADLINE = "No wait loop without a deadline (it never ends if what it waits for never comes, or is deleted): bound it in the loop (e.g. `[ $SECONDS -gt 1800 ] && break`), or wait on the PID with `while kill -0 <pid>; do sleep 30; done`; better, run the command in the background and wait for its notification (tools/dev/agent-rules.md, \"Turns\")."


def words_of(seg):
    """A segment's command words, leading assignments, prefixes and `sudo` taken off."""
    _, toks = command(tokens(seg))
    return toks


def remote(rest):
    """ssh's remote command (its words after the host), or None."""
    k = 0
    while k < len(rest) and rest[k].startswith("-"):
        k += 2 if len(rest[k]) == 2 and rest[k][1] in SSH_TAKES else 1
    return " ".join(rest[k + 1:]) if k + 1 < len(rest) else None


def scripts(text, depth=0):
    """The scripts a command line runs, itself first: (script, bounded) pairs, `bounded` when it runs
    under `timeout`."""
    out = []
    # (Heredocs: the body of one fed to a shell or ssh is a script; any heredoc is no command here.)
    bodies, kept, at = [], [], 0
    for m in HEREDOC.finditer(text):
        line = re.split(r"[\n;&|()]", text[:m.start()])[-1]
        w = words_of(line)
        while w and w[0] in LEADS + ("do",): w = w[1:]
        c0 = os.path.basename(w[0]) if w else ""
        if c0 in SHELLS or c0 == "ssh":
            bodies.append(m.group(3))
        kept.append(text[at:m.start()] + "<<HEREDOC")
        at = m.end()
    text = "".join(kept) + text[at:]
    text, inner = substitutions(text)
    out.append((text, False))
    if depth >= 4:
        return out
    for b in bodies + inner:
        out += scripts(b, depth + 1)
    for seg in segments(text):
        w = words_of(seg)
        while w and w[0] in LEADS + ("do", "while", "until"): w = w[1:]
        if not w:
            continue
        c0, rest = os.path.basename(w[0]), w[1:]
        timed = c0 in ("timeout", "gtimeout")
        if timed:
            # (timeout <duration> <command…>: what it runs is bounded.)
            rest = [r for r in rest if not r.startswith("-")][1:]
            if not rest: continue
            c0, rest = os.path.basename(rest[0]), rest[1:]
        got = []
        if c0 in SHELLS:
            for k, t in enumerate(rest):
                if re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest): got.append(rest[k + 1]); break
        elif c0 == "eval":
            got.append(" ".join(rest))
        elif c0 == "ssh" and (r := remote(rest)):
            got.append(r)
        for g in got:
            out += [(s, b or timed) for s, b in scripts(g, depth + 1)]
    return out


def loops(text):
    """Each `while`/`until` loop of a script: (kind, its condition's commands, its body's commands)."""
    # (Arithmetic, `(( … ))`, kept as one word: segments split at parentheses.)
    text = re.sub(r"\(\((.*?)\)\)", lambda m: " __arith__ '" + m.group(1).replace("'", "") + "' ", text)
    out, open_ = [], []
    for seg in segments(text):
        w = words_of(seg)
        while w:
            if w[0] in ("while", "until", "for", "select"):
                open_.append([w[0], [], [], False]); w = w[1:]
                if open_[-1][0] in ("for", "select"): w = []
            elif w[0] == "do" and open_:
                open_[-1][3] = True; w = w[1:]
            elif w[0] == "done" and open_:
                f = open_.pop(); w = w[1:]
                if f[0] in ("while", "until"): out.append((f[0], f[1], f[2]))
            elif w[0] in LEADS:
                w = w[1:]
            else:
                break
        if w:
            for f in open_:
                (f[2] if f[3] else f[1]).append(w)
    return out


def bounded(cmds):
    for c in cmds:
        c0 = os.path.basename(c[0])
        if any("SECONDS" in x for x in c): return True
        if c0 in ("timeout", "gtimeout"): return True
        if c0 == "kill" and "-0" in c: return True
        if c0 in ("[", "[[", "test") and any(x in COMPARE for x in c): return True
        if c0 == "__arith__" and re.search(r"[<>]|-[lg][te]\b", " ".join(c[1:])): return True
    return False


for script, under_timeout in scripts(cmd):
    for kind, cond, body in loops(script):
        first = cond[0] if cond else []
        while first and first[0] == "!": first = first[1:]
        c0 = os.path.basename(first[0]) if first else ""
        if c0 == "read":
            continue
        if any(os.path.basename(c[0]) == "pgrep" and any(re.match(r"^-\w*f", x) for x in c[1:]) for c in cond):
            block(PGREP)
        if under_timeout or bounded(cond + body):
            continue
        if kind == "until" and c0 == "grep":
            block(UNTIL_GREP)
        if any(os.path.basename(c[0]) == "sleep" for c in cond + body):
            block(NO_DEADLINE)

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
