#!/usr/bin/env python3
"""A depth source too big for the disk at once (BlueTopo ~97 GB, NONNA-10 ~90 GB), made a chunk at a
time: each chunk's files fetched (bluetopo.py's and nonna.py's own URLs, checks and specs; a
transient failure retried, any other stopping the run), its datum surfaces solved over the chunk's water, its grids produced,
tarred over SSH to `--nas host:dir` (the NAS's sources/fetched/<id>/), each file's size compared
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
import glob
import http.client
import json
import math
import shlex
import shutil
import socket
import subprocess
import time
import urllib.error
from pathlib import Path

import numpy as np

from datums import OpenSea, from_table, water_of
from grid import Writer, fetch, log

# NONNA-10's blocks: the service's own index cells.
N10_ORIGIN, N10_BLOCK = (-142.0, 41.0), (0.5, 0.3)


class Unrenderable(SystemExit):
    """fetch_retry's stop when its last two tries were both a 504 or a timeout: the server couldn't
    render the subset in time, and that repeats, so a smaller subset may work. (A SystemExit, so
    uncaught it stops the run.)"""


def _timed_out(e: BaseException) -> bool:
    return isinstance(e, (socket.timeout, TimeoutError)) or (
        isinstance(e, urllib.error.URLError) and isinstance(e.reason, (socket.timeout, TimeoutError)))


def is_tiff(p: Path) -> bool:
    """Whether `p` starts with a TIFF or BigTIFF header (II*\\0, MM\\0*, II+\\0, MM\\0+)."""
    with open(p, "rb") as f:
        return f.read(4) in (b"II*\x00", b"MM\x00*", b"II+\x00", b"MM\x00+")


def fetch_retry(url: str, dest: Path, pause: float, tries: int = 5) -> Path:
    """grid.fetch, again (30 s, 60 s, …, none after the last try) after a dropped or refused
    connection, a timeout, a body cut short (http.client's IncompleteRead), 429 or a 5xx; any other HTTP
    answer, or the last try's failure, stops the run (SystemExit), so no chunk's part is written with a
    file missing. Two 504s or timeouts in a row (a render timeout repeats) raise Unrenderable at once,
    without the remaining tries."""
    slow = 0
    for k in range(tries):
        try:
            got = fetch(url, dest, pause=pause)
            if not is_tiff(got):
                # (The WCS answers some server failures 200 with an OWS exception report, not a TIFF:
                # "Failed to read the coverage … Connection reset". Retried as a 5xx.)
                why = got.read_bytes()[:400].decode("utf-8", "replace").replace("\n", " ")
                got.unlink(missing_ok=True)
                log(f"{url}: not a TIFF ({why[:200]}) (try {k + 1})")
                slow = 0
            else:
                return got
        except urllib.error.HTTPError as e:
            if e.code != 429 and e.code < 500:
                raise SystemExit(f"{url}: HTTP {e.code}")
            log(f"{url}: HTTP {e.code} (try {k + 1})")
            slow = slow + 1 if e.code == 504 else 0
        except (OSError, http.client.HTTPException) as e:
            log(f"{url}: {e!r} (try {k + 1})")
            slow = slow + 1 if _timed_out(e) else 0
        if slow >= 2:
            raise Unrenderable(f"{url}: 504 or timeout twice in a row")
        if k + 1 < tries:
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



# A NONNA-10 block is split no further when its quarters would be under this many metres a side.
MIN_SPLIT_M = 1000.0
NODATA = float(np.finfo("float32").max)
# NONNA-10's pixel grid in EPSG:3857, as the service answers a subset: one grid for the whole coverage,
# its pixels 19.0986 m × 19.1202 m (not square), a subset's edges snapped to its lines. Read on 17 Oct
# from 1 km subsets near Halifax, Vancouver and the NS-coast block that 504s: subsets requested on these
# lines came back with exactly those edges (so quarters abut with no gap or overlap), while subsets
# requested off them came back snapped by the service to a line, with a different row count for the
# same Y range at times (so their quarters could leave a nodata row).
GRID_X0, GRID_Y1, GRID_RX, GRID_RY = -7081005.895301013, 5499015.35251896, 19.098649920770324, 19.120193157291325


def on_grid(box: tuple[float, float, float, float]) -> tuple[float, float, float, float]:
    """`box` (x0, y0, x1, y1) with its outer edges moved outward to the service's pixel lines (x0 and y0
    down, x1 and y1 up; an edge already on a line, within 1e-6 pixel, stays). So the quarters of a
    split block cover at least what the service would give the whole block: beside an unsplit
    block they can overlap by a pixel, never leave a gap."""
    x0, y0, x1, y1 = box
    e = 1e-6
    return (GRID_X0 + math.floor((x0 - GRID_X0) / GRID_RX + e) * GRID_RX,
            GRID_Y1 - math.ceil((GRID_Y1 - y0) / GRID_RY - e) * GRID_RY,
            GRID_X0 + math.ceil((x1 - GRID_X0) / GRID_RX - e) * GRID_RX,
            GRID_Y1 - math.floor((GRID_Y1 - y1) / GRID_RY + e) * GRID_RY)


def mid_on_grid(a: float, b: float, y: bool = False) -> float:
    """The middle of `a`, `b` moved to the nearest pixel line (of the Y lines if `y`)."""
    m = (a + b) / 2
    if y:
        return GRID_Y1 - round((GRID_Y1 - m) / GRID_RY) * GRID_RY
    return GRID_X0 + round((m - GRID_X0) / GRID_RX) * GRID_RX


def mosaic(subs: list[Path], dest: Path) -> None:
    """The quarter files in `subs` into `dest`, a float32 TIFF of their union. Anything but quarters of
    one grid that tile their union exactly (one CRS and resolution, each offset a whole number of
    pixels within 1e-6, none overlapping or leaving a gap, nodata float32's max) stops the run
    (SystemExit): a mismatch would shift or lose rows silently. (By hand: rasterio.merge drops values
    when the nodata is float32's max.)"""
    import rasterio
    from rasterio.transform import from_origin

    srcs = [rasterio.open(p) for p in subs]
    try:
        d0 = srcs[0]
        rx, ry = d0.res
        for p, d in zip(subs, srcs):
            if d.crs != d0.crs or d.count != d0.count or d.dtypes != d0.dtypes:
                raise SystemExit(f"{p}: not the CRS, bands or type of the other quarters")
            if abs(d.res[0] - rx) > rx * 1e-9 or abs(d.res[1] - ry) > ry * 1e-9:
                raise SystemExit(f"{p}: resolution {d.res} isn't {d0.res}")
            if d.nodata != NODATA:
                raise SystemExit(f"{p}: nodata {d.nodata} isn't float32's max")
        xmin, ymax = min(d.bounds.left for d in srcs), max(d.bounds.top for d in srcs)
        xmax, ymin = max(d.bounds.right for d in srcs), min(d.bounds.bottom for d in srcs)
        wd, ht = (xmax - xmin) / rx, (ymax - ymin) / ry
        if abs(wd - round(wd)) > 1e-6 or abs(ht - round(ht)) > 1e-6:
            raise SystemExit(f"{dest}: the quarters' union isn't a whole number of pixels")
        wd, ht = round(wd), round(ht)
        data = np.full((d0.count, ht, wd), NODATA, "float32")
        covered = np.zeros((ht, wd), bool)
        for p, d in zip(subs, srcs):
            c0, r0 = (d.bounds.left - xmin) / rx, (ymax - d.bounds.top) / ry
            if abs(c0 - round(c0)) > 1e-6 or abs(r0 - round(r0)) > 1e-6:
                raise SystemExit(f"{p}: not on the pixel grid of the others (offset {c0:.4f}, {r0:.4f} pixels)")
            c0, r0 = round(c0), round(r0)
            if r0 + d.height > ht or c0 + d.width > wd:
                raise SystemExit(f"{p}: reaches past the union")
            if covered[r0:r0 + d.height, c0:c0 + d.width].any():
                raise SystemExit(f"{p}: overlaps another quarter")
            covered[r0:r0 + d.height, c0:c0 + d.width] = True
            data[:, r0:r0 + d.height, c0:c0 + d.width] = d.read()
        if not covered.all():
            raise SystemExit(f"{dest}: the quarters leave a gap")
        prof = {**d0.profile, "height": ht, "width": wd, "transform": from_origin(xmin, ymax, rx, ry), "nodata": NODATA,
                "dtype": "float32"}
    finally:
        for d in srcs:
            d.close()
    tmp = dest.with_suffix(dest.suffix + ".part")
    with rasterio.open(tmp, "w", **prof) as o:
        o.write(data)
    tmp.replace(dest)


def fetch_split(cov: str, box: tuple[float, float, float, float], dest: Path, pause: float, tries: int = 5) -> Path:
    """The coverage's subset `box` (EPSG:3857 x0, y0, x1, y1) into `dest`. When the last two tries were
    a 504 or timeout the box is split into 4 quarters on the service's pixel grid (on_grid), each
    fetched the same way (recursively, down to MIN_SPLIT_M) and mosaicked into `dest`, so the file
    holds the same cells as one fetch (its edges at most half a pixel from the service's own snapping).
    A quarter failing at the minimum size, or any other failure, stops the run (nothing is dropped).
    Quarter files already in the folder (a resumed run) skip the whole block's tries."""
    from nonna import merc_url

    x0, y0, x1, y1 = box
    resumed = dest.parent.is_dir() and any(dest.parent.glob(f"{glob.escape(dest.stem)}_q*{dest.suffix}")) and not dest.exists()
    if not resumed:
        try:
            return fetch_retry(merc_url(cov, *box), dest, pause, tries)
        except Unrenderable:
            if min(x1 - x0, y1 - y0) / 2 < MIN_SPLIT_M:
                raise
    log(f"{dest.name}: {'quarters already fetched' if resumed else '504 or timeout twice'}; splitting into 4 quarters")
    x0, y0, x1, y1 = on_grid(box)
    xm, ym = mid_on_grid(x0, x1), mid_on_grid(y0, y1, y=True)
    quarters = [(x0, y0, xm, ym), (xm, y0, x1, ym), (x0, ym, xm, y1), (xm, ym, x1, y1)]
    subs = [fetch_split(cov, q, dest.with_name(f"{dest.stem}_q{i}{dest.suffix}"), pause, tries) for i, q in enumerate(quarters)]
    mosaic(subs, dest)
    for p in subs:
        p.unlink(missing_ok=True)
    return dest


def nonna10_chunk(chunk, table: str, cache: Path, w: Writer, sea: OpenSea | None = None) -> list[Path]:
    from grid import lonlat_to_merc
    from nonna import COVERAGE, block_url, spec
    from raster import produce

    files = []
    for b in chunk:
        u = block_url(COVERAGE["10"], *b)
        p = cache / f"nonna10_{b[0]:.2f}_{b[1]:.2f}.tif"
        # (A block with no data comes as a whole TIFF of nodata, 200, which makes no grid: the
        # service has no empty answer, so every failure stops the run.)
        (x0, y0), (x1, y1) = lonlat_to_merc(b[0], b[1]), lonlat_to_merc(b[2], b[3])
        files.append((fetch_split(COVERAGE["10"], (float(x0), float(y0), float(x1), float(y1)), p, 0.5), u))
    if files:
        s = spec("10", from_table((table, "mwl_above_cd_m", frozenset(), "id"), [p for p, _ in files], sea=sea))
        for p, u in files:
            produce(p, s, w, u, f"nonna10-{p.stem}")
    # (The files to delete: the blocks, and any quarters or .part files a split left.)
    return [p for p, _ in files] + sorted(cache.glob("nonna10_*_q*"))


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
    bad = size_mismatches({f: (out / f).stat().st_size for f in files}, listing)
    if bad:
        raise SystemExit(f"{out}: on the NAS not as here: {bad[:10]}{' …' if len(bad) > 10 else ''}")
    return len(files), local


def size_mismatches(local: dict[str, int], listing: str) -> list[str]:
    """The files whose size in `ls -l`'s `listing` isn't the local one (or that it lacks)."""
    there = {}
    for l in listing.splitlines():
        if l.startswith("-"):
            f = l.split(None, 8)
            there[f[8]] = int(f[4])
    return [f"{n}: {there.get(n)} of {b}" for n, b in sorted(local.items()) if there.get(n) != b]


def estimate(cache: Path, which: str, size: int) -> str:
    """A chunk's fetched bytes, from the files already in `cache` (none there: unknown)."""
    pats = ("*.tiff", "*.tiff.aux.xml") if which == "bluetopo" else ("nonna10_*.tif",)
    sizes = [[p.stat().st_size for p in cache.glob(g) if p.is_file() and "_q" not in p.name] for g in pats] if cache.is_dir() else []
    if not sizes or not sizes[0]:
        return "a chunk's bytes unknown (nothing fetched yet to measure)"
    per = sum(sum(x) / len(x) for x in sizes if x)
    return f"about {per * size / 1e9:.1f} GB fetched a chunk ({per / 1e6:.1f} MB an item, from {len(sizes[0])} in the cache)"


def free(path: Path) -> str:
    return f"{shutil.disk_usage(path).free / 1e9:.1f} GB free"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("which", choices=["bluetopo", "nonna10"])
    ap.add_argument("items", type=Path, help="bluetopo: the tile scheme; nonna10: NONNA-100's .tif folder")
    ap.add_argument("table", help="bluetopo: vdatum.py's CSV; nonna10: stations.py chs's")
    ap.add_argument("work", type=Path)
    ap.add_argument("--nas", help="host:dir, the NAS's sources/fetched/<id>/")
    ap.add_argument("--size", type=int, default=300)
    ap.add_argument("--lakes", type=Path, help="nonna10 (required): Natural Earth's SQLite (sources/basemap/natural_earth_vector.sqlite.zip, unzipped), "
                    "the inland lakes kept out of the datum surfaces' open-sea extrapolation (datums.OpenSea)")
    ap.add_argument("--count", action="store_true", help="say how many items and chunks, and stop")
    a = ap.parse_args()
    items = bluetopo_items(a.items) if a.which == "bluetopo" else nonna10_items(a.items)
    chunks = [items[i:i + a.size] for i in range(0, len(items), a.size)]
    log(f"{len(items)} {'tiles' if a.which == 'bluetopo' else 'blocks'} in {len(chunks)} chunks")
    if a.count:
        log(estimate(a.work / "cache", a.which, a.size))
        return
    if not a.nas or ":" not in a.nas:
        raise SystemExit("--nas host:dir")
    sea = None
    if a.which == "nonna10":
        if not a.lakes:
            raise SystemExit("nonna10 needs --lakes (Natural Earth's SQLite): its datum surfaces extrapolate open sea, never into an inland lake")
        sea = OpenSea.natural_earth(a.lakes, every_body=True)  # NONNA charts the sea only: any body off the lakes is sea
    parts, cache = a.work / "parts", a.work / "cache"
    parts.mkdir(parents=True, exist_ok=True)
    for ci, chunk in enumerate(chunks):
        part = parts / f"part-{ci:03d}.geojson"
        if part.exists():
            continue
        out = a.work / f"out-{ci:03d}"
        log(f"chunk {ci + 1}/{len(chunks)}: {free(a.work)}")
        w = Writer(out)
        fetched = bluetopo_chunk(chunk, a.table, cache, w) if a.which == "bluetopo" else nonna10_chunk(chunk, a.table, cache, w, sea)
        w.pack()  # (The chunk's grids go to the NAS as packs: packs.py.)
        n, size = to_nas(out, a.nas) if out.exists() and any(out.iterdir()) else (0, 0)
        w.write_index(part)
        if out.exists():
            for p in out.iterdir():
                p.unlink()
            out.rmdir()
        for p in fetched:
            p.unlink(missing_ok=True)
        log(f"chunk {ci + 1}/{len(chunks)}: {n} files, {size / 1e9:.2f} GB on the NAS; {free(a.work)}")
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
