# tools/dev/shell-guard.sh's git check (task #980): a session writes with git only in its own
# checkouts. Reads the PreToolUse input on stdin; prints the reason to block, or nothing.
#
# A git write (WRITES, below; `stash` but its list/show/drop/clear/create/store) goes to the
# checkout its command names: `git -C <dir>`, `--work-tree`, `--git-dir` (a worktree's admin
# folder: that worktree), GIT_WORK_TREE/GIT_DIR (set on the line or exported earlier on it), or
# the folder a `cd`/`pushd`/`popd`/`env -C` earlier in the command line went to, else the
# session's cwd; `git worktree remove|move <path>` to <path>. Only checkouts of this repo are
# looked at (another repo, a folder that isn't one: no concern of this). Whose they are:
# - a subagent's worktree, .claude/worktrees/agent-<id> (Claude Code made it): agent <id>'s while
#   it runs (running(), below: its session's process is alive and the agent has written since
#   that process started, not stopped or finished); after, whoever writes there next, a reviewer
#   working on its branch or the main session; a subagent's write there claims it
#   (.git/worktree-owners/, the path's hash) while that subagent runs;
# - a worktree a subagent made itself (a `git worktree add <path>` this sees, of a path not yet
#   there, recorded in .git/worktree-owners/): that subagent's while it runs;
# - any other checkout (the main checkout, the PM's int-* worktrees, made before this guard or
#   after): the main session's (the PM). Main sessions are told apart from subagents only (the
#   hook input's agent_id), not by session id: a resumed or forked session has a new one.
# A write whose folder can't be told (a variable or substitution in the path) is refused.
# Commands are read as test-guard.py reads them (shellwords.py), the scripts of sh/bash/zsh -c and
# eval too; what a script run by the command does is not seen, nor a `cd` in a subshell, `( … )`,
# which the line's later commands are taken to be in too.
import glob, hashlib, json, os, re, subprocess, sys
from datetime import datetime
from shellwords import strip_heredocs, segments, substitutions, tokens, command

WRITES = {"reset", "checkout", "switch", "restore", "commit", "rebase", "merge", "cherry-pick", "revert",
          "am", "pull", "clean", "add", "rm", "mv", "apply", "read-tree", "update-index", "checkout-index"}
STASH_READS = {"list", "show", "drop", "clear", "create", "store"}
GIT_TAKES = {"-c", "--namespace", "--exec-path", "--config-env", "--super-prefix", "--list-cmds", "--attr-source"}

d = json.load(sys.stdin)
if d.get("tool_name") != "Bash": sys.exit()
cmd = (d.get("tool_input") or {}).get("command") or ""
if "git" not in cmd: sys.exit()
cwd, agent = d.get("cwd") or os.getcwd(), d.get("agent_id") or ""
here = os.path.dirname(os.path.abspath(__file__))
common = os.path.realpath(subprocess.check_output(["git", "-C", here, "rev-parse", "--path-format=absolute", "--git-common-dir"], text=True).strip())
repo = os.path.dirname(common)   # the main checkout
owners = os.path.join(common, "worktree-owners")


def block(m):
    print(m + " (tools/dev/git-guard.py; tools/dev/testing.md, \"Whose checkout\")"); sys.exit()


def path(p, base, env):
    """p as an absolute path from base (None: unknown), or None when the shell's expansion can't be told."""
    env = {**env, "PWD": base or "${UNKNOWN}"}   # (the shell's $PWD, $(pwd): where the command runs)
    p = re.sub(r"\$(\w+)|\$\{(\w+)\}", lambda m: env.get(m.group(1) or m.group(2), m.group(0)), p)
    p = os.path.expandvars(os.path.expanduser(p))
    if re.search(r"[$`*?\[]|^~", p) or (base is None and not p.startswith("/")): return None
    return os.path.normpath(os.path.join(base or "/", p))


def checkout(p):
    """The top folder of this repo's checkout holding p (its realpath), or None (another repo, none)."""
    while p and not os.path.exists(p): p = os.path.dirname(p)
    r = subprocess.run(["git", "-C", p, "rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"],
                       capture_output=True, text=True)
    out = r.stdout.split("\n")
    if r.returncode or len(out) < 2 or os.path.realpath(out[0]) != common: return None
    return os.path.realpath(out[1])


def owner_file(top): return os.path.join(owners, hashlib.sha1(top.encode()).hexdigest())


def owner(top):
    """The subagent id recorded for top (made or claimed by it), else None."""
    try:
        with open(owner_file(top)) as f: return f.read().split("\n")[1] or None
    except (OSError, IndexError):
        return None


def record(top):
    """Records top as this subagent's (the main session's need no record)."""
    if not agent: return
    try:
        os.makedirs(owners, exist_ok=True)
        with open(owner_file(top), "w") as f: f.write("%s\n%s\n" % (top, agent))
    except OSError:
        pass


def proj_dir():
    """The project's transcripts folder (a subagent's transcript is <proj>/<session>/subagents/…)."""
    t = os.path.dirname(d.get("transcript_path") or "")
    return os.path.dirname(os.path.dirname(t)) if os.path.basename(t) == "subagents" else t


def live_sessions():
    """{session id: when the Claude Code process running it started (epoch seconds)} for each live
    process, from Claude Code's <config>/sessions/<pid>.json (removed as the process exits). A record
    whose pid is gone, or now another process's (its start time, ps's lstart in the C locale and UTC
    as Claude Code takes it, isn't the record's procStart), is stale; a live pid whose start ps can't
    tell counts. None without that folder: liveness can't be told."""
    reg = os.path.join(os.path.dirname(os.path.dirname(proj_dir())), "sessions")
    if not os.path.isdir(reg): return None
    live = {}
    for f in os.listdir(reg):
        if not f.endswith(".json"): continue
        try:
            with open(os.path.join(reg, f)) as h: r = json.load(h)
            s, pid, start = r["sessionId"], int(r["pid"]), (r.get("startedAt") or 0) / 1000
            os.kill(pid, 0)
        except PermissionError:
            pass   # (alive, another user's)
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            continue
        try:
            ps = subprocess.run(["/bin/ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True,
                                env={**os.environ, "TZ": "UTC", "LC_ALL": "C"}).stdout.split()
        except OSError:
            ps = []
        if ps and r.get("procStart") and ps != str(r["procStart"]).split(): continue
        live[s] = min(start, live.get(s, start))
    return live


def last_entry(j):
    """Transcript j's last entry with a timestamp (read from the end), else None."""
    try: f = open(j, "rb")
    except OSError: return None
    with f:
        end, buf = f.seek(0, 2), b""
        while end > 0:
            n = min(1 << 16, end); end -= n; f.seek(end); buf = f.read(n) + buf
            lines = buf.split(b"\n")
            buf = lines[0] if end else b""   # (maybe cut: read on)
            for l in reversed(lines[1:] if end else lines):
                try: e = json.loads(l)
                except ValueError: continue
                if isinstance(e, dict) and e.get("timestamp"): return e
    return None


def stamp(e):
    try: return datetime.fromisoformat(e["timestamp"].replace("Z", "+00:00")).timestamp()
    except (ValueError, AttributeError): return 0


def interrupted(e):
    """Whether entry e is Claude Code's mark of an interrupted agent (TaskStop's, a cut-off turn's)."""
    c = (e.get("message") or {}).get("content") if e.get("type") == "user" else None
    texts = [c] if isinstance(c, str) else [x.get("text") or "" for x in c or [] if isinstance(x, dict)]
    return any(t.startswith("[Request interrupted by user") for t in texts)


def running(a):
    """Whether subagent a runs: some copy of its files (<projects>/<project>/<session>/subagents/
    agent-<a>.*, in any project: a resumed or forked session copies its agents' files) is of a
    session a live Claude Code process runs, and that copy's transcript has an entry since the
    process started (one cut off by a restart has none), the last isn't an interruption (TaskStop's)
    and its meta doesn't say finished. An agent no transcript names runs nowhere this can see: False;
    without the transcripts folder (no transcript_path in the input) nothing can be told: True."""
    proj = proj_dir()
    if not (proj and os.path.isdir(proj)): return True
    live = live_sessions()
    for sub in {os.path.dirname(f) for f in glob.glob(os.path.join(os.path.dirname(proj), "*", "*", "subagents", "agent-%s.*" % a))}:
        base, s = os.path.join(sub, "agent-%s" % a), os.path.basename(os.path.dirname(sub))
        try:
            with open(base + ".meta.json") as f:
                if json.load(f).get("finished") is True: continue
        except (OSError, ValueError, AttributeError):
            pass
        start = 0 if live is None else live.get(s)
        if start is None: continue
        e = last_entry(base + ".jsonl")
        if e is None or (stamp(e) >= start and not interrupted(e)): return True
    return False


def agent_of(top):
    """The id of the subagent whose Claude Code worktree top is, else None."""
    m = re.match(r"agent-([0-9a-f]+)$", os.path.basename(top))
    if m and top == os.path.realpath(os.path.join(repo, ".claude", "worktrees", m.group(0))): return m.group(1)
    return None


def mine(top):
    """Whether this session may write to checkout top; claims a finished agent's worktree it may."""
    a, o = agent_of(top), owner(top)
    if a:
        if a == agent: return True
        if running(a): return False   # (its agent runs: its alone)
        if o and o != agent and running(o): return False   # (a running subagent took it)
        record(top); return True
    if o: return o == agent or (not agent and not running(o))   # (a subagent's own, while it runs)
    return not agent


def git(args, base, env):
    """Checks one git command (its words after `git`) run from folder base."""
    i, wt, gd = 0, env.get("GIT_WORK_TREE"), env.get("GIT_DIR")
    while i < len(args) and args[i].startswith("-"):
        a = args[i]
        if a == "-C" and i + 1 < len(args):
            base = path(args[i + 1], base, env); i += 2; continue
        for opt in ("--work-tree", "--git-dir"):
            if a == opt and i + 1 < len(args): a = "%s=%s" % (opt, args[i + 1]); i += 1
            if a.startswith(opt + "="):
                if opt == "--work-tree": wt = a.split("=", 1)[1]
                else: gd = a.split("=", 1)[1]
        i += 2 if a in GIT_TAKES else 1
    if i >= len(args): return
    sub, rest = args[i], args[i + 1:]
    opts, k = [], 0   # (the words not options; worktree add's -b/-B/--reason take one)
    while k < len(rest):
        if rest[k] in ("-b", "-B", "--reason") and sub == "worktree": k += 1
        elif not rest[k].startswith("-"): opts.append(rest[k])
        k += 1
    target, what = None, "git " + sub
    if sub == "worktree" and opts[:1] == ["add"] and len(opts) > 1:
        p = path(opts[1], base, env)
        if p and not os.path.lexists(p): record(os.path.realpath(p))   # (a new folder: the subagent's)
        return
    if sub == "worktree" and opts[:1] and opts[0] in ("remove", "move"):
        if len(opts) < 2: return
        target, what = opts[1], "git worktree " + opts[0]
    elif sub == "stash":
        if opts[:1] and opts[0] in STASH_READS: return
    elif sub not in WRITES:
        return
    if target is None:
        if wt: target = wt
        elif gd:
            g = path(gd, base, env)
            if g is None: block("Can't tell which checkout `%s` writes to (--git-dir %s): write the path out." % (what, gd))
            target = os.path.dirname(open(os.path.join(g, "gitdir")).read().strip()) if os.path.isfile(os.path.join(g, "gitdir")) else os.path.dirname(g)
        else: target = "."
    p = path(target, base, env)
    if p is None:
        block("Can't tell which checkout `%s` writes to (a variable, substitution or `cd -` in its path): write the path out." % what)
    top = checkout(p)
    if top is None: return
    if agent and what in ("git worktree remove", "git worktree move") and agent_of(top) not in (None, agent):
        block("`%s` of %s: a subagent's worktree is removed or moved by the main session (the PM) or its own agent only." % (what, top))
    if mine(top): return
    a = agent_of(top)
    why = ("its agent (%s) is still running" % a if a and running(a) else
           "a running subagent (%s) is working there" % owner(top) if a else
           "a subagent (%s) made it" % owner(top) if owner(top) else "it is the main session's (the PM's)")
    block("`%s` would write to %s, which isn't this session's: %s. A session writes with git only in its own checkouts: a subagent's worktree, a finished agent's that no running subagent works in, and the worktrees it made; the main session in the rest. To look at or test another commit, make a worktree of your own (`git worktree add <scratch folder> <commit>`) and work there." % (what, top, why))


def chdir(toks):
    """Where an `env -C <dir>`/`--chdir` prefix of the words runs the command (None: no such prefix),
    and the words with `--chdir <dir>` as one (command() takes only -C's value)."""
    out, where, k = [], None, 0
    while k < len(toks):
        t = toks[k]
        if os.path.basename(t) == "env":
            out.append(t); k += 1
            while k < len(toks) and (toks[k].startswith("-") or re.match(r"^\w+=", toks[k])):
                o = toks[k]
                if o in ("-C", "--chdir") and k + 1 < len(toks): where = toks[k + 1]; out += ["-C", where]; k += 2; continue
                if o.startswith("--chdir=") or (o.startswith("-C") and len(o) > 2):
                    where = o.split("=", 1)[1] if o.startswith("--") else o[2:]; out += ["-C", where]; k += 1; continue
                out.append(o); k += 1
            continue
        if not (re.match(r"^\w+=", t) or os.path.basename(t) in ("time", "nice", "command", "exec", "nohup")): break
        out.append(t); k += 1
    return where, out + toks[k:]


def scan(text, base, env, depth=0):
    if depth > 4: block("Commands nested this deep can't be checked: write the git command out plainly.")
    text, inner = substitutions(strip_heredocs(text), True)
    for t in inner: scan(t, base, env, depth + 1)
    stack = []
    for seg in segments(text):
        where, raw = chdir(tokens(seg))
        sets, toks = command(raw)
        if not toks:
            env = {**env, **sets}; continue
        c0, rest = os.path.basename(toks[0]), toks[1:]
        if c0 in ("cd", "pushd"):
            a = [r for r in rest if not r.startswith("-") or r == "-"]
            if c0 == "pushd": stack.append(base)
            base = None if a[:1] == ["-"] else path(a[0], base, env) if a else os.path.expanduser("~")
        elif c0 == "popd": base = stack.pop() if stack else None
        elif c0 == "export":
            env = {**env, **dict(r.split("=", 1) for r in rest if re.match(r"^\w+=", r))}
        elif c0 in ("sh", "bash", "zsh", "dash"):
            for k, t in enumerate(rest):
                if re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest): scan(rest[k + 1], base if where is None else path(where, base, env), env, depth + 1); break
        elif c0 == "eval": scan(" ".join(rest), base, env, depth + 1)
        elif c0 == "git": git(rest, base if where is None else path(where, base, env), {**env, **sets})


scan(cmd, cwd, {})
