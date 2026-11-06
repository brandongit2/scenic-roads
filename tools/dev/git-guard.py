# tools/dev/shell-guard.sh's git check (task #980): a session writes with git only in its own
# checkouts. Reads the PreToolUse input on stdin; prints the reason to block, or nothing.
#
# A git write (WRITES, below; `stash` but its list/show/drop/clear/create/store) goes to the
# checkout its command names: `git -C <dir>`, `--work-tree`, `--git-dir` (a worktree's admin
# folder: that worktree), or the folder a `cd` earlier in the command line went to, else the
# session's cwd; `git worktree remove|move <path>` to <path>. Only checkouts of this repo are
# looked at (another repo, a folder that isn't one: no concern of this). Its own are:
# - a subagent (agent_id in the input): its worktree, .claude/worktrees/agent-<agent_id>, which
#   Claude Code made for it; an agent without one has none;
# - the main session (the PM): the main checkout and the checkout it started in (its transcript's
#   first cwd);
# - either: the worktrees it made itself: a `git worktree add <path>` this sees, of a path not yet
#   there, records it as the session's (.git/worktree-owners/, the path's hash);
# - the main session may also `git worktree remove` a subagent's worktree once Claude Code says that
#   agent finished (its <session>/subagents/agent-<id>.meta.json in the project's transcripts).
# A write whose folder can't be told (a variable or substitution in the path) is refused.
# Commands are read as test-guard.py reads them (shellwords.py), the scripts of sh/bash/zsh -c and
# eval too; what a script run by the command does is not seen.
import hashlib, json, os, re, subprocess, sys
from shellwords import strip_heredocs, segments, tokens, command

WRITES = {"reset", "checkout", "switch", "restore", "commit", "rebase", "merge", "cherry-pick", "revert",
          "am", "pull", "clean", "add", "rm", "mv", "apply", "read-tree", "update-index", "checkout-index"}
STASH_READS = {"list", "show", "drop", "clear", "create", "store"}
GIT_TAKES = {"-c", "--namespace", "--exec-path", "--config-env", "--super-prefix", "--list-cmds", "--attr-source"}

d = json.load(sys.stdin)
if d.get("tool_name") != "Bash": sys.exit()
cmd = (d.get("tool_input") or {}).get("command") or ""
if "git" not in cmd: sys.exit()
cwd, agent, session = d.get("cwd") or os.getcwd(), d.get("agent_id") or "", d.get("session_id") or ""
here = os.path.dirname(os.path.abspath(__file__))
common = os.path.realpath(subprocess.check_output(["git", "-C", here, "rev-parse", "--path-format=absolute", "--git-common-dir"], text=True).strip())
repo = os.path.dirname(common)   # the main checkout
owners = os.path.join(common, "worktree-owners")
me = "%s %s" % (session, agent or "-")


def block(m):
    print(m + " (tools/dev/git-guard.py; tools/dev/testing.md, \"Whose checkout\")"); sys.exit()


def path(p, base, env):
    """p as an absolute path from base (None: unknown), or None when the shell's expansion can't be told."""
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


def start_dir():
    """The folder the main session started in: its transcript's first cwd."""
    try:
        with open(d.get("transcript_path") or "", encoding="utf-8", errors="replace") as f:
            for n, line in enumerate(f):
                m = re.search(r'"cwd":\s*"((?:[^"\\]|\\.)*)"', line)
                if m: return json.loads('"%s"' % m.group(1))
                if n > 200: break
    except OSError:
        pass
    return None


def finished_agent(top):
    """The subagent whose worktree top is, when a transcript of this project says it finished."""
    m = re.match(r"agent-([0-9a-f]+)$", os.path.basename(top))
    t = d.get("transcript_path") or ""
    if not m or top != os.path.realpath(os.path.join(repo, ".claude", "worktrees", os.path.basename(top))) or not t:
        return False
    proj = os.path.dirname(t)
    for s in os.listdir(proj) if os.path.isdir(proj) else []:
        try:
            with open(os.path.join(proj, s, "subagents", "agent-%s.meta.json" % m.group(1))) as f:
                if json.load(f).get("finished") is True: return True
        except (OSError, ValueError):
            pass
    return False


def own():
    tops = []
    if agent:
        tops.append(os.path.join(repo, ".claude", "worktrees", "agent-" + agent))
    else:
        tops.append(repo)
        s = start_dir()
        if s: tops.append(checkout(s) or "")
    return {os.path.realpath(t) for t in tops if t}


def mine(top):
    if top in own(): return True
    try:
        with open(owner_file(top)) as f: return f.read().split("\n")[1] == me
    except (OSError, IndexError):
        return False


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
        if p and not os.path.lexists(p):   # (a new folder: the session's from now on)
            try:
                os.makedirs(owners, exist_ok=True)
                with open(owner_file(os.path.realpath(p)), "w") as f: f.write("%s\n%s\n" % (p, me))
            except OSError:
                pass
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
    if top is None or mine(top): return
    if what == "git worktree remove" and not agent and finished_agent(top): return
    whose = "your own worktree (%s)" % os.path.join(repo, ".claude", "worktrees", "agent-" + agent) if agent else "the main checkout, the checkout you started in, or a worktree you made"
    block("`%s` would write to %s, which isn't this session's: an agent writes with git only in %s. To look at or test another commit, make a worktree of your own (`git worktree add <scratch folder> <commit>`) and work there; never reset, check out or commit in another agent's worktree." % (what, top, whose))


def scan(text, base, env, depth=0):
    if depth > 4: block("Commands nested this deep can't be checked: write the git command out plainly.")
    for seg in segments(strip_heredocs(text)):
        sets, toks = command(tokens(seg))
        if not toks:
            env = {**env, **sets}; continue
        c0, rest = os.path.basename(toks[0]), toks[1:]
        if c0 in ("cd", "pushd"):
            a = [r for r in rest if not r.startswith("-") or r == "-"]
            base = None if a[:1] == ["-"] else path(a[0], base, env) if a else os.path.expanduser("~")
        elif c0 in ("sh", "bash", "zsh", "dash"):
            for k, t in enumerate(rest):
                if re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest): scan(rest[k + 1], base, env, depth + 1); break
        elif c0 == "eval": scan(" ".join(rest), base, env, depth + 1)
        elif c0 == "git": git(rest, base, {**env, **sets})


scan(cmd, cwd, {})
