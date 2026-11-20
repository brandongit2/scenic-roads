# tools/dev/shell-guard.sh's own rules (the owner's shell rules, for every session). Reads the
# PreToolUse input on stdin; prints the reason to block, or nothing.
# - no `find /` (a walk of the whole disk: hours, and nothing to find there; search the repo, the
#   NAS's project folder or a named folder instead);
# - no waiting on `pgrep -f` in a loop's condition, or a PID taken from one (`kill -0 $(pgrep -f
#   x)`): its pattern is on the waiting shell's own command line, so it matches itself and never
#   ends; wait on the PID, `while kill -0 <pid>; do sleep 30; done`;
# - no open-ended `until grep` wait, nor any loop that sleeps with no deadline in itself (#1525; "wait
#   loops" below says what bounds one and which scripts are read; quoted words, comments and a
#   heredoc fed to a program that isn't a shell are no commands);
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
# A loop is read as the shell reads it (shellwords): words in quotes, a comment or a heredoc fed to a
# program that isn't a shell are no commands. Each `while`/`until` loop (zsh's short `while c { … }`
# too, and `for ((;;))`) that sleeps needs a bound in its own condition or body, an inner loop's
# not counting: a comparison (`[`, `[[`, `test` with -gt/-ge/-lt/-le/-eq/-ne/=/!=, or `(( … ))`) of
# the clock (`$SECONDS`, `$EPOCHSECONDS`, `$(date …)`, a variable set from those) or of a counter
# the loop changes by arithmetic (`let`, `(( i++ ))`, `i=$((i+1))`); a PID's wait (`kill -0` as its
# condition or with `|| break`, `ps -p`, either over ssh); or `timeout` round the whole loop. A loop
# reading lines (`while read`) ends with its input. Read too: the scripts of substitutions, `sh -c`,
# `eval`, ssh's remote command, a heredoc or here-string fed to a shell (or piped to one), and ssh
# with no remote command or a shell as it.
SHELLS = ("sh", "bash", "zsh", "dash")
SSH_TAKES = set("bcDEeFIiJLlmOoopQRSWw")
# (The words that lead a command: taken off before its own words are read.)
KEYWORDS = ("then", "else", "elif", "if", "!", "fi", "sudo", "do", "while", "until", "{", "}")
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1([^\n]*)\n(.*?)\n\s*\2[ \t]*(?=\n|$)", re.S)
HERESTRING = re.compile(r"(?:^|[\s;&|(])(?:sh|bash|zsh|dash)\b[^\n;&|]*?<<<\s*(?:'([^']*)'|\"((?:[^\"\\]|\\.)*)\"|(\S+))")
ARITH = re.compile(r"\(\((.*?)\)\)", re.S)
PIDWAIT = re.compile(r"\bkill\s+-0\s+[^;&|\n]*\|\|\s*break\b")
TEST_CMP = ("-gt", "-ge", "-lt", "-le", "-eq", "-ne", "=", "==", "!=")
CLOCKS = ("SECONDS", "EPOCHSECONDS", "EPOCHREALTIME")
UNTIL_GREP = "No open-ended `until grep` wait (it never ends if the text never comes): bound it (e.g. `[ $SECONDS -gt 1800 ] && break`) or wait on the PID that writes the log."
PGREP = "No waiting on `pgrep -f` (it matches its own shell and never ends): wait on the PID, `while kill -0 <pid>; do sleep 30; done`."
NO_DEADLINE = "No wait loop without a deadline (it never ends if what it waits for never comes, or is deleted): bound it in the loop (e.g. `[ $SECONDS -gt 1800 ] && break`), or wait on the PID with `while kill -0 <pid>; do sleep 30; done`; better, run the command in the background and wait for its notification (tools/dev/agent-rules.md, \"Turns\")."


def cmd_words(seg):
    """A segment's command words: its leading keywords, then assignments, prefixes and `sudo` off."""
    w = tokens(seg)
    while True:
        while w and (w[0] in KEYWORDS or w[0] == "function"):
            w = w[2:] if w[0] == "function" else w[1:]
        _, rest = command(w)
        if rest == w:
            return w
        w = rest


def remote(rest):
    """ssh's remote command (its words after the host), or None."""
    k = 0
    while k < len(rest) and rest[k].startswith("-"):
        k += 2 if len(rest[k]) == 2 and rest[k][1] in SSH_TAKES else 1
    return " ".join(rest[k + 1:]) if k + 1 < len(rest) else None


def reads_script(w):
    """Whether a command (its words) runs its input as a script: a shell, or ssh with no remote
    command or a shell as it."""
    if not w:
        return False
    c0 = os.path.basename(w[0])
    if c0 in SHELLS:
        return True
    if c0 == "ssh":
        r = remote(w[1:])
        return r is None or os.path.basename((r.split() or [""])[0]) in SHELLS
    return False


def scripts(text, depth=0):
    """The scripts a command line runs, itself first: (script, bounded, its substitutions' bodies),
    `bounded` when it runs under `timeout`; each substitution in the script is `${SUBSTITUTION<k>}`."""
    out = []
    bodies, kept, at = [], [], 0
    for m in HEREDOC.finditer(text):
        line = re.split(r"[\n;&|()]", text[:m.start()])[-1]
        piped = re.match(r"\s*\|([^;&|\n]*)", m.group(3))
        if reads_script(cmd_words(line)) or (piped and reads_script(cmd_words(piped.group(1)))):
            bodies.append(m.group(4))
        kept.append(text[at:m.start()] + "<<HEREDOC" + m.group(3))
        at = m.end()
    text = "".join(kept) + text[at:]
    for m in HERESTRING.finditer(text):
        bodies.append(next(g for g in m.groups() if g is not None))
    text, inner = substitutions(text)
    n = iter(range(len(inner) + 1))
    text = re.sub(r"\$\{SUBSTITUTION\}", lambda _: "${SUBSTITUTION%d}" % next(n), text)
    out.append((text, False, inner))
    if depth >= 4:
        return out
    for b in bodies + inner:
        out += scripts(b, depth + 1)
    for seg in segments(text):
        w = cmd_words(seg)
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
            out += [(s, b or timed, i) for s, b, i in scripts(g, depth + 1)]
    return out


def loops(text):
    """Each `while`/`until` loop of a script: {kind, cond, body, sleeps, counted}, cond and body its
    own commands as (assignments, words), an inner loop's apart; `sleeps` when it or a loop inside
    it sleeps."""
    text = ARITH.sub(lambda m: " __arith__ '" + m.group(1).replace("'", "") + "' ", text)
    text = PIDWAIT.sub(" __pidwait__ ", text)
    out, open_ = [], []

    def close():
        f = open_.pop()
        if f["kind"] != "for":
            out.append(f)

    for seg in segments(text):
        w = tokens(seg)
        while w:
            h, top = w[0], (open_[-1] if open_ else None)
            if h in ("while", "until"):
                open_.append({"kind": h, "cond": [], "body": [], "on": False, "short": 0, "sleeps": False, "counted": False})
                w = w[1:]
            elif h in ("for", "select"):
                f = {"kind": "for", "cond": [], "body": [], "on": False, "short": 0, "sleeps": False, "counted": False}
                if h == "for" and len(w) > 2 and w[1] == "__arith__":
                    # (`for ((a; b; c))`: a loop on b, counted by c; `for ((;;))` endless.)
                    parts = w[2].split(";")
                    f.update(kind="while", counted=len(parts) > 1 and parts[1].strip() != "")
                    w = w[3:]
                else:
                    w = w[w.index("{"):] if "{" in w else []
                open_.append(f)
            elif h == "do" and top and not top["on"]:
                top["on"] = True; w = w[1:]
            elif h == "done" and top and not top["short"]:
                close(); w = w[1:]
            elif h == "{":
                if top and not top["on"]:
                    top.update(on=True, short=1)   # (zsh's short form: `while c { … }`)
                elif top and top["short"]:
                    top["short"] += 1
                w = w[1:]
            elif h == "}":
                if top and top["short"]:
                    top["short"] -= 1
                    if not top["short"]:
                        close()
                w = w[1:]
            elif h == "function":
                w = w[2:]
            elif h in KEYWORDS:
                w = w[1:]
            else:
                k = next((i for i, x in enumerate(w) if x in ("{", "}")), len(w))
                sets, c = command(w[:k])
                w = w[k:]
                if top and (c or sets):
                    (top["body"] if top["on"] else top["cond"]).append((sets, c))
                    if c and os.path.basename(c[0]) == "sleep":
                        for f in open_:
                            f["sleeps"] = True
    return out


def sub_of(word, subs):
    """The bodies of the substitutions in a word."""
    return [subs[int(k)] for k in re.findall(r"SUBSTITUTION(\d+)", word) if int(k) < len(subs)]


def pgrep_f(words):
    return bool(words) and os.path.basename(words[0]) == "pgrep" and any(re.match(r"^-\w*f", x) for x in words[1:])


def finds_by_pgrep(c, subs):
    """Whether a command is `pgrep -f`, or takes a word from one (`kill -0 $(pgrep -f x)`)."""
    return pgrep_f(c) or any(pgrep_f(cmd_words(s)) for x in c for b in sub_of(x, subs) for s in segments(b))


def pid_wait(c):
    """Whether a condition waits on a PID: `kill -0`, `ps -p`, or either as ssh's remote command."""
    c0 = os.path.basename(c[0])
    if c0 == "ssh" and (r := remote(c[1:])):
        c, c0 = r.split(), (r.split() or [""])[0]
    return (c0 == "kill" and "-0" in c) or (c0 == "ps" and any(re.match(r"^-\w*p", x) for x in c[1:]))


def bounded(f, subs):
    """Whether a loop's own condition or body bounds it (the header's list)."""
    cmds = f["cond"] + f["body"]
    if f["counted"] or any(c and pid_wait(c) for _, c in f["cond"]) or any(c[:1] == ["__pidwait__"] for _, c in f["body"]):
        return True
    clocks = set(CLOCKS) | {"SUBSTITUTION%d" % k for k, b in enumerate(subs) if (b.split() or [""])[0] == "date"}
    counters = set()
    for sets, c in cmds:
        for k, v in sets.items():
            for b in sub_of(v, subs):
                h = b.lstrip()
                if h.startswith("(") or h.split()[:1] == ["expr"]: counters.add(k)
                if h.split()[:1] == ["date"]: clocks.add(k)
            if any(re.search(r"\$\{?%s\b" % x, v) for x in CLOCKS): clocks.add(k)
        if c[:1] == ["let"]:
            counters |= {re.match(r"\W*(\w+)", x).group(1) for x in c[1:] if re.match(r"\W*\w", x)}
        if c[:1] == ["__arith__"] and len(c) > 1:
            counters |= {a or b for a, b in re.findall(r"(\w+)\s*(?:\+\+|--|[-+*/%]?=(?!=))|(?:\+\+|--)\s*(\w+)", c[1])}
    moving = clocks | counters
    for _, c in cmds:
        c0 = os.path.basename(c[0]) if c else ""
        if c0 in ("[", "[[", "test") and any(x in TEST_CMP for x in c):
            if {n for x in c for n in re.findall(r"\$\{?(\w+)", x)} & moving:
                return True
        if c0 == "__arith__" and len(c) > 1 and re.search(r"[<>]|[=!]=", c[1]):
            if set(re.findall(r"[A-Za-z_]\w*", c[1])) & moving:
                return True
    return False


for script, under_timeout, subs in scripts(cmd):
    for f in loops(script):
        first = f["cond"][0][1] if f["cond"] else []
        if first and os.path.basename(first[0]) == "read":
            continue
        if any(finds_by_pgrep(c, subs) for _, c in f["cond"]) or any(
                c and os.path.basename(c[0]) == "kill" and finds_by_pgrep(c, subs) for _, c in f["body"]):
            block(PGREP)
        if under_timeout or bounded(f, subs):
            continue
        if f["kind"] == "until" and first and os.path.basename(first[0]) == "grep":
            block(UNTIL_GREP)
        if f["sleeps"]:
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
