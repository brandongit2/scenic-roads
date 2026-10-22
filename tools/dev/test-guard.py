# tools/dev/test-guard.sh's checks (see there). Prints the reason to block, or nothing.
import json, sys, os, re, shlex, subprocess
d = json.load(sys.stdin)
tool, ti, cwd, agent = d.get("tool_name"), d.get("tool_input") or {}, d.get("cwd") or "", d.get("agent_id")
repo = os.path.dirname(subprocess.check_output(["git", "-C", os.path.dirname(os.path.abspath(__file__)), "rev-parse", "--path-format=absolute", "--git-common-dir"], text=True).strip())   # the main checkout
RULES = " See tools/dev/testing.md."
def block(m): print(m + RULES); sys.exit()
if tool == "Agent":
    p = ti.get("prompt") or ""
    if not agent and re.search(r"\btests?\b|cargo test", p) and "tools/dev/testing.md" not in p:
        print("Agent brief mentions tests but not the testing rules: add \"Follow tools/dev/testing.md for running tests.\" to the prompt.")
    elif not agent and re.search(r"\bcommit", p) and "tools/dev/check.sh" not in p:
        print("Agent brief asks for commits but not the check: add \"Run tools/dev/check.sh in your worktree before reporting; report only after it prints PASS.\" to the prompt.")
    sys.exit()
if tool != "Bash" or not agent: sys.exit()
cmd = ti.get("command") or ""
repro, perf = "#repro" in cmd, "#perf" in cmd

# --- Finding the test runs: commands, not text. A command starts at the start, after an unquoted
# ; & | ( ) or newline, or after a prefix (env, time, nice, command, xargs, exec, nohup). Quoted
# text is looked into only as the script of sh/bash/zsh -c, eval, or python -c.
def strip_heredocs(t):
    return re.sub(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n.*?\n\s*\2[ \t]*(?=\n|$)", "<<HEREDOC", t, flags=re.S)
def segments(t):
    out, cur, q, i = [], [], None, 0
    while i < len(t):
        ch = t[i]
        if q:
            cur.append(ch)
            if ch == "\\" and q == '"' and i + 1 < len(t): cur.append(t[i + 1]); i += 1
            elif ch == q: q = None
        elif ch == "\\" and i + 1 < len(t): cur.append(ch); cur.append(t[i + 1]); i += 1
        elif ch in "'\"": q = ch; cur.append(ch)
        elif ch == "&" and ((cur and cur[-1] in "<>") or t[i + 1:i + 2] == ">"): cur.append(ch)   # 2>&1, &>
        elif ch in ";&|()\n": out.append("".join(cur)); cur = []
        else: cur.append(ch)
        i += 1
    out.append("".join(cur))
    return [s for s in out if s.strip()]
def tokens(seg):
    try: return shlex.split(seg, comments=True)
    except ValueError: return seg.split()
PREFIX_TAKES = {"nice": {"-n"}, "env": {"-u", "-C", "-S"}, "xargs": {"-I", "-n", "-L", "-P", "-d", "-E", "-s"}, "time": set(), "command": set(), "exec": set(), "nohup": set()}
CARGO_TAKES = {"-C", "--config", "-Z", "--color", "--manifest-path", "--target-dir", "--explain"}
found = []   # argument lists of cargo test / nextest runs
def scan(text, depth=0):
    if depth > 4: block("Commands nested this deep can't be checked: write the cargo test command out plainly.")
    for seg in segments(strip_heredocs(text)):
        toks = tokens(seg)
        while toks and re.match(r"^\w+=", toks[0]): toks = toks[1:]
        while toks and os.path.basename(toks[0]) in PREFIX_TAKES:
            takes = PREFIX_TAKES[os.path.basename(toks[0])]; toks = toks[1:]
            while toks and (toks[0].startswith("-") or re.match(r"^\w+=", toks[0])):
                toks = toks[2:] if toks[0] in takes else toks[1:]
        if not toks: continue
        c0, rest = os.path.basename(toks[0]), toks[1:]
        if c0 in ("sh", "bash", "zsh", "dash"):
            for k, t in enumerate(rest):
                if re.match(r"^-\w*c\w*$", t) and k + 1 < len(rest):
                    inner = rest[k + 1]
                    if re.search(r"\$|`", inner) and re.search(r"\b(t|test|nextest)\b", inner) and "cargo" in inner:
                        block("A cargo test inside %s -c built from variables or substitutions can't be checked: write it out plainly." % c0)
                    scan(inner, depth + 1); break
        elif c0 == "eval": scan(" ".join(rest), depth + 1)
        elif c0.startswith("python") and "-c" in rest:
            k = rest.index("-c")
            if k + 1 < len(rest):
                for _, s in re.findall(r"(['\"])(.*?)\1", rest[k + 1]): scan(s, depth + 1)
        elif c0 in ("make", "just", "cargo-make") and any(t in ("test", "tests", "check-all") for t in rest):
            block("%s %s runs tests the guard can't see: run the cargo test commands directly." % (c0, " ".join(rest)))
        elif toks[0].startswith("$") and rest and rest[0] in ("t", "test", "nextest"):
            block("A test run through a variable (%s) can't be checked: write cargo out." % toks[0])
        elif c0 == "cargo":
            i, conf = 0, []
            while i < len(rest) and rest[i].startswith(("-", "+")):
                if rest[i] in CARGO_TAKES and i + 1 < len(rest):
                    if rest[i] == "--config": conf.append(rest[i + 1])
                    if rest[i] == "-C": conf.append("-C")
                    i += 2
                else:
                    if rest[i].startswith("--config="): conf.append(rest[i])
                    i += 1
            sub = rest[i:]
            if any("alias" in c for c in conf): block("cargo --config alias.… can't be checked: run cargo test itself.")
            if sub and sub[0] in ("t", "test"): args = sub[1:]
            elif len(sub) > 1 and sub[0] == "nextest" and sub[1] in ("r", "run"): args = sub[2:]
            else: continue
            if "-C" in conf: block("cargo -C <dir> test: run it from the checkout's root, naming the crate with -p.")
            found.append(args)
        elif c0 == "cargo-nextest":
            sub = rest[1:] if rest[:1] == ["nextest"] else rest
            if sub and sub[0] in ("r", "run"): found.append(sub[1:])
scan(cmd)

TAKES = {"-p","--package","--test","--bin","--features","-F","-j","--jobs","--target-dir","--manifest-path","--profile",
         "--example","--bench","--exclude","--target","--color","--message-format","-Z","--config","--partition","--retries",
         "--test-threads","--threads","--skip","--format","--logfile","--report-time","--shuffle-seed","-E","--filterset",
         "--filter-expr","--cargo-profile","--run-ignored","--final-status-level","--status-level","--failure-output",
         "--success-output","--tool-config-file","--archive-file","--workspace-remap","-P"}
def parse(toks):
    """(crates, targets [(flag, name)], filters, flags) of a cargo test / nextest run's arguments."""
    toks = [t for t in toks if not re.match(r"^\d*[<>&]", t)]   # redirections
    crates, targets, filters, flags, i = [], [], [], set(), 0
    while i < len(toks):
        t = toks[i]
        if t == "--": i += 1; continue
        if "=" in t and t.startswith("--"): k, v = t.split("=", 1); toks[i:i+1] = [k, v]; t = k
        if t in ("-p","--package") and i+1 < len(toks): crates.append(toks[i+1])
        elif t in ("--test","--bin") and i+1 < len(toks): targets.append((t, toks[i+1]))
        elif t == "--lib": targets.append(("--lib", ""))
        elif t.startswith("-"): flags.add(t)
        else: filters.append(t)
        if t in TAKES: i += 2; continue
        i += 1
    return crates, targets, filters, flags

wt = cwd if "/.claude/worktrees/" in cwd else None
root = (wt.split("/.claude/worktrees/")[0] + "/.claude/worktrees/" + wt.split("/.claude/worktrees/")[1].split("/")[0]) if wt else repo
def crate_dirs(): return sorted(os.listdir(os.path.join(root, "crates")))
def is_module(c, seg):
    s = os.path.join(root, "crates", c, "src")
    return seg not in ("tests", "lib", "main") and (os.path.exists(os.path.join(s, seg + ".rs")) or os.path.isdir(os.path.join(s, seg)))
for args in found:
    crates, targets, filters, flags = parse(args)
    if ("--workspace" in flags or "--all" in flags) and not (repro or perf):
        block("--workspace runs every crate: name the crate with -p (only #repro or #perf may use it).")
    if ("--release" in flags or "-r" in flags or ("--profile" in args and "release" in args) or ("--cargo-profile" in args and "release" in args)) and not perf:
        block("Tests in --release only for performance work (mark the command #perf).")
    if repro: continue
    for t, name in targets:
        if t == "--test" and not any(os.path.exists(os.path.join(root, "crates", c, "tests", name + ".rs")) for c in (crates or crate_dirs())):
            block("No integration test %s in %s." % (name, " ".join(crates) or "any crate"))
    single = targets and all(t != "--lib" for t, _ in targets)   # one test or binary: its own names may be bare
    if not single:
        for f in filters:
            seg = f.split("::")[0]
            if "::" not in f or not any(is_module(c, seg) for c in (crates or crate_dirs())):
                block("Filter %s isn't a module path: a filter starts with a module of the crate (agent::tests::, agent::tests::unit_key); a bare word, tests:: or :: matches almost every test." % f)
    full = not filters and not [t for t in targets if t[0] in ("--test", "--bin")]
    if full:
        block("No full test suites from agents or reviewers: run the tests tools/dev/affected-tests.sh prints for your worktree (a reviewer: only the tests the commit adds or changes, filtered). A full suite only for a failure the PM names: mark it #repro. publish.sh runs everything.")
    if not wt: continue   # a reviewer in the main checkout: filtered runs are theirs to pick
    r = subprocess.run([repo + "/tools/dev/affected-tests.sh"], cwd=root, capture_output=True, text=True, timeout=30)
    allowed = {}   # (crate, target) -> None (whole) | set of filter prefixes
    for line in r.stdout.splitlines():
        if not line.startswith("cargo test "): continue
        c, ts, fs, _ = parse(shlex.split(line)[2:])
        key = (c[0], ts[0])
        allowed[key] = None if not fs else (allowed.get(key) or set()) | set(fs)
    why = None
    bad = [f for f in ("--workspace", "--all", "--bins", "--tests", "--all-targets", "--doc", "--examples", "--benches", "-E", "--filterset", "--filter-expr", "--ignored", "--include-ignored", "--run-ignored") if f in flags]
    if bad: why = "%s widens the run" % bad[0]
    elif not crates: why = "name the crate with -p"
    for c in crates:
        if why: break
        tg = targets or [k[1] for k in allowed if k[0] == c and k[1][0] != "--test"]
        if not tg: why = "nothing in crate %s is affected" % c; break
        for t in tg:
            if (c, t) not in allowed: why = "-p %s %s isn't affected" % (c, " ".join(t).strip()); break
            a = allowed[(c, t)]
            if a is None: continue
            if not filters: why = "-p %s %s whole, where the affected set is filtered" % (c, " ".join(t).strip()); break
            off = [f for f in filters if not any(f.startswith(x) for x in a)]
            if off: why = "filter %s isn't under one the script prints for -p %s %s" % (off[0], c, " ".join(t).strip()); break
    if why:
        block("This cargo test is outside the affected set (%s). Run tools/dev/affected-tests.sh in your worktree and run what it prints, or narrower: the same -p and target, filters starting with one it prints (the full path, e.g. agent::tests::unit_key). A failure the PM names may be reproduced with #repro." % why)
