#!/usr/bin/env python3
"""Which of check.sh's Rust parts an earlier pass vouches for (task #950; tools/dev/testing.md).

  check-reuse.py <check-passed file> <tests file>

Run by tools/dev/check.sh in the checkout it checks, clean (nothing untracked either), HEAD the
commit checked. <tests file>: the commands affected-tests.sh prints for it, one a line (empty: no
tests part). Prints one line a part, tab-separated:
  <part> run    -    <why>             run it
  <part> reuse  <P>  <why>             skip it: its pass at commit P holds here
  tests  partial <P> <why>             run only the `cmd` lines after it; P's pass holds for the rest
  tests  cmd    -    <command>
The parts are clippy and tests (the others take seconds and always run).

The rule. A pass at P, recorded with its base (`at`, the merge-base it was checked against), its
tree key and (tests) the hash of its test commands, holds for a part here only when
  - HEAD's tree is P's tree plus main's changes since P's base: with D the files that differ
    between P's base and HEAD's base, every file differing between P and HEAD is in D, and every
    file in D is HEAD's base's version in HEAD;
  - no file in D is unknown (its top-level entry outside KNOWN) and none is an input of the
    part: the Rust build's (BUILD and the workspace's package folders, the files their sources
    include_*!, #[path] or build scripts read, the scripts that run the part, RUNNER);
  - tests: HEAD's test commands are P's (the same hash), and the tests reading a file in D at run
    time (a `//! scans:` line, or a source's CARGO_MANIFEST_DIR or "../" path inside a #[test]
    function) are run again, those of them HEAD's commands run. A reading this can't place
    (outside a #[test] function, a path it can't resolve) runs the whole part.
Anything not shown safe (no such pass, a git lookup failing) runs the part.
"""
import hashlib, os, posixpath, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
# The Rust build's inputs besides its package folders: what cargo, rustc and clippy read.
BUILD = ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "mise.toml",
         ".mise.toml", "clippy.toml", ".clippy.toml", "rustfmt.toml", ".rustfmt.toml", ".cargo", ".config"]
# The scripts that run clippy and tests (check.sh's lines, the test set, the shim, the test Mac's run).
# tools/dev/test/check-reuse.zsh checks they run no other repo script.
RUNNER = ["tools/dev/check.sh", "tools/dev/check-reuse.py", "tools/dev/affected-tests.sh",
          "tools/dev/bin", "tools/app/remote-tests.sh"]
# Every top-level entry of the repo, so a file under a new one is unknown (doubt: both parts run).
# tools/dev/test/check-reuse.zsh checks every tracked file's entry is here.
KNOWN = set(BUILD) | {"docs", "web", "producers", "dem", "tools", "scripts", ".claude", "README.md",
                      ".gitignore", ".git-blame-ignore-revs"}


class Doubt(Exception):
    pass


def git(*a):
    r = subprocess.run(["git", *a], capture_output=True, text=True)
    if r.returncode:
        raise Doubt("git %s failed: %s" % (" ".join(a), r.stderr.strip()[:200]))
    return r.stdout


def names(*a):
    return set(filter(None, git("diff", "--no-renames", "--name-only", *a).split("\n")))


def under(path, prefix):
    return prefix == "" or path == prefix or path.startswith(prefix.rstrip("/") + "/")


def norm(base_dir, rel):
    """A repo path from a path relative to base_dir; None outside the repo."""
    p = posixpath.normpath(posixpath.join(base_dir, rel))
    if p == ".":
        return ""
    return None if p == ".." or p.startswith("../") or p.startswith("/") else p


def read(p):
    try:
        return open(p, encoding="utf-8", errors="replace").read()
    except OSError:
        return ""


def records(path):
    """check-passed's lines, newest first: (commit, {key: value}, reused)."""
    out = []
    for line in read(path).splitlines():
        t = line.split()
        if len(t) < 2:
            continue
        kv, reused, i = {}, [], 2
        while i + 1 < len(t):
            if t[i] == "reused":
                reused.append(t[i + 1])
            else:
                kv[t[i]] = t[i + 1]
            i += 2
        out.append((t[0], kv, reused))
    return out[::-1]


# ---- the Rust build's inputs, from the sources ----

def packages(files):
    """The workspace's package folders: its members, excluded crates and path dependencies."""
    root = read("Cargo.toml")
    dirs = []
    for key in ("members", "exclude"):
        m = re.search(r"^%s\s*=\s*\[([^\]]*)\]" % key, root, re.M)
        for g in re.findall(r'"([^"]+)"', m.group(1)) if m else []:
            if "*" not in g:
                dirs.append(g); continue
            if not g.endswith("/*") or "*" in g[:-2]:
                raise Doubt("Cargo.toml's %s has a glob this doesn't read: %s" % (key, g))
            pre = g[:-1]
            dirs += sorted({pre + f[len(pre):].split("/")[0] for f in files if f.startswith(pre) and "/" in f[len(pre):]})
    seen, todo = [], list(dirs)
    while todo:
        d = todo.pop()
        if d in seen:
            continue
        seen.append(d)
        for rel in re.findall(r'\bpath\s*=\s*"([^"]+)"', read(d + "/Cargo.toml")):
            p = norm(d, rel)
            if p is None:
                raise Doubt("%s/Cargo.toml names a path outside the repo: %s" % (d, rel))
            todo.append(p)
    return sorted(seen)


INCLUDE = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*("([^"\\]*)")?')
PATHATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def build_inputs(files, pkgs):
    """(prefix, why) for every repo path the build reads."""
    ins = [(p, "the Rust build") for p in BUILD + pkgs] + [(p, "runs the part") for p in RUNNER]
    for f in files:
        if not f.endswith(".rs") or not any(under(f, p) for p in pkgs):
            continue
        src, d = read(f), posixpath.dirname(f)
        for m in INCLUDE.finditer(src):
            if not m.group(1):
                raise Doubt("%s includes a path that isn't a literal" % f)
            p = norm(d, m.group(2))
            if p is None:
                raise Doubt("%s includes a file outside the repo: %s" % (f, m.group(2)))
            ins.append((p, "%s includes it" % f))
        for rel in PATHATTR.findall(src):
            p = norm(d, rel)
            if p is None:
                raise Doubt("%s has a #[path] outside the repo: %s" % (f, rel))
            ins.append((p, "%s's #[path]" % f))
        if posixpath.basename(f) == "build.rs" and posixpath.dirname(f) in pkgs:
            for p, _ in reads(f, src):
                ins.append((p, "%s (a build script) reads it" % f))
    return ins


# ---- the tests' reads at run time ----

def reads(f, src):
    """(repo prefix, offset) for each repo path f's code reads at run time: a CARGO_MANIFEST_DIR
    joined with a literal (none: the whole repo), or a "../" literal from the crate's folder (a
    test's working folder). Paths outside the repo are left out (a test's "../../etc/passwd")."""
    crate = "/".join(f.split("/")[:2])
    out = []
    for m in re.finditer(r'CARGO_MANIFEST_DIR|"(\.\./[^"\\]*)"', src):
        a, b = src.rfind("\n", 0, m.start()) + 1, src.find("\n", m.start())
        if INCLUDE.search(src[a: b if b >= 0 else len(src)]) or PATHATTR.search(src[a: b if b >= 0 else len(src)]):
            continue   # (the build's inputs)
        if m.group(1) is None:   # joined at once with a literal, or else the whole repo
            j = re.match(r'"\s*\)\s*\)\s*\.join\(\s*"([^"\\]*)"\s*\)', src[m.end():])
            p = norm(crate, j.group(1)) if j else ""
        else:
            p = norm(crate, m.group(1))
        if p is not None and (p, m.start()) not in out:
            out.append((p, m.start()))
    return out


def blank(src):
    """src with comments, strings and char literals as spaces (offsets and newlines kept)."""
    out, i, n = list(src), 0, len(src)
    char = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F]{1,6}\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'")
    raw = re.compile(r'b?r(#*)"')

    def wipe(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            wipe(i, j); i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j): depth += 1; j += 2
                elif src.startswith("*/", j): depth -= 1; j += 2
                else: j += 1
            wipe(i, j); i = j
        elif (m := raw.match(src, i)) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            end = src.find('"' + m.group(1), m.end())
            j = n if end < 0 else end + 1 + len(m.group(1))
            wipe(i, j); i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            wipe(i, j + 1); i = j + 1
        elif c == "'" and (m := char.match(src, i)):
            wipe(i, m.end()); i = m.end()
        else:
            i += 1
    return "".join(out)


def enclosing(src, at):
    """(in-file module names, the #[test] function holding offset at), or None when the innermost
    function holding it isn't a #[test] one (or none does)."""
    b = blank(src)
    stack, pending, last = [], None, 0
    for m in re.finditer(r"\b(fn|mod)\s+(\w+)|[{};]", b):
        if m.start() >= at:
            break
        t = m.group(0)
        if m.group(1):
            pending = (m.group(1), m.group(2), b[last: m.start()])
        elif t == "{":
            stack.append(pending); pending = None; last = m.end()
        elif t == "}":
            if stack: stack.pop()
            pending = None; last = m.end()
        else:
            pending = None; last = m.end()
    fns = [s for s in stack if s and s[0] == "fn"]
    if not fns or not re.search(r"#\[\s*(?:\w+::)*test\s*[\](]", fns[-1][2]):
        return None
    return [s[1] for s in stack if s and s[0] == "mod"], fns[-1][1]


SCANS = re.compile(r"^//! scans: \S", re.M)


def units_of(f, at, src, files):
    """The test units [(crate, target, filter or None: the whole target)] a read at offset at of
    f runs in. An integration test with a `//! scans:` line: none (that line says what it reads)."""
    parts = f.split("/")
    if len(parts) < 4 or parts[0] != "crates":
        raise Doubt("%s reads the repo, outside a crates/<crate>/ package" % f)
    c, kind, rel = parts[1], parts[2], parts[3:]
    line = src.count("\n", 0, at) + 1
    if kind == "tests":
        tdir = "crates/%s/tests/" % c
        if len(rel) == 1:   # a test of its own
            return [] if SCANS.search(src) else [(c, "--test " + rel[0][:-3], None)]
        # A helper folder's (tests/common/): every test declaring it without a scans line.
        decl = re.compile(r"^\s*mod\s+%s\s*;" % re.escape(rel[0]), re.M)
        return [(c, "--test " + posixpath.basename(t)[:-3], None) for t in files
                if t.startswith(tdir) and t.count("/") == 3 and t.endswith(".rs")
                and decl.search(t_src := read(t)) and not SCANS.search(t_src)]
    if kind != "src":
        return []   # examples, benches: cargo test builds them, runs none of them
    for g in files:
        if g.startswith("crates/%s/src/" % c) and g.endswith(".rs"):
            if any(posixpath.basename(x) == posixpath.basename(f) for x in PATHATTR.findall(read(g))):
                raise Doubt("%s may be a #[path] module (%s)" % (f, g))
    e = enclosing(src, at)
    if e is None:
        raise Doubt("%s:%d reads the repo outside a #[test] function" % (f, line))
    mods, fn = e
    if rel[0] == "bin":
        if len(rel) != 2:
            raise Doubt("%s:%d is in a binary's module" % (f, line))
        return [(c, "--bin " + rel[1][:-3], "::".join(mods + [fn]))]
    path = "/".join(rel)[:-3].split("/")
    if path[-1] == "mod": path = path[:-1]
    if path in (["lib"], ["main"]): path = []
    tgt = "--lib" if os.path.exists("crates/%s/src/lib.rs" % c) else "--bin " + c
    return [(c, tgt, "::".join(path + mods + [fn]))]


def parse_cmds(lines):
    """affected-tests.sh's commands as {(crate, target): filters or None (the whole target)}."""
    units = {}
    for line in lines:
        t = line.split()
        if t[:3] != ["cargo", "test", "-p"] or len(t) < 5:
            raise Doubt("can't read the test command %r" % line)
        c, i, tgts = t[3], 4, []
        while i < len(t) and t[i] != "--":
            if t[i] == "--lib": tgts.append("--lib"); i += 1
            elif t[i] in ("--bin", "--test", "--example") and i + 1 < len(t): tgts.append(t[i] + " " + t[i + 1]); i += 2
            else: raise Doubt("can't read the test command %r" % line)
        flt = t[i + 1:] if i < len(t) else None
        if flt is not None and (not flt or any(x.startswith("-") for x in flt)):
            raise Doubt("can't read the test command %r" % line)
        for g in tgts:
            old = units.get((c, g), [])
            units[(c, g)] = None if flt is None or old is None else old + flt
    return units


def commands(units):
    whole, out = {}, []
    for (c, g), fs in units.items():
        if fs is None: whole.setdefault(c, []).append(g)
    out += ["cargo test -p %s %s" % (c, " ".join(gs)) for c, gs in whole.items()]
    out += ["cargo test -p %s %s -- %s" % (c, g, " ".join(sorted(set(fs)))) for (c, g), fs in units.items() if fs is not None]
    return out


def intersect(hits, ours):
    """The hit units that our commands run (cargo filters match by substring)."""
    run = {}
    for k, hf in hits.items():
        if k not in ours: continue
        of = ours[k]
        if hf is None or of is None:
            run[k] = of if hf is None else hf
            continue
        got = [h for h in hf if any(o in h for o in of)] + [o for o in of if any(h in o for h in hf)]
        if got: run[k] = sorted(set(got))
    return run


def short(xs, n=4):
    xs = sorted(xs)
    return ", ".join(xs[:n]) + (" and %d more" % (len(xs) - n) if len(xs) > n else "")


def decide(passed, tests_file):
    tests = [l for l in read(tests_file).splitlines() if l.strip()]
    th = hashlib.sha1(read(tests_file).encode()).hexdigest()
    if git("status", "--porcelain"):
        raise Doubt("the checkout isn't clean")
    head, base = git("rev-parse", "HEAD").strip(), git("merge-base", "main", "HEAD").strip()
    cands = [r for r in records(passed) if r[0] != head and "at" in r[1] and "tree" in r[1]]
    if not cands:
        raise Doubt("no earlier pass recorded with its base")
    pick, tried = None, 0
    for p, kv, reused in cands:
        tried += 1
        try:
            if git("rev-parse", p + "^{tree}").strip() != kv["tree"]: continue
            d = names(kv["at"], base)
            if not names(p, head) <= d or names(base, head) & d: continue
        except Doubt:
            continue
        pick = (p, kv, d); break
    if not pick:
        raise Doubt("no earlier pass (of %d) whose tree plus main's changes since its base is this tree" % tried)
    p, kv, d = pick
    sp = p[:7]
    how = "this tree is %s's plus main's changes since its base %s (%s)" % (
        sp, kv["at"][:7], "%d files: %s" % (len(d), short(d)) if d else "none")
    unknown = [f for f in d if f.split("/")[0] not in KNOWN]
    if unknown:
        raise Doubt("main changed files of no known kind: " + short(unknown))
    files = git("ls-files").split("\n")
    ins = build_inputs(files, packages(files))
    hit = {f: why for f in d for pre, why in ins if under(f, pre)}
    out = []
    if hit:
        why = "main changed Rust build inputs since %s: %s" % (sp, short("%s (%s)" % (f, w) for f, w in hit.items()))
        out.append(("clippy", "run", "-", why))
        if tests: out.append(("tests", "run", "-", why))
        return out
    out.append(("clippy", "reuse", p, how + "; none is a Rust build input"))
    if not tests:
        return out
    if kv.get("tests") != th:
        out.append(("tests", "run", "-", "the affected tests here aren't %s's" % sp))
        return out
    try:
        out += tests_part(tests, d, files, dict(kv, commit=p), base, how + "; none is a Rust build input")
    except Doubt as e:
        out.append(("tests", "run", "-", str(e)))
    return out


def tests_part(tests, d, files, kv, base, how):
    p = kv["commit"]
    ours = parse_cmds(tests)
    # Reads at run time: the scan tests' lines (affected-tests.sh over main's changes), then sources.
    r = subprocess.run([HERE + "/affected-tests.sh", "%s..%s" % (kv["at"], base)], capture_output=True, text=True)
    if r.returncode:
        raise Doubt("affected-tests.sh over main's changes failed: " + r.stderr.strip()[:200])
    hits = parse_cmds([l for l in r.stdout.splitlines() if l and not l.startswith("#")])
    why = ["%s (scans)" % k[1] for k in hits]
    for f in files:
        if not (f.startswith("crates/") and f.endswith(".rs")):
            continue
        src = read(f)
        for pre, at in reads(f, src):
            touched = [x for x in d if under(x, pre)]
            if not touched: continue
            for c, g, flt in units_of(f, at, src, files):
                if (c, g) in hits and hits[(c, g)] is None: continue
                hits[(c, g)] = None if flt is None else hits.get((c, g), []) + [flt]
                why.append("%s (%s reads %s)" % (flt or g, f, short(touched, 2)))
    run = intersect(hits, ours)
    if not run:
        return [("tests", "reuse", p, how + ("; the tests reading them at run time (%s) aren't run here" % short(why)
                                              if why else "; no test reads them at run time"))]
    return [("tests", "partial", p, how + "; run again, reading them at run time: " + short(why, 6))] + \
        [("tests", "cmd", "-", c) for c in commands(run)]


def main():
    if len(sys.argv) != 3 or sys.argv[1].startswith("-"):
        print(__doc__.split("\n\n")[0], file=sys.stderr); sys.exit(2)
    os.chdir(git("rev-parse", "--show-toplevel").strip())
    try:
        out = decide(sys.argv[1], sys.argv[2])
    except Doubt as e:
        out = [("clippy", "run", "-", str(e)), ("tests", "run", "-", str(e))]
    for o in out:
        print("\t".join(o))


if __name__ == "__main__":
    main()
