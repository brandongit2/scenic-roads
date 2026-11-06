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
The parts are clippy and tests (the others are never reused).

The rule. A pass at P, recorded with its base (`at`, the merge-base it was checked against), its
tree key and (tests) the hash of its test commands (the commands kept in check-tests/<hash> beside
the record), is weighed against X, the files differing between P's tree and HEAD's. X's files
are main's (main changed it since P's base and HEAD has main's version) or the branch's own (an
edit since P, #969). Of the passes recorded, the one with the fewest own files (then the fewest
in X) is weighed. It holds for a part here only when
  - no file in X is unknown (its top-level entry outside KNOWN);
  - clippy: none is an input of the Rust build (BUILD and the package folders: the workspace's
    members, excludes and the paths its manifests name; the files their sources include_*!,
    #[path] or name as a macro's first "../" literal; what build scripts and proc macros read; the
    scripts that run the part, RUNNER);
  - tests: none of main's is such an input, and each own one that is, is a module's source (a .rs
    file under crates/<crate>/src or tests, not a crate's root, build script or proc macro) or a
    file include_str!d or include_bytes!d only inside #[test] functions (those tests), nothing
    include!ing it, naming it by #[path] or in a macro; then run again: the tests HEAD's commands run that P's didn't, and of those
    both run, the tests affected-tests.sh gives for P..HEAD (the plain sources' modules traced
    as for the branch, the scan tests whose `//! scans:` line covers a file in X) and the tests
    reading a file in X at run time (a source's CARGO_MANIFEST_DIR or ".." path inside a #[test]
    function). A reading this can't place (outside a #[test] function, in a macro, a crate whose
    targets aren't cargo's defaults) runs the whole part; a path whose base can't be told is the
    whole repo.
Doubt runs both parts: a symlink in the repo, an include! or #[path] outside the package folders
(whose own modules aren't followed), a path a manifest or .cargo names outside them.
Anything not shown safe (no such pass, a git lookup failing) runs the part.
"""
import hashlib, os, posixpath, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
# The Rust build's inputs besides its package folders: what cargo, rustc and clippy read.
BUILD = ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "mise.toml",
         ".mise.toml", "clippy.toml", ".clippy.toml", "rustfmt.toml", ".rustfmt.toml", ".cargo", ".config"]
# The scripts that run clippy and tests (check.sh's lines, the test set, the shim, the test Mac's run, the
# dev jobs' recorder wrapping them).
# tools/dev/test/check-reuse.zsh checks they run no other repo script.
RUNNER = ["tools/dev/check.sh", "tools/dev/check-reuse.py", "tools/dev/affected-tests.sh",
          "tools/dev/bin", "tools/app/remote-tests.sh", "tools/dev/devjob.zsh", "tools/dev/devjob.py"]
# Every top-level entry of the repo, so a file under a new one is unknown (doubt: both parts run).
# tools/dev/test/check-reuse.zsh checks every tracked file's entry is here.
KNOWN = set(BUILD) | {"docs", "web", "producers", "dem", "tools", "scripts", ".claude", "README.md",
                      ".gitignore", ".git-blame-ignore-revs"}


# How many of the newest passes are weighed (each a tree diff against HEAD's).
CANDIDATES = 60


class Doubt(Exception):
    pass


def git(*a):
    r = subprocess.run(["git", *a], capture_output=True, text=True)
    if r.returncode:
        raise Doubt("git %s failed: %s" % (" ".join(a), r.stderr.strip()[:200]))
    return r.stdout


def names(*a):
    return set(filter(None, git("diff", "--no-renames", "--name-only", "-z", *a).split("\0")))


def under(path, prefix):
    """Is path under prefix? A prefix "*/<p>": under <p> from any of path's folders."""
    if prefix.startswith("*/"):
        parts = path.split("/")
        return any(under("/".join(parts[i:]), prefix[2:]) for i in range(len(parts)))
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
    """check-passed's lines, newest first: (commit, {key: value}), its `reused` left out."""
    out = []
    for line in read(path).splitlines():
        t = line.split()
        if len(t) >= 2:
            out.append((t[0], {k: v for k, v in zip(t[2::2], t[3::2]) if k != "reused"}))
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
    # The root manifest's own paths: [workspace.dependencies], [patch].
    for rel in re.findall(r'\bpath\s*=\s*"([^"]+)"', root):
        p = norm("", rel)
        if p is None:
            raise Doubt("Cargo.toml names a path outside the repo: %s" % rel)
        dirs.append(p)
    for c in (".cargo/config", ".cargo/config.toml"):
        if re.search(r"\bpaths?\s*=", read(c)):
            raise Doubt("%s names paths, which this doesn't follow" % c)
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


INCLUDE = re.compile(r'\binclude(_str|_bytes)?!\s*\(\s*("([^"\\]*)")?')
PATHATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
# A "../" literal an attribute or a macro takes first (#[folder = "../../web"], include_dir!(…)):
# a file read as it compiles, by a third-party macro, relative to the source's folder or the crate's.
MACROLIT = re.compile(r'#!?\[\s*\w+(?:::\w+)*\s*(?:=|\()\s*"(\.\./[^"\\]*)"|\b(?!include(?:_str|_bytes)?!)\w+!\s*[(\[{]\s*"(\.\./[^"\\]*)"')


def build_inputs(files, pkgs):
    """(prefix, why) for every repo path the build reads; the build scripts and proc-macro package
    folders; and each file include_str!d or include_bytes!d: [(the source, the macro's offset)]."""
    ins = [(p, "the Rust build") for p in BUILD + pkgs] + [(p, "runs the part") for p in RUNNER]
    # Build scripts: a package's build.rs, or its manifest's `build = "…"`; and every source of a
    # proc-macro package (it runs as the crates using it compile).
    scripts, macros, data = set(), [], {}
    for d in pkgs:
        toml = read(d + "/Cargo.toml")
        b = re.search(r'^build\s*=\s*"([^"]+)"', toml, re.M)
        scripts.add(norm(d, b.group(1)) if b else d + "/build.rs")
        if re.search(r"^proc-macro\s*=\s*true", toml, re.M):
            macros.append(d)
    for f in files:
        if not f.endswith(".rs") or not any(under(f, p) for p in pkgs):
            continue
        src, d = read(f), posixpath.dirname(f)
        # (include! and #[path] name Rust sources, whose own modules and includes are followed only
        # inside the package folders.)
        for m in INCLUDE.finditer(src):
            if not m.group(2):
                raise Doubt("%s includes a path that isn't a literal" % f)
            p = norm(d, m.group(3))
            if p is None:
                raise Doubt("%s includes a file outside the repo: %s" % (f, m.group(3)))
            if m.group(1) is None and not any(under(p, k) for k in pkgs):
                raise Doubt("%s include!s Rust outside the package folders: %s" % (f, p))
            if m.group(1) is None:
                ins.append((p, "%s includes it" % f))
            else:
                ins.append((p, "%s reads it as it compiles" % f))
                data.setdefault(p, []).append((f, m.start()))
        for rel in PATHATTR.findall(src):
            p = norm(d, rel)
            if p is None or not any(under(p, k) for k in pkgs):
                raise Doubt("%s has a #[path] outside the package folders: %s" % (f, rel))
            ins.append((p, "%s's #[path]" % f))
        for m in MACROLIT.finditer(src):
            rel = m.group(1) or m.group(2)
            for p in (norm(d, rel), norm("/".join(f.split("/")[:2]), rel)):
                if p is None:
                    raise Doubt("%s names a path outside the repo in a macro: %s" % (f, rel))
                ins.append((p, "%s names it in a macro" % f))
        if f in scripts or any(under(f, k) for k in macros):
            for p, _ in reads(f, src):
                ins.append((p, "%s (a build script or proc macro) reads it" % f))
    return ins, scripts, macros, data


PLAIN = "the Rust build"   # (the reason a file inside a package folder, and nothing more, gives)
DATA = " reads it as it compiles"


def traced(f, whys, scripts, macros, data, files):
    """The test units an own change of f, a Rust build input, affects beyond what affected-tests.sh
    traces for it ([] none more); None when they can't be told. Traced: a module's source (a .rs
    file under crates/<crate>/src or tests, not the crate's root, whose own tests no filter
    isolates nor its users are traced, nor a build script or a proc macro's), and a file
    include_str!d or include_bytes!d inside #[test] functions (those tests: a source test reading
    agent/build.rs). Not: one include!d, named by #[path] or in a macro, or read by a build script."""
    p = f.split("/")
    for w in set(whys):
        if w == PLAIN:
            if not (len(p) >= 4 and p[0] == "crates" and p[2] in ("src", "tests") and f.endswith(".rs")
                    and f not in scripts and not any(under(f, k) for k in macros)
                    and "/".join(p[2:]) not in ("src/lib.rs", "src/main.rs")):
                return None
        elif not w.endswith(DATA):
            return None
    units = []
    for g, at in data.get(f, []):
        try:
            units += units_of(g, at, read(g), files)
        except Doubt:
            return None
    return units


# ---- the tests' reads at run time ----

READ = re.compile(r'CARGO_MANIFEST_DIR|"(\.\./[^"\\]*)"|"((?:\{[^}"]*\})?/\.\.(?:/[^"\\]*)?)"'
                  r'|\b(?:join|new|from|push|read_dir|canonicalize)\(\s*"(\.\.)"')


def reads(f, src):
    """(repo prefix, offset) for each repo path f's code reads at run time: a CARGO_MANIFEST_DIR
    joined at once with a literal, or a "../" literal from the crate's folder (a test's working
    folder; one leaving the repo from there, "../../../x/y", has a deeper base: "*/x/y", x/y under
    any folder). The whole repo ("") for any other: a CARGO_MANIFEST_DIR otherwise used or its
    parent taken, a ".." whose base can't be told ("{}/../x", "/../x", join(".."))."""
    crate = "/".join(f.split("/")[:2])
    built = {m.start(2) for m in INCLUDE.finditer(src) if m.group(2)} | {m.start(1) - 1 for m in PATHATTR.finditer(src)}
    out = []
    for m in READ.finditer(src):
        if m.start() in built:
            continue   # (the build's inputs)
        if m.group(0) == "CARGO_MANIFEST_DIR":
            j = re.match(r'"\s*\)\s*\)\s*\.join\(\s*"([^"\\]*)"\s*\)(?!\s*\.\s*parent\b)', src[m.end():])
            p = norm(crate, j.group(1)) if j else None
            p = "" if p is None else p
        elif m.group(1) is not None:
            p = norm(crate, m.group(1))
            if p is None:
                t = posixpath.normpath(m.group(1))
                while t.startswith("../"): t = t[3:]
                p = "" if t in ("..", ".") else "*/" + t
        else:
            p = ""
        if (p, m.start()) not in out:
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
    for m in re.finditer(r"\b(fn|mod)\s+(\w+)|\bmacro_rules!\s*(\w+)|[{};]", b):
        if m.start() >= at:
            break
        t = m.group(0)
        if m.group(3):
            pending = ("macro", m.group(3), "")
        elif m.group(1):
            pending = (m.group(1), m.group(2), b[last: m.start()])
        elif t == "{":
            stack.append(pending); pending = None; last = m.end()
        elif t == "}":
            if stack: stack.pop()
            pending = None; last = m.end()
        else:
            pending = None; last = m.end()
    fns = [s for s in stack if s and s[0] == "fn"]
    if any(s and s[0] == "macro" for s in stack):
        return None   # (a macro's test is where the macro is used)
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
    # The targets are named as cargo's defaults name them: the package after its folder, no
    # targets of its own.
    toml = read("crates/%s/Cargo.toml" % c)
    if not re.search(r'^name\s*=\s*"%s"' % re.escape(c), toml, re.M) or \
            re.search(r"^\[(?:lib|\[(?:bin|test|bench|example)\])\]|^auto(?:bins|tests|examples|benches)\s*=", toml, re.M):
        raise Doubt("crates/%s's targets aren't cargo's defaults for its folder" % c)
    if kind == "tests":
        tdir = "crates/%s/tests/" % c
        if any(t.startswith(tdir) and t.count("/") == 3 and "#[path" in read(t) for t in files):
            raise Doubt("a test of crates/%s declares a #[path] module" % c)
        if len(rel) == 1:   # a test of its own
            return [] if SCANS.search(src) else [(c, "--test " + rel[0][:-3], None)]
        if os.path.exists(tdir + rel[0] + "/main.rs"):   # a test of its own, as a folder
            return [] if SCANS.search(read(tdir + rel[0] + "/main.rs")) else [(c, "--test " + rel[0], None)]
        # A helper folder's (tests/common/): every test declaring it without a scans line.
        decl = re.compile(r"^\s*mod\s+%s\s*;" % re.escape(rel[0]), re.M)
        return [(c, "--test " + posixpath.basename(t)[:-3], None) for t in files
                if t.startswith(tdir) and t.count("/") == 3 and t.endswith(".rs")
                and decl.search(t_src := read(t)) and not SCANS.search(t_src)]
    if kind != "src":
        return []   # examples, benches: cargo test builds them, runs none of them
    # (A #[path] in an inline module is relative to that module's folder: matched by its tail.)
    for g in files:
        if g.startswith("crates/%s/src/" % c) and g.endswith(".rs"):
            for x in PATHATTR.findall(read(g)):
                t = posixpath.normpath(x)
                if norm(posixpath.dirname(g), x) == f or f.endswith("/" + t) or \
                        (t.startswith("../") and posixpath.basename(t) == posixpath.basename(f)):
                    raise Doubt("%s may be a #[path] module (%s)" % (f, g))
    e = enclosing(src, at)
    if e is None:
        raise Doubt("%s:%d reads the repo outside a #[test] function" % (f, line))
    mods, fn = e
    if rel[0] == "bin":
        if len(rel) != 2:
            raise Doubt("%s:%d is in a binary's module" % (f, line))
        return [(c, "--bin " + rel[1][:-3], "::".join(mods + [fn]))]
    lib = os.path.exists("crates/%s/src/lib.rs" % c)
    if lib and os.path.exists("crates/%s/src/main.rs" % c):
        raise Doubt("crates/%s has a library and a main.rs: %s may be either's" % (c, f))
    path = "/".join(rel)[:-3].split("/")
    if path[-1] == "mod": path = path[:-1]
    if path in (["lib"], ["main"]): path = []
    tgt = "--lib" if lib else "--bin " + c
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
            elif t[i] in ("--bin", "--test") and i + 1 < len(t): tgts.append(t[i] + " " + t[i + 1]); i += 2
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
    xs = sorted(set(xs))
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
    # (Each pass's tree against HEAD's: the one with the fewest own files since, then the fewest
    # files in all; the newest of equals.)
    branch, pick, seen = names(base, head), None, set()
    for p, kv in cands[:CANDIDATES]:
        if p in seen:
            continue
        seen.add(p)
        try:
            if git("rev-parse", p + "^{tree}").strip() != kv["tree"]:
                continue
            d, x = names(kv["at"], base), names(p, head)
        except Doubt:
            continue
        mains = {f for f in x if f in d and f not in branch}
        if pick is None or (len(x - mains), len(x)) < (len(pick[3]), len(pick[2])):
            pick = (p, kv, x, x - mains, mains)
    if not pick:
        raise Doubt("no earlier pass (of %d) whose commit and tree can be found" % len(seen))
    p, kv, x, own, mains = pick
    sp = p[:7]
    how = "this tree is %s's plus main's changes since its base %s (%s)" % (
        sp, kv["at"][:7], "%d files: %s" % (len(mains), short(mains)) if mains else "none")
    if own:
        how += " and the branch's own since (%d files: %s)" % (len(own), short(own))
    unknown = [f for f in x if f.split("/")[0] not in KNOWN]
    if unknown:
        raise Doubt("files of no known kind changed since %s: %s" % (sp, short(unknown)))
    files, links = [], []
    for e in filter(None, git("ls-files", "-s", "-z").split("\0")):
        mode, f = e.split(" ", 1)[0], e.split("\t", 1)[1]
        files.append(f)
        if mode == "120000": links.append(f)
    if links:   # (a read through one names the link, not the file changed)
        raise Doubt("the repo holds symlinks, which a read can go through: " + short(links))
    ins, scripts, macros, data = build_inputs(files, packages(files))
    hit = {}
    for f in x:
        for pre, why in ins:
            if under(f, pre):
                hit.setdefault(f, []).append(why)
    say = lambda fs: short("%s (%s)" % (f, ([w for w in hit[f] if w != PLAIN] + [PLAIN])[0]) for f in fs)
    out = []
    if hit:
        out.append(("clippy", "run", "-", "Rust build inputs changed since %s: %s" % (sp, say(hit))))
    else:
        out.append(("clippy", "reuse", p, how + "; none is a Rust build input"))
    if not tests:
        return out
    hit_main = [f for f in hit if f in mains]
    if hit_main:
        out.append(("tests", "run", "-", "main changed Rust build inputs since %s: %s" % (sp, say(hit_main))))
        return out
    extra, odd = [], []
    for f in hit:
        u = traced(f, hit[f], scripts, macros, data, files)
        if u is None: odd.append(f)
        else: extra += [(c, g, flt, f) for c, g, flt in u]
    if odd:
        out.append(("tests", "run", "-", "the branch changed Rust build inputs since %s that the "
                    "test set doesn't trace: %s" % (sp, say(odd))))
        return out
    if kv.get("tests") == th:
        theirs = tests
    else:
        kept = os.path.join(os.path.dirname(os.path.abspath(passed)), "check-tests", kv.get("tests", "-"))
        if not os.path.isfile(kept):
            out.append(("tests", "run", "-", "the affected tests here aren't %s's, and its aren't kept" % sp))
            return out
        theirs = [l for l in read(kept).splitlines() if l.strip()]
    how += "; none is a Rust build input" if not hit else "; the Rust ones the branch's own modules"
    try:
        out += tests_part(tests, theirs, x, files, p, how, extra)
    except Doubt as e:
        out.append(("tests", "run", "-", str(e)))
    return out


def tests_part(tests, theirs, x, files, p, how, extra):
    ours = parse_cmds(tests)
    # HEAD's tests that P's commands didn't run (cargo's filters match by substring: a filter of
    # P's inside one of HEAD's ran its tests).
    old, new = parse_cmds(theirs), {}
    for k, fs in ours.items():
        if k not in old:
            new[k] = fs
        elif old[k] is None:
            continue
        elif fs is None:
            new[k] = None
        else:
            left = [f for f in fs if not any(o in f for o in old[k])]
            if left: new[k] = left
    # The tests X affects: affected-tests.sh over P..HEAD (the own sources' modules traced as for
    # the branch, the scan tests' lines), then the sources' reads at run time.
    r = subprocess.run([HERE + "/affected-tests.sh", "%s..HEAD" % p], capture_output=True, text=True)
    if r.returncode:
        raise Doubt("affected-tests.sh over the changes since %s failed: %s" % (p[:7], r.stderr.strip()[:200]))
    hits = parse_cmds([l for l in r.stdout.splitlines() if l and not l.startswith("#")])
    why = ["%s%s (affected)" % (k[1], " -- " + short(v, 2) if v else "") for k, v in hits.items()]
    for c, g, flt, f in extra:   # (the tests include_str!ing a changed file)
        if (c, g) in hits and hits[(c, g)] is None: continue
        hits[(c, g)] = None if flt is None else hits.get((c, g), []) + [flt]
        why.append("%s (reads %s as it compiles)" % (flt or g, f))
    for f in files:
        if not (f.startswith("crates/") and f.endswith(".rs")):
            continue
        src = read(f)
        for pre, at in reads(f, src):
            touched = [y for y in x if under(y, pre)]
            if not touched: continue
            for c, g, flt in units_of(f, at, src, files):
                if (c, g) in hits and hits[(c, g)] is None: continue
                hits[(c, g)] = None if flt is None else hits.get((c, g), []) + [flt]
                why.append("%s (%s reads %s)" % (flt or g, f, short(touched, 2)))
    run = intersect(hits, ours)
    for k, fs in new.items():
        run[k] = None if fs is None or run.get(k, []) is None else sorted(set(run.get(k, []) + fs))
    if new:
        why.append("not run at %s: %s" % (p[:7], short("%s%s" % (k[1], " -- " + " ".join(v) if v else "")
                                                      for k, v in new.items())))
    if not run:
        return [("tests", "reuse", p, how + ("; the tests they affect (%s) aren't run here" % short(why)
                                              if why else "; no test reads them"))]
    return [("tests", "partial", p, how + "; run again: " + short(why, 6))] + \
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
