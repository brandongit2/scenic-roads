#!/bin/zsh
# The guards publishing runs before it builds (docs/inputs.md §7): the inputs folders read only through
# their accessor and no data source named in the app's code (the pipeline's inputs_accessor and
# source_names tests), then the Python steps the agent runs loaded from the app's dem/ placed in a
# stand-in NAS root whose inputs folders can't be read: a step that reads one as it loads, around
# dem/inputs.py's arg, fails there. Needs cargo and uv on the PATH (tools/app/publish.sh runs it
# so); exits non-zero, saying why, on a failure. The tests' whole output is kept
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
(cargo fmt --check && cargo fmt --check --manifest-path tools/bldfield/Cargo.toml) > $log 2>&1 || {
  head -40 $log
  echo "the Rust isn't formatted; run cargo fmt (rustfmt.toml)"
  exit 1
}
# No test measures the machine (#441): no short elapsed() bound asserted, no fixed sleep then an
# assert (tools/app/test_timing.py says why and what instead).
python3 tools/app/test_timing.py || exit 1
# A test reading the repo's own files says which (#662): `//! scans: <path prefix> …`, so
# tools/dev/affected-tests.sh runs it when any of them changes (write_sets missed #660's room.rs).
python3 - <<'PY' || exit 1
import glob, re, sys
bad = [t for t in sorted(glob.glob("crates/*/tests/*.rs"))
       if re.search(r'\b(?:root|repo)\(\)|CARGO_MANIFEST_DIR"\)\)\.(?:join|parent)\("\.\./|\.parent\(\)', s := open(t).read())
       and not re.search(r"^//! scans: \S", s, re.M)]
for t in bad: print(t + ": reads the repo's files without a `//! scans: <path prefix> …` line (tools/dev/testing.md)")
sys.exit(1 if bad else 0)
PY
cargo test -q -p pipeline --test inputs_accessor --test source_names > $log 2>&1 || {
  sed -n '/^---- /,/^failures:$/p' $log
  echo "a guard failed; the tests' whole output: $log"
  exit 1
}
# The stand-in root: the app's dem/ as it's published (the tracked files alone), beside inputs folders
# with a file each, none readable.
sr=$(mktemp -d)
trap 'chmod -R u+rwx $sr 2>/dev/null; rm -rf $sr' EXIT
mkdir -p $sr/dem
git ls-files dem | while read f; do mkdir -p "$(dirname "$sr/$f")" && cp -X "$f" "$sr/$f"; done
for d in inputs inputs/sources inputs/elevation translations descriptions; do
  mkdir -p $sr/$d && print x > $sr/$d/x
done
chmod 000 $sr/inputs/sources $sr/inputs/elevation $sr/inputs $sr/translations $sr/descriptions
# (Unreadable here unless this runs as root, which would prove nothing.)
[[ ! -r $sr/inputs ]] || { echo "the stand-in's inputs folders are readable (running as root?)"; exit 1; }
# The Python steps the build agent runs load from the app's dem/, which has nothing of the
# repository around it: each must import there.
steps=(${(f)"$(grep -rhoE '"[a-z_]+\.py"' crates/pipeline/src | tr -d '"' | sed 's/\.py$//' | sort -u)"})
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
