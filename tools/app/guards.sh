#!/bin/zsh
# The guards publishing runs before it builds (docs/inputs.md §7): the inputs folders read only through
# their accessor and no data source named in the app's code (the pipeline's inputs_accessor and
# source_names tests), then the Python steps the agent runs loaded from the app's dem/ placed in a
# stand-in NAS root whose inputs folders can't be read: a step that reads one as it loads, around
# dem/inputs.py's arg, fails there; and tools/dev's programs under one strict mode. Needs cargo and
# uv on the PATH (tools/app/publish.sh runs it so); exits non-zero, saying why, on a failure. The tests' whole output is kept
# (~/Library/Logs/scenic/guards-<time>.log); shown on a failure: each failed test's own.
set -euo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) print "usage: tools/app/guards.sh   (no arguments)"; exit 0 ;;
  *) print -u2 "guards.sh takes no arguments: $*"; exit 2 ;;
esac
cd ${0:A:h}/../..
mkdir -p ~/Library/Logs/scenic
log=~/Library/Logs/scenic/guards-$(date +%Y%m%d-%H%M%S).log
# With SCENIC_GUARDS_IN_CHECK=1 (tools/dev/check.sh, #887) the fmt check and the two tests below are
# left to check.sh's own parts: its fmt, and its tests, where tools/dev/affected-tests.sh runs both
# (scan tests of crates/) whenever a file they read changed.
in_check=${SCENIC_GUARDS_IN_CHECK:-}
[[ -n $in_check ]] || (cargo fmt --check && cargo fmt --check --manifest-path tools/bldfield/Cargo.toml) > $log 2>&1 || {
  head -40 $log
  echo "the Rust isn't formatted; run cargo fmt (rustfmt.toml)"
  exit 1
}
# No test measures the machine (#441): no short elapsed() bound asserted, no fixed sleep then an
# assert (tools/app/test_timing.py says why and what instead).
python3 tools/app/test_timing.py || exit 1
# Canadian spelling (#1374, tools/dev/agent-rules.md): no -ise/-yse spelling from tools/check/spelling.py's
# word list in anything tracked (a deliberate use says `spelling: kept`); its own test first.
python3 tools/check/test_spelling.py > $log 2>&1 || { cat $log; exit 1; }
python3 tools/check/spelling.py || exit 1
# One strict mode for tools/dev's programs (consistency T13): each runs under `set -uo pipefail`
# (unset names and failed pipelines are errors; no errexit: each says itself why it stops), set
# before its first command or, one its test sources, after its source guard. A file only sourced
# (no #! line: board.sh) sets none.
for f in tools/dev/*.sh; do
  [[ $(head -c2 $f) == '#!' ]] || continue
  grep -qx 'set -uo pipefail' $f || { echo "$f: a tools/dev program sets 'set -uo pipefail' (tools/app/guards.sh says where)"; exit 1; }
done
# The rollback floor names a commit this tree contains (tools/app/rollback-floor, #1342): a branch's
# commit rebased away as it merged, or a mistyped one, would refuse every rollback after it.
floor=$(head -1 tools/app/rollback-floor)
[[ $floor =~ '^[0-9a-f]{40}$' ]] && git merge-base --is-ancestor "$floor" HEAD 2>/dev/null \
  || { echo "tools/app/rollback-floor's first line, $floor, isn't the full id of a commit this tree contains (the file says what it names)"; exit 1; }
# A test reading the repo's own files says which (#662): `//! scans: <path prefix> …`, so
# tools/dev/affected-tests.sh runs it when any of them changes (write_sets missed #660's room.rs).
python3 - <<'PY' || exit 1
import glob, re, sys
bad = [t for t in sorted(glob.glob("crates/*/tests/*.rs"))
       if re.search(r'\b(?:root|repo)\(\)|CARGO_MANIFEST_DIR"\)\)\.(?:join|parent)\("\.\./|\babsolute\("\.\./|\.parent\(\)', s := open(t).read())
       and not re.search(r"^//! scans: \S", s, re.M)]
for t in bad: print(t + ": reads the repo's files without a `//! scans: <path prefix> …` line (tools/dev/testing.md)")
# (And the line covers what it names: `repo().join("README.md")` under no prefix of cli.rs's, #969.)
for t in sorted(glob.glob("crates/*/tests/*.rs")):
    s = open(t).read()
    m = re.search(r"^//! scans: (.+)$", s, re.M)
    pre = m.group(1).split() if m else []
    for p in re.findall(r'\b(?:root|repo)(?:\(\))?\.join\("([^"]+)"\)', s):
        if m and not any(p == q or p.startswith(q.rstrip("/") + "/") for q in pre):
            print(t + ": reads " + p + ", under no prefix of its `//! scans:` line (tools/dev/testing.md)"); bad.append(t)
sys.exit(1 if bad else 0)
PY
# A test finds a program it runs by the CARGO_BIN_EXE_* cargo and nextest set as it runs, never by
# env!'s compiled-in path (#1108): in an agent worktree that path is the shared build folder's,
# which another worktree's build may replace mid-run; the clones the shim runs are named only at
# run time (tools/dev/testing.md, "One build folder per Mac").
if grep -rnE 'env!\(\s*"CARGO_BIN_EXE_' crates --include='*.rs'; then
  echo "a test names its program by env!(\"CARGO_BIN_EXE_…\"): read std::env::var(\"CARGO_BIN_EXE_…\") as it runs (tools/app/guards.sh says why)"
  exit 1
fi
# No program is a copy of one of macOS's own (#1110): its launch constraints kill a copy of an Apple
# platform program as it starts (AMFI's "Launch Constraint Violation", SIGKILL), so a stand-in copied
# from /bin/sleep never ran, and its test passed only while a look caught it dying. A test copies
# its own program instead (tools/dev/testing.md, "A program that ran is kept 10 minutes").
if grep -rnE 'copy\(\s*&?(Path::new\(\s*)?"/(bin|sbin|usr/bin|usr/sbin|usr/libexec|System)/' crates --include='*.rs'; then
  echo "a copy of a macOS program, which its launch constraints kill as it starts: copy the test's own program, std::env::current_exe() (tools/app/guards.sh says why)"
  exit 1
fi
# No test reaches a live program's port on this Mac (#906: a unit test's coordinator on 127.0.0.1:8090,
# beside the mini's agent, took its jobs' connections): test code (Rust test modules and tests/, the
# node, shell, Swift and Python tests) names none of store::spawn::POOL_PORTS on a loopback or
# wildcard address, nor coord::PORT or net::DEFAULT_PORT; a line that only pins or quotes one says
# `pool port: not bound`. (A test's listen on one panics too: store::spawn::listen.)
# Nor does a test listen on a fixed port of its own (#914: tests run beside each other, and one's
# fixed or remembered port is another's at any time): a listen or bind names port 0, or a variable
# holding one the test was given (coord::test_port, whose listeners the process keeps).
python3 - <<'PY' || exit 1
import glob, re, sys
pool = re.search(r'POOL_PORTS: \[u16; \d+\] = \[([\d, ]+)\]', open("crates/store/src/spawn.rs").read()).group(1).replace(" ", "").split(",")
bad_re = re.compile(r"(?:127\.0\.0\.1|localhost|\[::1\]|0\.0\.0\.0)[:\"', ]+(?:%s)\b|\bcoord::PORT\b|\bDEFAULT_PORT\b" % "|".join(pool))
fixed = r"""\b(?:%s)\s*\(\s*(?:\(\s*(?:"[^"]*"|[\w:.\[\]]+)\s*,\s*|["'][^"']*:|)([1-9]\d{0,4})\b"""
fixed_re = re.compile(fixed % "listen|bind|listeners_on")
fixed_py = re.compile(fixed % "bind")  # (Python's socket.listen(n) takes a backlog, not a port.)
bad, fixed_at = [], []
def scan(path, text, first=1):
    for i, line in enumerate(text.split("\n"), first):
        if bad_re.search(line) and "pool port: not bound" not in line:
            bad.append(f"{path}:{i}: {line.strip()}")
        if (fixed_py if path.endswith(".py") else fixed_re).search(line):
            fixed_at.append(f"{path}:{i}: {line.strip()}")
for f in sorted(glob.glob("crates/*/src/**/*.rs", recursive=True)):
    s = open(f).read()
    if f.endswith("/tests.rs") or "/tests/" in f:
        scan(f, s)
    elif m := re.search(r"^#\[cfg\(test\)\]\s*\nmod tests\b", s, re.M):
        scan(f, s[m.start():], s[: m.start()].count("\n") + 1)
tests = set(glob.glob("crates/*/tests/**/*.rs", recursive=True))
for g in ["web/test/*", "tools/check/*.test.mjs", "tools/devtools/test/*", "tools/app/test/*", "tools/dev/test/*",
          "tools/status/test/*", "producers/**/test_*.py", "dem/**/test_*.py"]:
    tests |= set(glob.glob(g, recursive=True))
for f in sorted(tests):
    try:
        scan(f, open(f).read())
    except (IsADirectoryError, UnicodeDecodeError):
        pass
for b in bad: print(b + "  <- a test naming a live program's port on this Mac (store::spawn::POOL_PORTS): use port 0, or mark a line that only quotes it `pool port: not bound`")
for b in fixed_at: print(b + "  <- a test listening on a fixed port (#914): another test running beside may hold it; listen on port 0, or on coord::test_port's")
sys.exit(1 if bad or fixed_at else 0)
PY
[[ -n $in_check ]] || cargo test -q -p pipeline --test inputs_accessor --test source_names > $log 2>&1 || {
  sed -n '/^---- /,/^failures:$/p' $log
  echo "a guard failed; the tests' whole output: $log"
  exit 1
}
# The stand-in root: the app's dem/ as it's published (the tracked files alone), beside inputs folders
# with a file each, none readable. A kept folder, not removed here (tools/dev/ran.zsh, #1131): its
# .venv's libraries are loaded below, and Gatekeeper scans them after.
source ${0:A:h:h}/dev/ran.zsh
sr=$(ran_dir guards)
trap 'chmod -R u+rwx $sr 2>/dev/null' EXIT
mkdir -p $sr/dem
git ls-files dem | while read f; do mkdir -p "$(dirname "$sr/$f")" && cp -X "$f" "$sr/$f"; done
for d in inputs inputs/sources inputs/elevation translations descriptions; do
  mkdir -p $sr/$d && print x > $sr/$d/x
done
chmod 000 $sr/inputs/sources $sr/inputs/elevation $sr/inputs $sr/translations $sr/descriptions
# (Unreadable here unless this runs as root, which would prove nothing.)
[[ ! -r $sr/inputs ]] || { echo "the stand-in's inputs folders are readable (running as root?)"; exit 1; }
# The Python steps the build agent runs load from the app's dem/, which has nothing of the
# repository around it: each must import there (named in any crate's source: the steps live in
# several since task #916).
steps=(${(f)"$(grep -rhoE '"[a-z_]+\.py"' crates/*/src | tr -d '"' | sed 's/\.py$//' | sort -u)"})
(cd $sr/dem && uv run python -c "import importlib, sys; [importlib.import_module(m) for m in sys.argv[1:]]" $steps) || { echo "a Python step doesn't load from the app's dem/ in a NAS root whose inputs folders can't be read"; exit 1; }
# (Loading isn't running: a name used before it's bound, say, shows only then. pyflakes, a version
# kept, reads them and the modules of dem/ they import.)
read_too=(${(f)"$(cd $sr/dem && python3 -c '
import ast, sys
from pathlib import Path
todo, seen = sys.argv[1:], set()
while todo:
    m = todo.pop()
    if m in seen or not Path(m + ".py").exists():
        continue
    seen.add(m)
    for n in ast.walk(ast.parse(Path(m + ".py").read_text())):
        if isinstance(n, ast.Import):
            todo += [a.name.split(".")[0] for a in n.names]
        elif isinstance(n, ast.ImportFrom) and n.module and not n.level:
            todo.append(n.module.split(".")[0])
print("\n".join(sorted(seen)))' $steps)"})
(cd $sr/dem && uvx --quiet --from 'pyflakes==4.0.2' pyflakes ${read_too/%/.py}) || { echo "pyflakes finds a mistake in a Python step or what it imports"; exit 1; }
echo "guards: inputs folders through their accessor, no source in the code, ${#steps} Python steps load with the inputs folders unreadable"
