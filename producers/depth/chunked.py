#!/usr/bin/env python3
"""A depth source too big for the disk at once (BlueTopo ~97 GB, NONNA-10 ~90 GB), made a chunk at a
time: each chunk's files fetched (bluetopo.py's and nonna.py's own URLs, checks and specs; a
dropped connection retried), its datum surfaces solved over the chunk's water, its grids produced,
tarred over SSH to `--nas host:dir` (the NAS's sources/fetched/<id>/), the byte totals compared
there, and the chunk's files deleted here.
Each chunk's index part is kept in <work>/parts/; a chunk whose part exists is skipped, so the same
command resumes. After the last chunk the parts merge into <work>/<id>-grids.geojson.

- `bluetopo <scheme.gpkg> <vdatum.csv>`: every tile in the scheme, sorted by name; NAVD88 tiles
  brought to MSL by `msl_minus_navd88_m`, the others (Puerto Rico and the Virgin Islands, on MSL)
  not; MLLW below MSL the low water (vdatum.py's table).
- `nonna10 <nonna-100 dir> <stations.csv>`: NONNA-10's 0.5° × 0.3° blocks from the coverage's
  origin (142° W, 41° N) that NONNA-100 has water in (its fetched blocks, or the
  grids made of them in sources/fetched/nonna-100/), since
  NONNA-10 lies within NONNA-100; a block the service has nothing for is left out; chart datum → MSL
  by CHS's stations (stations.py chs), as nonna.py does.

Datum surfaces are a chunk's own (a chunk's tiles lie together, by name or by block), so where two
chunks meet their surfaces can differ by what the stations between them say differently.

usage: chunked.py bluetopo <gpkg> <vdatum.csv> <work dir> --nas <host>:<dir> [--size 300]
       chunked.py nonna10 <nonna-100 dir> <stations.csv> <work dir> --nas <host>:<dir> [--size 300] [--count]
"""
from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import time
import urllib.error
from pathlib import Path

import numpy as np

from datums import from_table, water_of
from grid import Writer, fetch, log

# NONNA-10's blocks: the service's own index cells.
N10_ORIGIN, N10_BLOCK = (-142.0, 41.0), (0.5, 0.3)


def fetch_retry(url: str, dest: Path, pause: float, tries: int = 5) -> Path:
    """grid.fetch, again after a dropped connection (30 s, 60 s, …); an HTTP answer isn't retried."""
    for k in range(tries):
        try:
            return fetch(url, dest, pause=pause)
        except urllib.error.HTTPError:
            raise
        except OSError as e:
            log(f"{url}: {e} (try {k + 1})")
            time.sleep(30 * (k + 1))
    raise SystemExit(f"{url}: failed {tries} times")


def bluetopo_items(gpkg: Path):
    from bluetopo import scheme_tiles

    return sorted(scheme_tiles(gpkg, [-180, -90, 180, 90]), key=lambda t: t["tile"])


def nonna10_items(n100: Path):
    src = sorted(p for p in n100.glob("*.tif") if not p.name.endswith((".low.tif", ".zoom.tif")))
    if not src:
        raise SystemExit(f"{n100}: no NONNA-100 .tif")
    water, w, n, r = water_of(src, 0.05)
    ys, xs = np.nonzero(water)
    (w0, s0), (bw, bh) = N10_ORIGIN, N10_BLOCK
    keys = sorted({(int((w + (x + 0.5) * r - w0) // bw), int((n - (y + 0.5) * r - s0) // bh)) for x, y in zip(xs, ys)})
    return [(w0 + bx * bw, s0 + by * bh, w0 + (bx + 1) * bw, s0 + (by + 1) * bh) for bx, by in keys if bx >= 0 and by >= 0]


def bluetopo_chunk(chunk, table: str, cache: Path, w: Writer) -> list[Path]:
    import rasterio

    from bluetopo import INTERPOLATED_ZOOM, interpolated_values, sha256
    from raster import Spec, produce

    have = []
    for t in chunk:
        name = t["tif"].rsplit("/", 1)[1]
        tif, aux = cache / name, cache / (name + ".aux.xml")
        fresh = not tif.exists()
        fetch_retry(t["tif"], tif, 0.2)
        fetch_retry(t["rat"], aux, 0.2)
        if fresh and t["sha"] and sha256(tif) != t["sha"].lower():
            tif.unlink()
            raise SystemExit(f"{name}: its checksum isn't the scheme's")
        with rasterio.open(tif) as d:
            navd = "navd88" in d.crs.to_wkt().lower()
        have.append((t, tif, aux, name, navd))
    nt = [h[1] for h in have if h[4]]
    offset = from_table((table, "msl_minus_navd88_m", frozenset(), "id"), nt) if nt else None
    low = from_table((table, "msl_minus_mllw_m", frozenset(), "id"), [h[1] for h in have])
    for t, tif, aux, name, navd in have:
        interp = np.asarray(interpolated_values(aux.read_bytes()) if aux.exists() else [], "float32")
        spec = Spec(band=1, flags=[(None, 3, lambda c, v=interp: np.isin(c, v), INTERPOLATED_ZOOM)], offset=offset if navd else None, low=low)
        produce(tif, spec, w, t["tif"], name.removesuffix(".tiff"))
    return [p for h in have for p in (h[1], h[2])]


def nonna10_chunk(chunk, table: str, cache: Path, w: Writer) -> list[Path]:
    from nonna import COVERAGE, block_url, spec
    from raster import produce

    files = []
    for b in chunk:
        u = block_url(COVERAGE["10"], *b)
        p = cache / f"nonna10_{b[0]:.2f}_{b[1]:.2f}.tif"
        try:
            files.append((fetch_retry(u, p, 0.5), u))
        except OSError as e:
            log(f"block {b}: {e}")
    if files:
        s = spec("10", from_table((table, "mwl_above_cd_m", frozenset(), "id"), [p for p, _ in files]))
        for p, u in files:
            produce(p, s, w, u, f"nonna10-{p.stem}")
    return [p for p, _ in files]


def to_nas(out: Path, nas: str) -> tuple[int, int]:
    """`out`'s files tarred into the NAS folder; their count and bytes, checked there."""
    host, dest = nas.split(":", 1)
    files = sorted(p.name for p in out.iterdir())
    local = sum((out / f).stat().st_size for f in files)
    ssh = ["ssh", "-o", "BatchMode=yes", host]
    tar = subprocess.Popen(["tar", "--no-xattrs", "-cf", "-", "-C", str(out), *files], stdout=subprocess.PIPE,
                           env={"COPYFILE_DISABLE": "1", "PATH": "/usr/bin:/bin"})
    subprocess.run([*ssh, f"mkdir -p {shlex.quote(dest)} && cd {shlex.quote(dest)} && tar xf -"], stdin=tar.stdout, check=True)
    tar.stdout.close()
    if tar.wait() != 0:
        raise SystemExit(f"tar of {out} failed")
    listing = subprocess.run([*ssh, f"cd {shlex.quote(dest)} && ls -l"], capture_output=True, text=True, check=True).stdout
    sizes = {l.split()[-1]: int(l.split()[4]) for l in listing.splitlines() if l.startswith("-")}
    remote = sum(sizes.get(f, -10**12) for f in files)
    if remote != local:
        raise SystemExit(f"{out}: the NAS has {remote} bytes of {local}")
    return len(files), local


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("which", choices=["bluetopo", "nonna10"])
    ap.add_argument("items", type=Path, help="bluetopo: the tile scheme; nonna10: NONNA-100's .tif folder")
    ap.add_argument("table", help="bluetopo: vdatum.py's CSV; nonna10: stations.py chs's")
    ap.add_argument("work", type=Path)
    ap.add_argument("--nas", help="host:dir, the NAS's sources/fetched/<id>/")
    ap.add_argument("--size", type=int, default=300)
    ap.add_argument("--count", action="store_true", help="say how many items and chunks, and stop")
    a = ap.parse_args()
    items = bluetopo_items(a.items) if a.which == "bluetopo" else nonna10_items(a.items)
    chunks = [items[i:i + a.size] for i in range(0, len(items), a.size)]
    log(f"{len(items)} {'tiles' if a.which == 'bluetopo' else 'blocks'} in {len(chunks)} chunks")
    if a.count:
        return
    if not a.nas or ":" not in a.nas:
        raise SystemExit("--nas host:dir")
    parts, cache = a.work / "parts", a.work / "cache"
    parts.mkdir(parents=True, exist_ok=True)
    for ci, chunk in enumerate(chunks):
        part = parts / f"part-{ci:03d}.geojson"
        if part.exists():
            continue
        out = a.work / f"out-{ci:03d}"
        w = Writer(out)
        fetched = (bluetopo_chunk if a.which == "bluetopo" else nonna10_chunk)(chunk, a.table, cache, w)
        n, size = to_nas(out, a.nas) if out.exists() and any(out.iterdir()) else (0, 0)
        w.write_index(part)
        if out.exists():
            for p in out.iterdir():
                p.unlink()
            out.rmdir()
        for p in fetched:
            p.unlink(missing_ok=True)
        log(f"chunk {ci + 1}/{len(chunks)}: {n} files, {size / 1e9:.2f} GB on the NAS")
    feats = []
    for p in sorted(parts.glob("part-*.geojson")):
        feats += json.loads(p.read_text())["features"]
    feats.sort(key=lambda f: f["properties"]["file"])
    index = a.work / f"{'bluetopo' if a.which == 'bluetopo' else 'nonna-10'}-grids.geojson"
    with open(index, "w") as f:
        json.dump({"type": "FeatureCollection", "features": feats}, f, separators=(",", ":"))
        f.write("\n")
    log(f"{index}: {len(feats)} grids")


if __name__ == "__main__":
    main()
