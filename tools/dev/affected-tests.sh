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
# under crates/<crate>/tests/ that name it, or that changed. Modules without tests are left out.
# Lines starting with # explain; the others are the commands. No command for doc-only or non-Rust
# changes. A `#[path]` module's file maps to its declared module (dem/generic_tests.rs → dem::generic::tests).
# tools/dev/test-guard.sh holds agents' cargo test to this set.
usage="usage: tools/dev/affected-tests.sh [<commit> | <a>..<b>]"
case $#:${1:-} in
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  0:|1:[^-]*) ;;
  *) print -ru2 -- "affected-tests.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
exec python3 - "$@" <<'PY'
import glob, os, re, subprocess, sys
from collections import OrderedDict
args = sys.argv[1:]
def git(*a):
    r = subprocess.run(["git", *a], capture_output=True, text=True)
    if r.returncode: sys.exit("git %s: %s" % (" ".join(a), r.stderr.strip()))
    return r.stdout
root = git("rev-parse", "--show-toplevel").strip()
os.chdir(root)
if not args:
    base = git("merge-base", "main", "HEAD").strip()
    changed = git("diff", "--name-only", base).split() + git("ls-files", "--others", "--exclude-standard").split()
    what = "this branch against main (merge-base %s), working tree included" % base[:7]
elif ".." in args[0]:
    changed = git("diff", "--name-only", args[0]).split(); what = args[0]
else:
    changed = git("diff", "--name-only", args[0] + "^", args[0]).split(); what = "commit " + args[0][:9]
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
# Files as they are in this checkout (a deleted file's module is gone; its users' own changes show).
changed = [f for f in dict.fromkeys(changed) if f.endswith(".rs") and f.startswith("crates/") and os.path.exists(f)]
print("# affected tests: " + what)
if not changed and not scan_hits:
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
    if rel.startswith("bin/"): return (c, "--bin " + rel[4:].split("/")[0].removesuffix(".rs"), None)
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

for f in changed:
    c = crate_of(f)
    if f.startswith("crates/%s/tests/" % c):
        rel = f[len("crates/%s/tests/" % c):]
        if "/" not in rel: add(c, "--test " + rel.removesuffix(".rs"), None, "changed")
        else:
            for t in test_files_declaring(c, rel.split("/")[0]):
                add(c, "--test " + os.path.basename(t).removesuffix(".rs"), None, "uses changed " + f)
        continue
    t = target_of(f)
    if not t: continue
    _, tgt, path = t
    add_file(f, "changed: " + f)
    if not path:
        if path == "": why.append("#   %s is the crate root: its own items' users aren't traced (a new `mod` line's file is in the diff)" % f)
        continue
    src = "crates/%s/src" % c
    segs = path.split("::"); parent, leaf = "::".join(segs[:-1]), segs[-1]
    pp = re.escape(parent + "::") if parent else ""
    # A submodule's users aren't this module's: `crate::agent::steps::…` doesn't use agent/mod.rs.
    d = "crates/%s/src/%s" % (c, path.replace("::", "/"))
    kids = sorted({k.removesuffix(".rs") for k in os.listdir(d) if k != "mod.rs"}) if os.path.isdir(d) else []
    notkid = r"(?!::(?:%s)\b)" % "|".join(map(re.escape, kids)) if kids else ""
    users = set(rg(r"crate::%s\b%s" % (re.escape(path), notkid), [src])) | set(rg(r"crate::%s\{[^}]*\b%s\b" % (pp, re.escape(leaf)), [src], True))
    if has_lib(c):
        users |= set(rg(r"\b%s::%s\b%s" % (c, re.escape(path), notkid), [src + "/bin"]))
    pf = mod_file(c, parent)
    if pf and re.search(r"(?<![\w:])(?:self::)?%s::" % re.escape(leaf), read(pf)): users.add(pf)
    if os.path.isdir(d):
        for ch in sorted(os.listdir(d)):
            p = d + "/" + ch
            if os.path.isdir(p): p += "/mod.rs"
            elif not ch.endswith(".rs") or ch == "mod.rs": continue
            if "super::" in read(p): users.add(p)
    for u in sorted(users - {f}): add_file(u, "uses " + path)
    tdir = "crates/%s/tests" % c
    named = set(rg(r"\b%s::%s\b%s" % (c, re.escape(path), notkid), [tdir])) | set(rg(r"\b%s::%s\{[^}]*\b%s\b" % (c, pp, re.escape(leaf)), [tdir], True))
    for t in sorted(named):
        rel = t[len(tdir) + 1:]
        if "/" in rel:
            for tt in test_files_declaring(c, rel.split("/")[0]):
                add(c, "--test " + os.path.basename(tt).removesuffix(".rs"), None, "names %s (via %s)" % (path, t))
        else: add(c, "--test " + rel.removesuffix(".rs"), None, "names " + path)

for c, tgt, reason in scan_hits: add(c, tgt, None, reason)
if not want:
    print("# the changed Rust files have no tests and no tested users: nothing to run"); sys.exit()
print("# why:"); print("\n".join(dict.fromkeys(why)))
for (c, tgt), fs in want.items():
    if None in fs:
        print("cargo test -p %s %s" % (c, tgt)); continue
    fs = sorted(fs)
    fs = [x for x in fs if not any(x != y and x.startswith(y) for y in fs)]
    print("cargo test -p %s %s -- %s" % (c, tgt, " ".join(fs)))
PY
