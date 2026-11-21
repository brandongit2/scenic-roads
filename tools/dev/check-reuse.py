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
The parts are clippy and tests (the others are never reused); check.sh's prod part, whose inputs are
clippy's, takes clippy's verdict.

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
  - tests: each one that is such an input, main's or the branch's own, is a source of a crate (a
    .rs file under crates/<crate>/src, not a build script or proc macro's), an integration test's
    (under crates/<crate>/tests), an example or a file include_str!d or include_bytes!d, nothing
    include!ing it or naming it in a macro (one a crate's source names by #[path] is that crate's
    source too). Then run again:
      - the tests HEAD's commands run that P's didn't;
      - every test of HEAD's commands in a program linking the changed source's crate (#969): the
        crate's own targets, and those of the crates reaching it through their dependencies (a
        program's dev-dependencies count, a dependency's don't). Naming can't place one nearer: a
        user's user, a trait's impl, a macro or a re-export names nothing. So too for a file
        compiled in outside a #[test] function;
      - of the rest, what can change without a program changing: the tests affected-tests.sh gives
        for P..HEAD (an integration test changed, the scan tests whose `//! scans:` line covers a
        file in X), a #[test] function's own include_str! of a changed file, and the tests reading
        a file in X at run time (a source's CARGO_MANIFEST_DIR or ".." path inside a #[test]
        function). A reading this can't place (outside a #[test] function, in a macro, a crate
        whose targets aren't cargo's defaults) runs the whole part; a path whose base can't be told
        is the whole repo.
Doubt runs both parts: a symlink in the repo, an include! or #[path] outside the package folders
(whose own modules aren't followed), a path a manifest or .cargo names outside them, a source
reading the environment as it compiles (env!, option_env! other than cargo's own CARGO_*; and
OUT_DIR, `include!(concat!(env!("OUT_DIR"), …))`, a build script's output, only in a package whose
build script, with the files its #[path] modules are, reads nothing outside the package's folder as
`reads` sees it: then what it writes is made from the build's inputs). A build script's own code
(the script, its #[path] modules, what they include_*!: cargo runs a script it compiled anew) and
what it says it reads by its `rerun-if-changed` lines (`script_reads`: crates/inputs' digests read
every crate its manifests reach by path, the root Cargo.toml and the registry crates it patches to a
path by [patch] or [replace]) are sources of its package: a change there runs the tests of the
programs linking it, and affected-tests.sh lists the package's tests whole. A package the root
manifest patches in for a registry crate (crates/geos-src) links into programs through registry
crates `linking` doesn't follow: a change in it runs the whole part.
Anything not shown safe (no such pass, a git lookup failing) runs the part.
"""
import hashlib, os, posixpath, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
# The Rust build's inputs besides its package folders: what cargo, rustc and clippy read.
BUILD = ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "mise.toml",
         ".mise.toml", "clippy.toml", ".clippy.toml", "rustfmt.toml", ".rustfmt.toml", ".cargo", ".config"]
# The scripts that run clippy, prod and tests (check.sh's lines, the test set, the shim, the test Mac's run, the
# dev jobs' recorder wrapping them).
# tools/dev/test/check-reuse.zsh checks they run no other repo script.
RUNNER = ["tools/dev/check.sh", "tools/dev/check-reuse.py", "tools/dev/affected-tests.sh", "tools/dev/rustitems.py",
          "tools/dev/bin", "tools/app/remote-tests.sh", "tools/dev/devjob.zsh", "tools/dev/devjob.py"]
# Every top-level entry of the repo, so a file under a new one is unknown (doubt: both parts run).
# tools/dev/test/check-reuse.zsh checks every tracked file's entry is here.
KNOWN = set(BUILD) | {"docs", "web", "producers", "dem", "tools", "scripts", ".claude", "README.md",
                      "CLAUDE.md", ".gitignore", ".git-blame-ignore-revs"}


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

def listed(key, files):
    """The folders the root manifest's [workspace] `key` list (members, exclude) names, its globs read."""
    m = re.search(r"^%s\s*=\s*\[([^\]]*)\]" % key, read("Cargo.toml"), re.M)
    dirs = []
    for g in re.findall(r'"([^"]+)"', m.group(1)) if m else []:
        # (As cargo reads them: "./crates/x" and "crates/x/" are crates/x.)
        if norm("", g) is None:
            raise Doubt("Cargo.toml's %s names a folder outside the repo: %s" % (key, g))
        g = norm("", g)
        if "*" not in g:
            dirs.append(g); continue
        if not g.endswith("/*") or "*" in g[:-2]:
            raise Doubt("Cargo.toml's %s has a glob this doesn't read: %s" % (key, g))
        pre = g[:-1]
        dirs += sorted({pre + f[len(pre):].split("/")[0] for f in files if f.startswith(pre) and "/" in f[len(pre):]})
    return dirs


def packages(files):
    """The workspace's package folders: its members, excluded crates and path dependencies."""
    root = read("Cargo.toml")
    dirs = listed("members", files) + listed("exclude", files)
    # The root manifest's own paths: [workspace.dependencies], [patch], [replace].
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
# (What an include names after its parenthesis when it's of a build script's output.)
OUT_DIR = re.compile(r'concat!\s*\(\s*env!\s*\(\s*"OUT_DIR"\s*\)')
PATHATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
# A "../" literal an attribute or a macro takes first (#[folder = "../../web"], include_dir!(…)):
# a file read as it compiles, by a third-party macro, relative to the source's folder or the crate's.
# The environment read as a source compiles: nothing in a tree says what it held. (cargo's own CARGO_*
# follow from the tree.)
ENVMACRO = re.compile(r'\b(?:option_)?env!\s*\(\s*("[^"\\]*")?')
# (#[path] is PATHATTR's: relative to its source's folder, a Rust source whose modules are followed.)
MACROLIT = re.compile(r'#!?\[\s*(?!path\s*=)\w+(?:::\w+)*\s*(?:=|\()\s*"(\.\./[^"\\]*)"|\b(?!include(?:_str|_bytes)?!)\w+!\s*[(\[{]\s*"(\.\./[^"\\]*)"')


def build_scripts(pkgs):
    """{a package folder: its build script, the manifest's `build = "…"` or build.rs (there or not)}."""
    out = {}
    for d in pkgs:
        b = re.search(r'^build\s*=\s*"([^"]+)"', read(d + "/Cargo.toml"), re.M)
        out[d] = norm(d, b.group(1)) if b else d + "/build.rs"
    return out


# What a build script tells cargo it read: a `rerun-if-changed=` path, a literal ("build/x.rs") or
# made as it runs ("{}", "{f}", or the literal ending there: the path concatenated).
RERUN = re.compile(r'rerun-if-changed=([^"\\]*)')
RERUN_ANY = re.compile(r"rerun-if-(?:env-)?changed=")


def script_reads(pkgs):
    """{a package folder: the repo prefixes its build script reads} for each package with one: the
    script's own code (the script, the files its #[path] modules are, what they include_str!,
    include_bytes! or include!: cargo compiles the script again when one changes, and runs a script
    it compiled anew), and what its `rerun-if-changed` lines say (what cargo reruns it on, so what
    its output is made from): a literal path, that path; a path made as it runs (crates/inputs'
    digest of its sources and every crate its manifest reaches, build/checks_code.rs), what a script
    can find from its package: the package's folder, every package its manifests reach by path
    (dependencies and build-dependencies, a registry crate the root manifest patches to a path by
    [patch] or [replace]) and the root manifest; no rerun line at all, cargo's default: the
    package's folder. A script naming only `rerun-if-env-changed` reads only its own code
    (agentcore's, which sets the cfg its tests compile under; geos-src's, which compiles GEOS's
    source/Version.txt in)."""
    graph = manifests(pkgs)[0]
    out = {}
    for d, s in sorted(build_scripts(pkgs).items()):
        own = script_files(s)
        code = "".join(blank(read(g), strings=False) for g in own)
        if not code:
            continue
        pre = set(own) | {p for g in own for m in INCLUDE.finditer(read(g)) if m.group(2)
                          for p in [norm(posixpath.dirname(g), m.group(3))] if p is not None}
        if not RERUN_ANY.search(code):
            pre.add(d)
        for rel in RERUN.findall(code):
            if rel and "{" not in rel:
                p = norm(d, rel)
                if p is None:
                    raise Doubt("%s's build script reruns on a path outside the repo: %s" % (d, rel))
                pre.add(p)
                continue
            seen, todo = set(), [d]
            while todo:
                q = todo.pop()
                if q not in seen:
                    seen.add(q)
                    todo += graph.get(q, ())
            pre |= seen | {"Cargo.toml"}
        out[d] = sorted(pre)
    return out


SCRIPT = "'s build script reads it"   # (after the package folder: "crates/inputs's build script …")


def build_inputs(files, pkgs):
    """(prefix, why) for every repo path the build reads; the build scripts' code (script_files) and
    the proc-macro package folders; and each file include_str!d or include_bytes!d: [(the source, the macro's offset)]."""
    ins = [(p, "the Rust build") for p in BUILD + pkgs] + [(p, "runs the part") for p in RUNNER]
    # Build scripts: a package's build.rs, or its manifest's `build = "…"`, and what each says it
    # reads; and every source of a proc-macro package (it runs as the crates using it compile).
    scripts, macros, data = set(build_scripts(pkgs).values()), [], {}
    for d, pres in script_reads(pkgs).items():
        ins += [(p, d + SCRIPT) for p in pres]
    for d in pkgs:
        if re.search(r"^proc-macro\s*=\s*true", read(d + "/Cargo.toml"), re.M):
            macros.append(d)
    runs = {g for s in scripts for g in script_files(s)}
    for f in files:
        if not f.endswith(".rs") or not any(under(f, p) for p in pkgs):
            continue
        src, d = read(f), posixpath.dirname(f)
        out_dir = own_build(f, pkgs, scripts)
        for m in ENVMACRO.finditer(src):
            ok = (m.group(1) or "").startswith('"CARGO_') or out_dir and m.group(1) == '"OUT_DIR"'
            if not ok and blank(src)[m.start()] != " ":
                raise Doubt("%s reads the environment as it compiles: %s" % (f, m.group(0)))
        # (include! and #[path] name Rust sources, whose own modules and includes are followed only
        # inside the package folders.)
        for m in INCLUDE.finditer(src):
            if not m.group(2) and out_dir and OUT_DIR.match(src, m.end()):
                continue
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
            ins.append((p, f + PATHED))
        for m in MACROLIT.finditer(src):
            rel = m.group(1) or m.group(2)
            for p in (norm(d, rel), norm("/".join(f.split("/")[:2]), rel)):
                if p is None:
                    raise Doubt("%s names a path outside the repo in a macro: %s" % (f, rel))
                ins.append((p, "%s names it in a macro" % f))
        if f in runs or any(under(f, k) for k in macros):
            for p, _ in reads(f, src):
                ins.append((p, "%s (a build script or proc macro) reads it" % f))
    return ins, runs, macros, data


# A module declared by its name alone, `mod name;` (one after a #[path] is PATHATTR's).
MODDECL = re.compile(r'(#\[path\s*=\s*"[^"]*"\]\s*)?(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?\bmod\s+(\w+)\s*;')


def script_files(s):
    """Build script s and the files its modules are, and theirs: the code that runs as it (none if
    it isn't there). A #[path] module's file is relative to its source's folder; a `mod name;`'s is
    name.rs or name/mod.rs in the folder of the source's own modules (the script's and a #[path]
    file's own folder, a mod.rs's too; a name.rs's name/ beside it): neither there is a Doubt."""
    out, todo = [], [(s, True)]
    while todo:
        f, own_dir = todo.pop()
        if f in out or not os.path.exists(f):
            continue
        out.append(f)
        src, d = read(f), posixpath.dirname(f)
        todo += [(p, True) for p in (norm(d, rel) for rel in PATHATTR.findall(src)) if p]
        kids = d if own_dir or f.endswith("/mod.rs") else f[:-len(".rs")]
        for m in MODDECL.finditer(blank(src, strings=False)):
            if m.group(1):
                continue
            c = [p for p in (kids + "/" + m.group(2) + ".rs", kids + "/" + m.group(2) + "/mod.rs") if os.path.exists(p)]
            if not c:
                raise Doubt("%s declares mod %s, whose file isn't there" % (f, m.group(2)))
            todo.append((c[0], False))
    return out


def own_build(f, pkgs, scripts):
    """Whether f's package has a build script reading nothing outside the package's folder (as
    `reads` sees it, in the script and its #[path] modules), so what it writes to OUT_DIR is made
    from the build's inputs."""
    pkg = max((p for p in pkgs if under(f, p)), key=len)
    runs = [g for s in scripts if under(s, pkg) for g in script_files(s)]
    return bool(runs) and all(p and under(p, pkg) for g in runs for p, _ in reads(g, read(g)))


PLAIN = "the Rust build"   # (the reason a file inside a package folder, and nothing more, gives)
DATA = " reads it as it compiles"
PATHED = "'s #[path]"   # (after the declaring source: "<f>'s #[path]")


def source(f, kind, scripts, macros):
    """A .rs file under crates/<crate>/<kind>, not a build script's or a proc macro's."""
    p = f.split("/")
    return (len(p) >= 4 and p[0] == "crates" and p[2] == kind and f.endswith(".rs")
            and f not in scripts and not any(under(f, k) for k in macros))


def manifests(pkgs):
    """From the manifests: ({a package folder: the folders its dependencies and build-dependencies
    name by path, and those the root manifest patches a registry one to}, {folder: package name},
    {folder: those and its dev-dependencies' paths, no patches}, the patched folders)."""
    try:
        import tomllib   # (Python 3.11's: affected-tests.sh imports this file, on any python3.)
    except ImportError:
        raise Doubt("this python3 has no tomllib to read the manifests")

    def toml(p):
        try:
            return tomllib.loads(read(p))
        except tomllib.TOMLDecodeError as e:
            raise Doubt("%s can't be read: %s" % (p, e))

    root = toml("Cargo.toml")
    table = root.get("workspace", {}).get("dependencies", {})
    patch = {n: norm("", v["path"]) for t in root.get("patch", {}).values() for n, v in t.items() if "path" in v}
    # ([replace]'s keys are package ID specs, "<name>:<version>" or "<source URL>#<name>:<version>": the
    # name is after the last '#', before ':' or '@', as crates/inputs' build/checks_code.rs reads it.)
    patch.update({re.split(r"[:@]", k.rsplit("#", 1)[-1])[0]: norm("", v["path"]) for k, v in root.get("replace", {}).items() if "path" in v})

    def deps(d, m, kinds, patched):
        out = set()
        for t in [m] + list(m.get("target", {}).values()):
            for sec, v in ((k, v) for k in t if k.replace("_", "-") in kinds for v in t[k].items()):
                name, v = v
                w = table.get(name) if isinstance(v, dict) and v.get("workspace") else v
                base = "" if w is not v else d
                if isinstance(w, dict) and "path" in w:
                    out.add(norm(base, w["path"]))
                elif patched:
                    pkg = (w.get("package") if isinstance(w, dict) else None) or name
                    if pkg in patch: out.add(patch[pkg])
        return out

    graph, names, every = {}, {}, {}
    for d in pkgs:
        m = toml(d + "/Cargo.toml")
        if "name" in m.get("package", {}):
            names[d] = m["package"]["name"]
            graph[d] = deps(d, m, ("dependencies", "build-dependencies"), True)
            every[d] = deps(d, m, ("dependencies", "build-dependencies", "dev-dependencies"), False)
    return graph, names, every, set(patch.values())


def linking(pkgs):
    """{a package folder: the package names of every folder whose test programs link it}: a program
    holds its crate, its dependencies of any kind and theirs but for their dev-dependencies. Without
    a folder [patch] names: its users' manifests name a version, not its path."""
    graph, names, every, patched = manifests(pkgs)
    runs = {d: {p for p in g if p not in patched} for d, g in graph.items()}
    out = {d: set() for d in names if d not in patched}
    for t in names:
        seen, todo = {t}, list(every[t])
        while todo:
            d = todo.pop()
            if d not in seen:
                seen.add(d)
                todo += runs.get(d, ())
        for d in seen & set(out):
            out[d].add(names[t])
    return out


def outside_users(pkgs, files):
    """{a package folder outside the workspace's members (an excluded crate, a path dependency of
    none, a registry crate patched to a path): the members whose own dependencies reach it without
    going through another member}, as Cargo.lock records the dependencies (a registry crate's are
    named only there: terrain reaches crates/geos-src through geos and geos-sys, which name it by
    version). cargo test -p takes members only; these are its nearest tests."""
    try:
        import tomllib
    except ImportError:
        raise Doubt("this python3 has no tomllib to read Cargo.lock")
    try:
        lock = tomllib.loads(open("Cargo.lock", encoding="utf-8").read()).get("package", [])
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise Doubt("Cargo.lock can't be read: %s" % e)

    def find(dep):   # a dependency's entry: "<name>", "<name> <version>" or "<name> <version> (<source>)"
        t = dep.split(" ", 2)
        c = [i for i, p in enumerate(lock) if p.get("name") == t[0] and (len(t) < 2 or p.get("version") == t[1])
             and (len(t) < 3 or "(%s)" % p.get("source") == t[2])]
        if len(c) != 1:
            raise Doubt("Cargo.lock's dependency %r names %d packages" % (dep, len(c)))
        return c[0]

    def path_pkg(name):   # (a package from the repo has no source)
        c = [i for i, p in enumerate(lock) if p.get("name") == name and "source" not in p]
        return c[0] if len(c) == 1 else None

    names = manifests(pkgs)[1]
    members = {d: path_pkg(names[d]) for d in listed("members", files) if d in names}
    ours = set(members.values())
    out = {d: [] for d in pkgs if d in names and d not in members}
    at = {path_pkg(names[d]): d for d in out}
    for m, i in sorted(members.items()):
        if i is None:
            raise Doubt("Cargo.lock has no package %s from the repo" % names[m])
        seen, todo = set(), [find(x) for x in lock[i].get("dependencies", [])]
        while todo:
            j = todo.pop()
            if j in seen or j in ours:
                continue
            seen.add(j)
            todo += [find(x) for x in lock[j].get("dependencies", [])]
        for j in seen & set(at):
            out[at[j]].append(m)
    return out


def traced(f, whys, scripts, macros, data, files, links):
    """What a change of f since the pass (main's or the branch's own), a Rust build input, affects
    beyond what affected-tests.sh gives for it: (test units, packages all of whose test programs
    link it); None when they can't be told. A crate's source: the packages linking its crate. An
    integration test's: itself, by affected-tests.sh. An example: none (no test builds or runs one;
    clippy checks it). A file include_str!d or include_bytes!d inside #[test] functions: those
    tests (a source test reading agent/build.rs); outside them, in a crate's source or an
    integration test: as that source (web/buildpage's pages in coord/mod.rs). A file a crate's source
    declares a module by #[path]: a source of that crate too (agentcore's unit tests compile the
    plan, pipeline's agent/build.rs …, in). A build script's code and a file it says it reads
    (`script_reads`): as a source of the script's package. Not: one include!d or named in a macro, or read by a
    build script otherwise (its `reads`)."""
    units, linked = [], set()

    def link(g, d=None):
        d = d or "/".join(g.split("/")[:2])
        if d not in links:
            return False
        linked.update(links[d])
        return True

    for w in set(whys):
        if w.endswith(SCRIPT):
            # (What the script makes of it is compiled into its package: as a source of it.)
            if not link(None, w[:-len(SCRIPT)]):
                return None
        elif w == PLAIN:
            if f in scripts or any(g in scripts for g, _ in data.get(f, [])):
                continue   # (a build script's code, or a file it compiles in: its package's, by its SCRIPT reason)
            if source(f, "src", scripts, macros):
                if not link(f):
                    return None
            elif not (source(f, "tests", scripts, macros) or source(f, "examples", scripts, macros)):
                return None
        elif w.endswith(PATHED):
            g = w[:-len(PATHED)]
            if g in scripts:
                continue   # (a build script's module: its package's, by its SCRIPT reason)
            if not source(g, "src", scripts, macros) or not link(g):
                return None
        elif not w.endswith(DATA):
            return None
    for g, at in data.get(f, []):
        if g in scripts:
            continue   # (in a build script's code: its package's, by its SCRIPT reason)
        src = read(g)
        if enclosing(src, at) is None:
            if not (source(g, "src", scripts, macros) or source(g, "tests", scripts, macros)) or not link(g):
                return None
            continue
        try:
            units += units_of(g, at, src, files)
        except Doubt:
            return None
    return units, linked


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


def blank(src, strings=True):
    """src with comments, strings and char literals as spaces (offsets and newlines kept); with
    `strings` false, the comments alone."""
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
            if strings: wipe(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            if strings: wipe(i, j + 1)
            i = j + 1
        elif c == "'" and (m := char.match(src, i)):
            if strings: wipe(i, m.end())
            i = m.end()
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
    pkgs = packages(files)
    ins, scripts, macros, data = build_inputs(files, pkgs)
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
    links = linking(pkgs)
    extra, linked, odd = [], {}, []
    for f in hit:
        u = traced(f, hit[f], scripts, macros, data, files, links)
        if u is None:
            odd.append(f)
        else:
            extra += [(c, g, flt, f) for c, g, flt in u[0]]
            for c in u[1]: linked.setdefault(c, f)
    if odd:
        whose = " and ".join(w for w, fs in (("main", [f for f in odd if f in mains]),
                                             ("the branch", [f for f in odd if f not in mains])) if fs)
        out.append(("tests", "run", "-", "%s changed Rust build inputs since %s that the test set "
                    "doesn't trace: %s" % (whose, sp, say(odd))))
        return out
    if kv.get("tests") == th:
        theirs = tests
    else:
        kept = os.path.join(os.path.dirname(os.path.abspath(passed)), "check-tests", kv.get("tests", "-"))
        try:
            with open(kept, "rb") as fh:
                b = fh.read()
        except OSError:
            b = None
        # (Named by its hash: a file cut short or replaced, by a writer of another worktree's or
        # anything else, isn't the pass's commands. A filter cut short would match tests by
        # substring that it never ran.)
        if b is None or hashlib.sha1(b).hexdigest() != kv.get("tests"):
            out.append(("tests", "run", "-", "the affected tests here aren't %s's, and its aren't kept%s" % (
                sp, "" if b is None else " whole (check-tests/%s isn't their hash)" % kv["tests"][:7])))
            return out
        theirs = [l for l in b.decode("utf-8", "replace").splitlines() if l.strip()]
    how += "; none is a Rust build input" if not hit else "; the Rust ones traced"
    try:
        out += tests_part(tests, theirs, x, files, p, how, extra, linked)
    except Doubt as e:
        out.append(("tests", "run", "-", str(e)))
    return out


def tests_part(tests, theirs, x, files, p, how, extra, linked):
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
    # The tests X affects: affected-tests.sh over P..HEAD (the integration tests changed, the scan
    # tests' lines), every test in a program linking a changed crate, then the sources' reads at
    # run time.
    r = subprocess.run([HERE + "/affected-tests.sh", "%s..HEAD" % p], capture_output=True, text=True)
    if r.returncode:
        raise Doubt("affected-tests.sh over the changes since %s failed: %s" % (p[:7], r.stderr.strip()[:200]))
    hits = parse_cmds([l for l in r.stdout.splitlines() if l and not l.startswith("#")])
    why = ["%s%s (affected)" % (k[1], " -- " + short(v, 2) if v else "") for k, v in hits.items()]
    for c, g, flt, f in extra:   # (the tests include_str!ing a changed file)
        if (c, g) in hits and hits[(c, g)] is None: continue
        hits[(c, g)] = None if flt is None else hits.get((c, g), []) + [flt]
        why.append("%s (%s changed)" % (flt or g, f))
    for k in ours:
        if k[0] in linked:
            hits[k] = None
            why.append("%s %s (links %s)" % (k[0], k[1], linked[k[0]]))
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
