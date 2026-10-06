"""The tree cover tiles of a few zoom-8 blocks from real data, made by dem/trees.py and by its Rust
port (the `trees` program), compared tile by tile (docs/plan.md §6, Trees): which tiles each made,
then per layer how many are the same bytes, how many the same pixels, and the largest difference of
a decoded value elsewhere. Then zoom 7–4 over those blocks (trees.py's `lower_zooms` against `trees
--assemble`). The port runs on each thread count given, its files compared byte for byte; with
--wasm, each block also runs as WebAssembly (Node's WASI), its squares read only from the bytes the
native run recorded (a fetch mirror), and is compared byte for byte too. Only --work is written.

    cd dem && uv run python ../tools/check/trees-same.py --coverage DIR --chm DIR --leaf DIR
        --blocks 8/x/y,… [--bin ../target/release] [--wasm ../target/wasm32-wasip1/release]
        [--threads 1,4] [--work DIR]

--coverage: each z3 tile's coverage as the trees program reads it, `3-<x>-<y>.json` (`scenic-build
trees-coverage 3/x/y --out … --root <NAS>`). --chm: the canopy squares (the agent's cache, or the
NAS's `sources/canopy`); --leaf: the leaf-type squares (the NAS's `sources/trees/leaf`). Exits 1 if
any tile's pixels differ, or the port's runs differ from each other.

    cd dem && uv run python ../tools/check/trees-same.py --compare <trees.py's out> <the port's out>

A whole z3 tile's archives (`trees-{cover,height,leaf}.tiles`), trees.py's `--z3` run's against the
trees program's (the same arguments), compared the same way.
"""
import io, json, shutil, struct, subprocess, sys, time
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "dem"))
import trees  # noqa: E402


def arg(name, default=None):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


def archive(p):
    """An archive's tiles: (z, x, y) → bytes."""
    b = Path(p).read_bytes()
    off, n = struct.unpack_from("<QQ", b, 8)
    out = {}
    for i in range(n):
        k, o, ln, _ = struct.unpack_from("<QQII", b, off + 24 * i)
        out[(k >> 58, (k >> 29) & ((1 << 29) - 1), k & ((1 << 29) - 1))] = b[o:o + ln]
    return out


def decoded(webp):
    """A tile's Terrarium values."""
    a = np.asarray(Image.open(io.BytesIO(webp)).convert("RGB")).astype(np.int64)
    return a[..., 0] * 256 + a[..., 1] + a[..., 2] / 256 - 32768


def files(d):
    return {f.name: f.read_bytes() for f in Path(d).iterdir()}


stats = {v: {"python": 0, "rust": 0, "only python": 0, "only rust": 0, "bytes": 0, "pixels": 0, "max diff": 0, "python bytes": 0, "rust bytes": 0} for v in trees.VARS}
bad = 0


def compare(py, rs, what):
    """Python's tiles (layer → {(z, x, y): webp}) against Rust's, into `stats`."""
    global bad
    for v in trees.VARS:
        s, a, b = stats[v], py.get(v, {}), rs.get(v, {})
        s["python"] += len(a)
        s["rust"] += len(b)
        s["only python"] += len(a.keys() - b.keys())
        s["only rust"] += len(b.keys() - a.keys())
        if a.keys() != b.keys():
            print(f"  {what} {v}: different tiles, only Python's {sorted(a.keys() - b.keys())[:5]}, only Rust's {sorted(b.keys() - a.keys())[:5]}")
            bad += 1
        s["python bytes"] += sum(len(x) for x in a.values())
        s["rust bytes"] += sum(len(x) for x in b.values())
        for k in sorted(a.keys() & b.keys()):
            s["bytes"] += a[k] == b[k]
            da, db = decoded(a[k]), decoded(b[k])
            if np.array_equal(da, db):
                s["pixels"] += 1
            else:
                d = float(np.abs(da - db).max())
                s["max diff"] = max(s["max diff"], d)
                print(f"  {what} {v} {k}: {int((da != db).sum())} pixels differ, by up to {d}")
                bad += 1


def summary():
    print()
    for v, s in stats.items():
        print(f"{v}: {s['python']} tiles from trees.py, {s['rust']} from the port ({s['only python']} only trees.py's, {s['only rust']} only the port's); "
              f"{s['bytes']} the same bytes, {s['pixels']} the same pixels, largest difference {s['max diff']}; "
              f"{s['python bytes'] / 1e6:.2f} MB from trees.py, {s['rust bytes'] / 1e6:.2f} MB from the port")


if "--compare" in sys.argv:
    i = sys.argv.index("--compare")
    py_dir, rs_dir = sys.argv[i + 1], sys.argv[i + 2]
    compare({v: archive(Path(py_dir) / f"trees-{v}.tiles") for v in trees.VARS}, {v: archive(Path(rs_dir) / f"trees-{v}.tiles") for v in trees.VARS}, "z3")
    summary()
    sys.exit(1 if bad else 0)

cov_dir, chm, leaf = Path(arg("--coverage")), arg("--chm"), arg("--leaf")
blocks = [tuple(int(v) for v in b.split("/")[1:]) for b in arg("--blocks").split(",")]
bin_dir = Path(arg("--bin", "../target/release")).resolve()
wasm_dir = arg("--wasm")
threads = [int(t) for t in arg("--threads", "1,4").split(",")]
work = Path(arg("--work", "/tmp/trees-same")).resolve()
here = Path(__file__).resolve().parent
shutil.rmtree(work, ignore_errors=True)
work.mkdir(parents=True)
times = {"python": [], **{f"rust {t}": [] for t in threads}, "wasm": []}
tops = {v: {} for v in trees.VARS}
for bx, by in blocks:
    name = f"8-{bx}-{by}"
    cov = cov_dir / f"3-{bx >> 5}-{by >> 5}.json"
    trees.load_shapes(str(cov))
    # trees.py's block, as its --z3 run makes it (the squares there are those it would use).
    w, s, e, n = trees.tile_bounds(8, bx, by)
    sqs = []
    for top in range(int(np.ceil(n / 10)) * 10, int(np.floor(s / 10)) * 10, -10):
        for left in range(int(np.floor(w / 10)) * 10, int(np.ceil(e / 10)) * 10, 10):
            if top > s and top - 10 < n and left < e and left + 10 > w:
                ps = [Path(chm) / f"meta_chm_lat={top}.0_lon={left}.0_{k}.tif" for k in ("cover5m", "p95")]
                if all(p.exists() and p.stat().st_size > 0 for p in ps):
                    sqs.append((top, left))
    t = time.time()
    _, tiles, t8 = trees.z3_block((bx, by, chm, leaf, sorted(sqs)))
    times["python"].append(time.time() - t)
    for v in trees.VARS:
        tops[v][(bx, by)] = t8[v]
    py = {}
    for v, z, x, y, img, _ in tiles:
        py.setdefault(v, {})[(z, x, y)] = img
    # The port, on each thread count (the first recording what it reads), the same bytes each time.
    first = None
    for th in threads:
        out = work / f"rust{th}" / name
        cmd = [str(bin_dir / "trees"), "--block", f"8/{bx}/{by}", "--coverage", str(cov), "--chm", chm, "--leaf", leaf, "--out", str(out), "--workers", str(th)]
        if first is None:
            cmd += ["--record", str(work / "mirror" / name)]
        t = time.time()
        r = subprocess.run(cmd, capture_output=True, text=True)
        times[f"rust {th}"].append(time.time() - t)
        if r.returncode != 0:
            sys.exit(f"trees --block 8/{bx}/{by}: {r.stderr}")
        print(f"{name}: {r.stderr.strip().splitlines()[-1]}")
        if first is None:
            first = out
            # What it read: each file's bytes (its ranges), as a task would be given them.
            read = {}
            for f in (work / "mirror" / name).rglob("*.ranges"):
                kind = "leaf type" if f.name.startswith("lat") else f.name.split("_")[-1].split(".")[0]
                n = sum(int(e) - int(s) for s, e in (l.split() for l in f.read_text().splitlines()))
                read[kind] = read.get(kind, 0) + n
            print("  read: " + ", ".join(f"{k} {n / 1e6:.1f} MB" for k, n in sorted(read.items())))
        elif files(out) != files(first):
            print(f"  {name}: {th} threads' files differ from {threads[0]}'s")
            bad += 1
    compare(py, {v: archive(first / f"trees-{v}.tiles") for v in trees.VARS}, name)
    if wasm_dir:
        out = work / "wasm" / name
        env = {"SCENIC_FETCH_MIRROR": str(work / "mirror" / name)}
        url = lambda d: "file://" + str(Path(d).resolve())
        t = time.time()
        r = subprocess.run(["node", str(here / "wasi-run.mjs"), str(Path(wasm_dir) / "trees.wasm"), json.dumps(env), "--block", f"8/{bx}/{by}", "--coverage", str(cov), "--chm", url(chm), "--leaf", url(leaf), "--out", str(out)], capture_output=True, text=True)
        times["wasm"].append(time.time() - t)
        if r.returncode != 0:
            sys.exit(f"trees.wasm --block 8/{bx}/{by}: {r.stderr}")
        print(f"  WebAssembly: {r.stderr.strip().splitlines()[-1]}")
        if files(out) != files(first):
            print(f"  {name}: WebAssembly's files differ from the native run's")
            bad += 1

# Zoom 7–4 over the blocks.
class Keep:
    def __init__(self):
        self.tiles = {}

    def add(self, z, x, y, blob, raw):
        self.tiles[(z, x, y)] = blob


keep = {v: Keep() for v in trees.VARS}
trees.lower_zooms(tops, trees.VARS, keep)
asm = work / "assembled"
r = subprocess.run([str(bin_dir / "trees"), "--assemble", "--out", str(asm), *[str(work / f"rust{threads[0]}" / f"8-{x}-{y}") for x, y in blocks]], capture_output=True, text=True)
if r.returncode != 0:
    sys.exit(f"trees --assemble: {r.stderr}")
lower = {v: {k: b for k, b in archive(asm / f"trees-{v}.tiles").items() if k[0] <= 7} for v in trees.VARS}
compare({v: keep[v].tiles for v in trees.VARS}, lower, "zoom 7–4")

summary()
for k, ts in times.items():
    if ts:
        print(f"{k}: {sum(ts) / len(ts):.2f} s a block (" + ", ".join(f"{t:.2f}" for t in ts) + ")")
sys.exit(1 if bad else 0)
