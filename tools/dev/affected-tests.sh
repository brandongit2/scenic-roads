#!/bin/zsh
# Prints the cargo test commands for the tests a change can affect (tools/dev/testing.md).
#   tools/dev/affected-tests.sh            this checkout's branch against main: merge-base..working tree,
#                                          untracked files included (run it in your worktree)
#   tools/dev/affected-tests.sh <commit>   that commit alone (<commit>^..<commit>)
#   tools/dev/affected-tests.sh <a>..<b>   a range
# The set: each changed .rs file's module (crates/pipeline/src/agent/build.rs → agent::build; a
# module with submodules gives only its own test modules, e.g. agent::tests::); one level of users
# in the same crate (`crate::<mod>`, `use crate::<parent>::{…<leaf>…}`, the crate's binaries naming
# `<crate>::<mod>`, the parent naming `<leaf>::`, a child using `super::`); the integration tests
# under crates/<crate>/tests/ that name it, or that changed; other crates' modules naming
# `<crate>::<mod>`, or `crate::<mod>` where their root re-exports it (`pub use <crate>::…`, #916).
# A file a module compiles in (include_str!, include_bytes! or include! outside a #[test] function)
# runs that module's own tests. Modules without tests are left out.
# Lines starting with # explain; the others are the commands. No command for doc-only or non-Rust
# changes. A `#[path]` module's file maps to its declared module (dem/generic_tests.rs → dem::generic::tests).
# tools/dev/test-guard.sh holds agents' cargo test to this set.
#   tools/dev/affected-tests.sh --nextest <file>  reads such `cargo test` lines from the file and
#                                          prints the `cargo nextest run` lines running the same tests (#969, below)
set -uo pipefail
usage="usage: tools/dev/affected-tests.sh [<commit> | <a>..<b> | --nextest <file>]"
case $#:${1:-} in
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  0:|1:[^-]*|2:--nextest) ;;
  *) print -ru2 -- "affected-tests.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
SCENIC_AFFECTED_HERE=${0:A:h} exec python3 - "$@" <<'PY'
import glob, os, re, subprocess, sys
from collections import OrderedDict
args = sys.argv[1:]
def git(*a):
    r = subprocess.run(["git", *a], capture_output=True, text=True)
    if r.returncode: sys.exit("git %s: %s" % (" ".join(a), r.stderr.strip()))
    return r.stdout
root = git("rev-parse", "--show-toplevel").strip()
os.chdir(root)

def crate_targets(c):
    """The crate's test targets as cargo's defaults lay them out ("--lib", "--bin <b>", "--test <t>")."""
    d, out = "crates/%s" % c, set()
    if os.path.exists(d + "/src/lib.rs"): out.add("--lib")
    if os.path.exists(d + "/src/main.rs"): out.add("--bin " + c)
    for p in glob.glob(d + "/src/bin/*.rs"): out.add("--bin " + os.path.basename(p)[:-3])
    for p in glob.glob(d + "/src/bin/*/main.rs"): out.add("--bin " + os.path.basename(os.path.dirname(p)))
    for p in glob.glob(d + "/tests/*.rs"): out.add("--test " + os.path.basename(p)[:-3])
    return out

if args[:1] == ["--nextest"]:
    # #969: the `cargo test` lines (affected-tests.sh's, or check-reuse.py's subset) as one
    # `cargo nextest run`: one cargo build of every package together, where each line built its own
    # with its own feature set (resolver 2 unifies features over the packages of one run: a line of
    # `-p server` compiled store, codecs, buildstore and pipeline again, ~170 s on the M4), and one
    # pool of test processes across every binary (cargo test ran each binary's tests after the last).
    # A filterset gives each line's binary its own filters (cargo test's filters match by substring,
    # nextest's `test(~…)` too). Cargo builds every selected target kind in every selected package:
    # a package that would get a target it didn't ask for (its lib's unit tests, an integration test
    # of the same name) goes to a run of its own, as before.
    units = OrderedDict()   # (crate, target) -> filters, None = the whole target
    for line in open(args[1], encoding="utf-8").read().splitlines():
        t = line.split()
        if not t or t[0].startswith("#"): continue
        if t[:3] != ["cargo", "test", "-p"] or len(t) < 5: sys.exit("affected-tests.sh --nextest: can't read %r" % line)
        c, i, tgts = t[3], 4, []
        while i < len(t) and t[i] != "--":
            if t[i] == "--lib": tgts.append("--lib"); i += 1
            elif t[i] in ("--bin", "--test") and i + 1 < len(t): tgts.append(t[i] + " " + t[i + 1]); i += 2
            else: sys.exit("affected-tests.sh --nextest: can't read %r" % line)
        flt = t[i + 1:] if i < len(t) else None
        if flt is not None and (not flt or any(x.startswith("-") or re.search(r"""[()|&'"\s]""", x) for x in flt)):
            sys.exit("affected-tests.sh --nextest: can't read %r" % line)
        for g in tgts:
            old = units.get((c, g), [])
            units[(c, g)] = None if flt is None or old is None else old + flt
    def bid(c, g):   # nextest's binary id
        return c if g == "--lib" else "%s::bin/%s" % (c, g[6:]) if g.startswith("--bin ") else "%s::%s" % (c, g[7:])
    pkgs = list(OrderedDict.fromkeys(c for c, _ in units))
    lib = lambda c: "--lib" in crate_targets(c)
    wants = lambda c: (c, "--lib") in units
    # (Only a lib's unit tests are worth a run of their own: an integration test or binary of the
    # same name built too costs a link; a second build, the packages' features again.)
    apart = [c for c in pkgs if lib(c) and not wants(c)] if any(map(wants, pkgs)) else []
    groups = [[c for c in pkgs if c not in apart and (lib(c) or not apart)], [c for c in pkgs if c in apart or (apart and not lib(c))]]
    for group in filter(None, groups):
        tg = sorted({g for (c, g) in units if c in group}, key=lambda g: (g != "--lib", g))
        expr = " | ".join(
            "binary_id(=%s)" % bid(c, g) if fs is None else
            "(binary_id(=%s) & (%s))" % (bid(c, g), " | ".join("test(~%s)" % f for f in sorted(set(fs))))
            for (c, g), fs in units.items() if c in group)
        print("cargo nextest run --no-fail-fast --hide-progress-bar --no-tests=warn %s %s -E '%s'" % (
            " ".join("-p " + c for c in group), " ".join(tg), expr))
    sys.exit()
# (`--no-renames` throughout: a moved file's old path too, which a scan test's prefix may cover.)
if not args:
    base = git("merge-base", "main", "HEAD").strip()
    changed = git("diff", "--no-renames", "--name-only", base).split() + git("ls-files", "--others", "--exclude-standard").split()
    what = "this branch against main (merge-base %s), working tree included" % base[:7]
elif ".." in args[0]:
    changed = git("diff", "--no-renames", "--name-only", args[0]).split(); what = args[0]
else:
    changed = git("diff", "--no-renames", "--name-only", args[0] + "^", args[0]).split(); what = "commit " + args[0][:9]
# Scan tests (#662): an integration test that reads the repo's files (write_sets, inputs_accessor,
# cli …) says which with a `//! scans: <path prefix> …` line; any change under one (any kind of
# file, deleted ones too) runs it whole. tools/app/guards.sh refuses a test file reading the repo
# without that line.
scan_hits = []
for t in sorted(glob.glob("crates/*/tests/*.rs")):
    m = re.search(r"^//! scans: (.+)$", open(t, encoding="utf-8").read(), re.M)
    if not m: continue
    pre = m.group(1).split()
    hit = next((f for f in dict.fromkeys(changed) if any(f == q or f.startswith(q.rstrip("/") + "/") for q in pre)), None)
    if hit: scan_hits.append((t.split("/")[1], "--test " + os.path.basename(t).removesuffix(".rs"), "scans " + hit))
# A file compiled into a module's code (include_str!, include_bytes! or include! outside a #[test]
# function: web/buildpage's pages in coord/mod.rs) runs that module's own tests (#969): its users see it
# only through what the module makes of it. (One a #[test] function includes is that test's read:
# check-reuse.py runs it again after a pass.)
import importlib.util
spec = importlib.util.spec_from_file_location("check_reuse", os.environ["SCENIC_AFFECTED_HERE"] + "/check-reuse.py")
reuse = importlib.util.module_from_spec(spec); spec.loader.exec_module(reuse)
cs, included = set(changed), []
r = subprocess.run(["rg", "-l", "--type", "rust", r"include(_str|_bytes)?!", "crates"], capture_output=True, text=True)
for f in sorted(r.stdout.split()):
    src = open(f, encoding="utf-8", errors="replace").read()
    for m in re.finditer(r'\binclude(?:_str|_bytes)?!\s*\(\s*"([^"\\]*)"', src):
        p = os.path.normpath(os.path.join(os.path.dirname(f), m.group(1)))
        if p in cs and reuse.enclosing(src, m.start()) is None:
            included.append((f, p)); break
# Files as they are in this checkout (a deleted file's module is gone; its users' own changes show).
changed = [f for f in dict.fromkeys(changed) if f.endswith(".rs") and f.startswith("crates/") and os.path.exists(f)]
print("# affected tests: " + what)
if not changed and not scan_hits and not included:
    print("# no Rust file changed and no scan test's files: no tests to run"); sys.exit()

_cache = {}
def read(p):
    if p not in _cache:
        try: _cache[p] = open(p, encoding="utf-8", errors="replace").read()
        except OSError: _cache[p] = ""
    return _cache[p]
def crate_of(f): return f.split("/")[1]
def has_lib(c): return os.path.exists("crates/%s/src/lib.rs" % c)
def root_file(c): return "crates/%s/src/%s" % (c, "lib.rs" if has_lib(c) else "main.rs")
def target_of(f):
    """(crate, target flag, module path; "" for the crate root, None for a binary under src/bin/)."""
    c = crate_of(f)
    if not f.startswith("crates/%s/src/" % c): return None
    rel = f[len("crates/%s/src/" % c):]
    if rel.startswith("bin/"):
        b = rel[4:].split("/")[0].removesuffix(".rs")
        # A folder without main.rs holds a binary's modules, which it declares by
        # `#[path = "<folder>/…"]` (scenic-build's tools/, #917): that binary's.
        bdir = "crates/%s/src/bin" % c
        if "/" in rel[4:] and not os.path.exists("%s/%s/main.rs" % (bdir, b)):
            for s in sorted(os.listdir(bdir)):
                if s.endswith(".rs") and re.search(r'#\[path\s*=\s*"%s/' % re.escape(b), read(os.path.join(bdir, s))):
                    b = s.removesuffix(".rs"); break
        return (c, "--bin " + b, None)
    parts = rel.removesuffix(".rs").split("/")
    if parts[-1] == "mod": parts = parts[:-1]
    tgt = "--lib" if has_lib(c) else "--bin " + c
    # A file a sibling declares with `#[path = "<file>"] mod <name>;` (dem/generic.rs declaring
    # generic_tests.rs as `tests`) is that sibling's module <name>: dem::generic::tests.
    d, name = os.path.split(f)
    pat = re.compile(r'#\[path\s*=\s*"%s"\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;' % re.escape(name))
    for s in sorted(os.listdir(d)):
        sp = os.path.join(d, s)
        if s == name or not s.endswith(".rs"): continue
        m = pat.search(read(sp))
        if m:
            owner = target_of(sp)
            if owner and owner[2] is not None:
                return (c, tgt, (owner[2] + "::" if owner[2] else "") + m.group(1))
    return (c, tgt, "" if parts in ([], ["lib"], ["main"]) else "::".join(parts))
def mod_file(c, path):
    if not path: return root_file(c)
    base = "crates/%s/src/%s" % (c, path.replace("::", "/"))
    for f in (base + ".rs", base + "/mod.rs"):
        if os.path.exists(f): return f
    return None
# `#[cfg(test)] mod tests` (also cfg(all(test, …)), not cfg(not(test))), other attributes between.
TESTMOD = re.compile(r"#\[cfg\((?![^\]]*not\(test)[^\]]*\btest\b[^\]]*\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)")
def has_tests(src): return "#[test]" in src or "::test]" in src or TESTMOD.search(src)
def filters_for(c, path, f):
    """Filters for module `path`'s own tests (not its submodules'), [] when it has none."""
    src = read(f)
    if not has_tests(src): return []
    if not path:   # the crate root's tests are `tests::…`, a filter every module's tests match
        why.append("#   %s's own tests left out: no filter isolates them (run one by its exact name)" % f); return []
    pre = path + "::" if path else ""
    children = os.path.isdir("crates/%s/src/%s" % (c, path.replace("::", "/"))) if path else True
    tmods = list(dict.fromkeys(TESTMOD.findall(src)))
    if children and tmods: return [pre + m + "::" for m in tmods]
    return [pre]

want = OrderedDict()   # (crate, target flag) -> filters; None = the whole target
why = []
def add(c, tgt, filt, reason):
    want.setdefault((c, tgt), set()).add(filt)
    why.append("#   -p %s %s %s  (%s)" % (c, tgt, filt or "(whole)", reason))
def add_file(f, reason):
    t = target_of(f)
    if not t: return
    c, tgt, path = t
    if path is None:
        if has_tests(read(f)): add(c, tgt, None, reason)
        return
    for flt in filters_for(c, path, f): add(c, tgt, flt, reason)
def rg(pattern, paths, multiline=False):
    paths = [p for p in paths if os.path.exists(p)]
    if not paths: return []
    r = subprocess.run(["rg", "-l", "-P", "--type", "rust", *(["-U"] if multiline else []), "-e", pattern, "--", *paths], capture_output=True, text=True)
    return r.stdout.split()
def test_files_declaring(c, helper):
    return [t for t in rg(r"^\s*mod\s+%s\s*;" % re.escape(helper), ["crates/%s/tests" % c]) if t.count("/") == 3]

def aliases(c, path):
    """Where else crate c's module `path` is named, as (crate, module path) (#916: modules moved to
    their own crates keep their old paths): a crate's root re-exporting its top module
    (`pub use c::{…top…}`), and a module of a crate re-exporting a module of c by glob
    (`pub use c::<m>::*;`), and so on through those crates' own re-exports (none is left between
    the workspace's crates since #1084)."""
    out, todo = [], [(c, path)]
    while todo:
        c0, p0 = todo.pop(0)
        top, found = p0.split("::")[0], []
        for oc in sorted(os.listdir("crates")):
            if oc == c0 or not os.path.isdir("crates/%s/src" % oc): continue
            if re.search(r"pub use (?:::)?%s::(?:\{[^}]*\b%s\b|%s\b)" % (c0, re.escape(top), re.escape(top)), read(root_file(oc))):
                found.append((oc, p0))
        for oc, om, rc, rm in globs():
            if rc != c0 or oc == c0: continue
            if p0 == rm: found.append((oc, om))
            elif p0.startswith(rm + "::"): found.append((oc, om + p0[len(rm):]))
        for a in found:
            if a != (c, path) and a not in out: out.append(a); todo.append(a)
    return out
_globs = []
def globs():
    """(crate, module, the crate it re-exports from, that crate's module) for each top-level
    `pub use <crate>::<module>::*;` of a module's file."""
    if not _globs:
        crates = set(os.listdir("crates"))
        for g in rg(r"^pub use (?:::)?\w+(?:::\w+)+::\*;", ["crates"]):
            t = target_of(g) if "/src/" in g else None
            if not t or not t[2]: continue
            for m in re.finditer(r"^pub use (?:::)?(\w+)((?:::\w+)+)::\*;", read(g), re.M):
                if m.group(1) in crates: _globs.append((t[0], t[2], m.group(1), m.group(2)[2:]))
        _globs.append(None)
    return [x for x in _globs if x]
# `#[path = "<file>"] mod <name>;`, the file in another crate (agentcore's unit tests compile the
# plan's modules in, pipeline's agent/{build,steps,…}.rs; docs/crates.md §6.3, 6f4).
PATHMOD = re.compile(r'#\[path\s*=\s*"([^"]+)"\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;')
_pathed = {}
def compiled_into():
    """{file: [the files of other crates declaring it a module by #[path]]}."""
    if not _pathed:
        for g in rg(r"#\[path\s*=", ["crates"]):
            if "/src/" not in g: continue
            for m in PATHMOD.finditer(read(g)):
                t = os.path.normpath(os.path.join(os.path.dirname(g), m.group(1)))
                if crate_of(t) != crate_of(g): _pathed.setdefault(t, []).append(g)
        _pathed[None] = []
    return _pathed

def trace(c, path, kids, reason):
    """Add the tests of the files naming crate c's module `path` (kids: its submodules, whose
    users aren't its): its crate's `crate::<path>` (or `use crate::<parent>::{…<leaf>…}`), its
    binaries' `<c>::<path>`, the parent naming `<leaf>::`, a child using `super::`; every other
    crate's `<c>::<path>` in its sources, and in its integration tests (each run whole)."""
    src = "crates/%s/src" % c
    segs = path.split("::"); parent, leaf = "::".join(segs[:-1]), segs[-1]
    pp = re.escape(parent + "::") if parent else ""
    d = "crates/%s/src/%s" % (c, path.replace("::", "/"))
    kids = sorted(set(kids) | ({k.removesuffix(".rs") for k in os.listdir(d) if k != "mod.rs"} if os.path.isdir(d) else set()))
    notkid = r"(?!::(?:%s)\b)" % "|".join(map(re.escape, kids)) if kids else ""
    users = set(rg(r"crate::%s\b%s" % (re.escape(path), notkid), [src])) | set(rg(r"crate::%s\{[^}]*\b%s\b" % (pp, re.escape(leaf)), [src], True))
    pf = mod_file(c, parent)
    if pf and re.search(r"(?<![\w:])(?:self::)?%s::" % re.escape(leaf), read(pf)): users.add(pf)
    if os.path.isdir(d):
        for ch in sorted(os.listdir(d)):
            p = d + "/" + ch
            if os.path.isdir(p): p += "/mod.rs"
            elif not ch.endswith(".rs") or ch == "mod.rs": continue
            if "super::" in read(p): users.add(p)
    # (One rg over every crate, its hits sorted by where they are.)
    named = {}
    tdirs = sorted(glob.glob("crates/*/tests"))
    for h in rg(r"\b%s::%s\b%s" % (c, re.escape(path), notkid), ["crates"]):
        oc = crate_of(h)
        if h.startswith("crates/%s/tests/" % oc): named[h] = oc
        elif h.startswith("crates/%s/src/" % oc) and (oc != c or (has_lib(c) and h.startswith(src + "/bin/"))): users.add(h)
    for h in rg(r"\b%s::%s\{[^}]*\b%s\b" % (c, pp, re.escape(leaf)), tdirs, True): named[h] = crate_of(h)
    for u in sorted(users - {mod_file(c, path)}): add_file(u, "uses %s's %s%s" % (c, path, reason))
    for t, oc in sorted(named.items()):
        tdir = "crates/%s/tests" % oc
        rel = t[len(tdir) + 1:]
        if "/" in rel:
            for tt in test_files_declaring(oc, rel.split("/")[0]):
                add(oc, "--test " + os.path.basename(tt).removesuffix(".rs"), None, "names %s::%s%s (via %s)" % (c, path, reason, t))
        else: add(oc, "--test " + rel.removesuffix(".rs"), None, "names %s::%s%s" % (c, path, reason))

for f in changed:
    c = crate_of(f)
    if f.startswith("crates/%s/tests/" % c):
        rel = f[len("crates/%s/tests/" % c):]
        if "/" not in rel: add(c, "--test " + rel.removesuffix(".rs"), None, "changed")
        else:
            for t in test_files_declaring(c, rel.split("/")[0]):
                add(c, "--test " + os.path.basename(t).removesuffix(".rs"), None, "uses changed " + f)
        continue
    # A file another crate compiles in as a module (#[path]): that crate's unit tests, whole. Its code
    # reaches them through what the crate makes of it, not by name: the plan, in agentcore's tests
    # through seam::for_tests (~430 of them), as check-reuse.py counts it that crate's source.
    for g in compiled_into().get(f, []):
        oc = crate_of(g)
        add(oc, "--lib" if has_lib(oc) else "--bin " + oc, None, "%s compiles in %s by #[path]" % (g, f))
    t = target_of(f)
    if not t: continue
    _, tgt, path = t
    add_file(f, "changed: " + f)
    if not path:
        if path == "": why.append("#   %s is the crate root: its own items' users aren't traced (a new `mod` line's file is in the diff)" % f)
        continue
    d = "crates/%s/src/%s" % (c, path.replace("::", "/"))
    kids = sorted({k.removesuffix(".rs") for k in os.listdir(d) if k != "mod.rs"}) if os.path.isdir(d) else []
    trace(c, path, kids, "")
    for ac, ap in aliases(c, path):
        trace(ac, ap, kids, " (%s's %s re-exported)" % (c, path))

for c, tgt, reason in scan_hits: add(c, tgt, None, reason)
for f, p in included: add_file(f, "compiles in " + p)
if not want:
    print("# the changed Rust files have no tests and no tested users: nothing to run"); sys.exit()
print("# why:"); print("\n".join(dict.fromkeys(why)))
# A crate's whole targets in one command (#887): one cargo run builds and links them in parallel.
whole = OrderedDict()
for (c, tgt), fs in want.items():
    if None in fs: whole.setdefault(c, []).append(tgt)
for c, tgts in whole.items():
    print("cargo test -p %s %s" % (c, " ".join(tgts)))
for (c, tgt), fs in want.items():
    if None in fs: continue
    fs = sorted(fs)
    fs = [x for x in fs if not any(x != y and x.startswith(y) for y in fs)]
    print("cargo test -p %s %s -- %s" % (c, tgt, " ".join(fs)))
PY
