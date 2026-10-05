"""Every Rust step of a unit, from the snapshots `scenic-build unit-snap` took, run three ways and
compared byte for byte: natively on 1 thread, natively on many, and as WebAssembly under Node's
WASI (docs/plan.md §8, Determinism; docs/workers.md). The Python steps are left out.

    python3 tools/check/same.py <unit-snap --out dir> [--bin target/release]
        [--wasm target/wasm32-wasip1/release] [--threads 14] [--only step]

The WebAssembly builds come from tools/app/wasm.sh. Exits 1 if any step's outputs differ.
"""
import hashlib, json, os, re, shutil, subprocess, sys, time

def arg(name, default):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default

out = sys.argv[1]
snap, units = os.path.join(out, "snap"), os.path.join(out, "units")
here = os.path.dirname(os.path.abspath(__file__))
bin_dir, wasm_dir = arg("--bin", "target/release"), arg("--wasm", "target/wasm32-wasip1/release")
threads, only = arg("--threads", "14"), arg("--only", None)
folder = os.path.join(units, os.listdir(units)[0])
work = os.path.join(out, "same")
os.makedirs(work, exist_ok=True)

def hashes(d):
    got = {}
    for root, _, files in os.walk(d):
        for f in files:
            p = os.path.join(root, f)
            rel = os.path.relpath(p, d)
            if rel != "steps.log":
                with open(p, "rb") as fh:
                    got[rel] = hashlib.sha256(fh.read()).hexdigest()
    return got

bad = 0
for name in sorted(os.listdir(snap)):
    m = re.match(r"(\d\d) (.*) before$", name)
    if not m:
        continue
    lines = open(os.path.join(snap, name + ".cmd")).read().splitlines()
    prog = os.path.basename(lines[0][len("prog "):])
    if prog == "uv" or (only and only not in m.group(2)):
        continue
    args = [l[4:] for l in lines if l.startswith("arg ")]
    env = dict(l[4:].split("=", 1) for l in lines if l.startswith("env "))
    got, secs = {}, {}
    for kind in ["native 1", f"native {threads}", "wasm"]:
        w = os.path.join(work, kind.replace(" ", ""))
        shutil.rmtree(w, ignore_errors=True)
        subprocess.run(["cp", "-c", "-R", os.path.join(snap, name), w], check=True)
        a = [x.replace(folder, w) for x in args]
        e = {k: v.replace(folder, w) for k, v in env.items()}
        e["RAYON_NUM_THREADS"] = kind.split()[1] if kind.startswith("native") else "1"
        t = time.time()
        if kind.startswith("native"):
            r = subprocess.run(["nice", "-n", "19", os.path.join(bin_dir, prog), *a], env={**os.environ, **e}, capture_output=True, text=True)
        else:
            r = subprocess.run(["nice", "-n", "19", "node", os.path.join(here, "wasi-run.mjs"), os.path.join(wasm_dir, prog + ".wasm"), json.dumps(e), *a], capture_output=True, text=True)
        secs[kind] = round(time.time() - t, 1)
        if r.returncode != 0:
            print(f"{m.group(2)}: {kind} failed: {(r.stderr or r.stdout)[-400:]}")
            got[kind] = {}
            continue
        got[kind] = hashes(w)
    keys = set().union(*got.values())
    diff = sorted(k for k in keys if len({g.get(k) for g in got.values()}) > 1)
    bad += bool(diff)
    print(f"{m.group(2):34s} {'same' if not diff else 'DIFFERS: ' + ', '.join(diff[:5])}   " + "  ".join(f"{k} {v} s" for k, v in secs.items()), flush=True)
sys.exit(1 if bad else 0)
