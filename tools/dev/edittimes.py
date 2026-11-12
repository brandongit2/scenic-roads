#!/usr/bin/env python3
"""Task #1025: what an edit of each file rebuilds, and how long (docs/crates.md §5, §7).

  tools/dev/edittimes.py [-n ROUNDS] [--cmd 'cargo …'] <file> …

Run in a built tree on the test Mac (through tools/app/remote-tests.sh, tools/dev/testing.md). For
each round, for each file in turn: one comment line appended to it, then the build command run and
timed (wall, CPU of its processes, `time -l`'s peak: the largest process), then the file put back
and the command run and timed again (a second edit of the same file). The command defaults to
§7.1's `cargo build --workspace --bins --timings`; with `--timings` in it, each workspace unit cargo
compiled is listed from its report (target/cargo-timings/cargo-timing.html): its start from the
build's start and its length (`start+length`), `rmeta` its metadata's time when cargo gives it (when
the units above it can start); units under 0.05 s (fresh, or a build script's run) are left out.
Interleaved (file a, b, … then a, b, … again), so the load's drift spreads over every file. Ends
with each file's medians. The files are restored on any exit.
"""
import glob
import json
import os
import re
import resource
import statistics
import subprocess
import sys
import time

args = sys.argv[1:]
rounds, cmd = 3, 'cargo build --workspace --bins --timings'
while args and args[0].startswith('-'):
    a = args.pop(0)
    if a == '-n':
        rounds = int(args.pop(0))
    elif a == '--cmd':
        cmd = args.pop(0)
    else:
        sys.exit(__doc__)
files = args
if not files:
    sys.exit(__doc__)
kept = {f: open(f, 'rb').read() for f in files}
target = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--no-deps'],
                        capture_output=True, text=True, check=True)
meta = json.loads(target.stdout)
target_dir = meta['target_directory']
ours = {p['name'].replace('-', '_') for p in meta['packages']}


def units():
    reps = sorted(glob.glob(os.path.join(target_dir, 'cargo-timings', 'cargo-timing-*.html')),
                  key=os.path.getmtime)
    if not reps:
        return []
    m = re.search(r'const UNIT_DATA = (\[.*?\]);\n', open(reps[-1]).read(), re.S)
    out = []
    for u in json.loads(m.group(1)) if m else []:
        if u['name'].replace('-', '_') not in ours or u['duration'] < 0.05:
            continue
        t = u.get('target', '').strip() or 'lib'
        r = u.get('rmeta_time')
        out.append(f"{u['name']}({t}) {u['start']:.1f}+{u['duration']:.1f}" +
                   (f" rmeta {r:.1f}" if r else ''))
    return out


def run():
    c0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.monotonic()
    p = subprocess.run(['/usr/bin/time', '-l', 'zsh', '-c', cmd], capture_output=True, text=True)
    wall = time.monotonic() - t0
    c1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (c1.ru_utime - c0.ru_utime) + (c1.ru_stime - c0.ru_stime)
    if p.returncode:
        sys.stderr.write(p.stdout + p.stderr)
        sys.exit(f'edittimes: {cmd!r} failed')
    m = re.search(r'(\d+)\s+maximum resident set size', p.stderr)
    peak = int(m.group(1)) / 1e9 if m else 0
    return wall, cpu, peak


def load():
    return subprocess.run(['sysctl', '-n', 'vm.loadavg'], capture_output=True,
                          text=True).stdout.split()[1]


res = {f: [] for f in files}
try:
    print(f'edittimes: warm build: {run()[0]:.1f} s (load {load()})', flush=True)
    for r in range(rounds):
        for f in files:
            for what in ('edit', 'back'):
                if what == 'edit':
                    open(f, 'ab').write(b'\n// edittimes\n')
                else:
                    open(f, 'wb').write(kept[f])
                ld = load()
                wall, cpu, peak = run()
                res[f].append((wall, cpu))
                print(f'{f} r{r + 1} {what}: wall {wall:.1f} s, cpu {cpu:.1f} s, peak {peak:.2f} GB, '
                      f'load {ld}: ' + ', '.join(units()), flush=True)
finally:
    for f, b in kept.items():
        open(f, 'wb').write(b)
print()
for f in files:
    w = [x for x, _ in res[f]]
    c = [y for _, y in res[f]]
    if w:
        print(f'{f}: median wall {statistics.median(w):.1f} s ({min(w):.1f}-{max(w):.1f}), '
              f'cpu {statistics.median(c):.1f} s, {len(w)} builds')
