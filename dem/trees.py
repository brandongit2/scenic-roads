#!/usr/bin/env python3
"""Tree cover layer tiles: tree cover, canopy height and leaf type on the Web-Mercator grid.

Sources: Meta / WRI global canopy height (1.2 m imagery, aggregated to 0.00025° in the cached
10° files, data/cache/chm10: cover5m = share of ground under trees over 5 m, ‰; p95 = canopy
height, cm) and the leaf-type squares from leaftype.py (data/trees/leaf). Only ground inside
our regions (Geofabrik region outlines, data/trees/poly) is kept.

Writes three tile archives (build dir), zoom 4–12, 256 px Terrarium-encoded lossless WebP (about
half the size of PNG) so the app colours them on the GPU (MapLibre color-relief), like the slope
tint:
  trees-cover.tiles   tree cover, % in 2 % steps
  trees-height.tiles  canopy height where cover ≥ 5 %, m in 2 m steps
  trees-leaf.tiles    leaf type: 1 broadleaf, 2 conifer, 3 mixed (0 none; no tile = no data)
Zoom 12 samples the sources (nearest); coarser zooms average (leaf type: each class's share is
averaged, and a pixel shows the commonest leaf type where forest is at least half of it). Tiles with
nothing to show are left out.

Incremental: each zoom-8 block's inputs (the region outline inside it, the source files) are
fingerprinted in data/cache/trees/blocks.json and its zoom-8 values kept in data/cache/trees/tops;
a block whose fingerprint is unchanged has its zoom 8–12 tiles copied from the previous archives,
so adding a region only computes its own blocks (zoom 7–4 are rebuilt from the kept values).

A canopy square the last build used and no longer in the cache stops the run (data/cache/trees/
squares.json): its blocks would come out empty (a build on a machine holding only part of the
cache). --allow-missing builds anyway.

usage: trees.py <build_dir> [workers] [--bbox=w,s,e,n] [--vars=cover,height,leaf] [--allow-missing]
       trees.py --z3 x,y --coverage cov.json --chm dir --chm-store dir --leaf dir --out dir [--workers n]
           the build agent's (crates/pipeline/src/treepacks.rs): one z3 tile of the coverage, its
           canopy squares in `chm` (the units' cache, the same names), filled from the NAS's
           `chm-store`, where each is downloaded once, and its leaf-type squares made in `leaf`
           where it lacks them whole (leaftype.py); out/trees-*.tiles hold its zoom 4–12 tiles.
           cov.json: {"shapes": [[ring, …], …]}, each shape's rings in degrees, inside by even–odd
           (crates/pipeline/src/coverage.rs).
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import re
import struct
import sys
import time
import zlib
from multiprocessing import Pool
from pathlib import Path

import numpy as np
import rasterio
from rasterio.features import rasterize
from rasterio.transform import from_bounds
from rasterio.windows import Window
from PIL import Image
from shapely.geometry import box
from shapely.ops import transform as shp_transform
from shapely.prepared import prep

from leaftype import regions

ROOT = Path(__file__).resolve().parent.parent
CHM = ROOT / "data" / "cache" / "chm10"
CACHE = ROOT / "data" / "cache" / "trees"
VERSION = 2  # bump when the tile computation changes (invalidates the block cache)
LEAF = ROOT / "data" / "trees" / "leaf"
ZMAX, ZBLOCK, ZMIN = 12, 8, 4
TS = 256
BS = TS << (ZMAX - ZBLOCK)  # block size in z12 pixels (4096)
VARS = ("cover", "height", "leaf")
R = 6378137.0


def lon_of(px: np.ndarray, z: int) -> np.ndarray:
    return px / (TS << z) * 360.0 - 180.0


def lat_of(py: np.ndarray, z: int) -> np.ndarray:
    return np.degrees(np.arctan(np.sinh(np.pi * (1 - 2 * py / (TS << z)))))


def tile_bounds(z: int, x: int, y: int) -> tuple[float, float, float, float]:
    w, e = lon_of(np.array([x * TS, (x + 1) * TS]), z)
    n, s = lat_of(np.array([y * TS, (y + 1) * TS]), z)
    return float(w), float(s), float(e), float(n)


def merc(lon, lat):
    return R * np.radians(lon), R * np.log(np.tan(np.pi / 4 + np.radians(lat) / 2))


STEP = {"cover": 2.0, "height": 2.0, "leaf": 1.0}


def terrarium(v: np.ndarray, step: float) -> bytes:
    """256×256 values → Terrarium-encoded lossless WebP, in whole `step`s (values are small, ≥ 0)."""
    enc = np.clip(np.round(v / step) * step, 0, 30000).astype(np.int32) + 32768
    rgb = np.zeros((TS, TS, 3), np.uint8)
    rgb[..., 0] = enc >> 8
    rgb[..., 1] = enc & 255
    bio = io.BytesIO()
    Image.fromarray(rgb).save(bio, "WEBP", lossless=True, quality=100, method=4)
    return bio.getvalue()


def squares() -> list[tuple[int, int]]:
    out = []
    for f in sorted(CHM.glob("*_cover5m.tif")):
        m = re.search(r"lat=(-?[\d.]+)_lon=(-?[\d.]+)_", f.name)
        out.append((int(float(m[1])), int(float(m[2]))))
    return out


def sample(path: Path, top: int, left: int, res: float, lon: np.ndarray, lat: np.ndarray, out: np.ndarray, fill_ok) -> None:
    """Nearest-neighbour samples of one 10° square into `out` (rows = lat, cols = lon)."""
    cols = np.floor((lon - left) / res).astype(np.int64)
    rows = np.floor((top - lat) / res).astype(np.int64)
    n = int(round(10 / res))
    ci = np.nonzero((cols >= 0) & (cols < n))[0]
    ri = np.nonzero((rows >= 0) & (rows < n))[0]
    if not len(ci) or not len(ri):
        return
    c0, c1 = cols[ci].min(), cols[ci].max() + 1
    r0, r1 = rows[ri].min(), rows[ri].max() + 1
    with rasterio.open(path) as d:
        win = d.read(1, window=Window(int(c0), int(r0), int(c1 - c0), int(r1 - r0)))
    sub = win[np.ix_(rows[ri] - r0, cols[ci] - c0)]
    view = out[np.ix_(ri, ci)]
    keep = fill_ok(view)
    view[keep] = sub[keep]
    out[np.ix_(ri, ci)] = view


def block(args):
    """One zoom-8 block: sample at zoom 12 inside the regions, then average down to zoom 8."""
    bx, by, region_wkb, *rest = args
    want = rest[0] if rest else VARS
    from shapely import wkb

    region = wkb.loads(region_wkb)
    px = (np.arange(BS) + bx * BS + 0.5).astype(np.float64)
    py = (np.arange(BS) + by * BS + 0.5).astype(np.float64)
    lon, lat = lon_of(px, ZMAX), lat_of(py, ZMAX)
    w, s, e, n = tile_bounds(ZBLOCK, bx, by)
    cover = np.zeros((BS, BS), np.uint16)
    height = np.zeros((BS, BS), np.uint16)
    leaf = np.full((BS, BS), 255, np.uint8)
    for top, left in squares():
        if left >= e or left + 10 <= w or top <= s or top - 10 >= n:
            continue
        stem = CHM / f"meta_chm_lat={top}.0_lon={left}.0"
        sample(Path(f"{stem}_cover5m.tif"), top, left, 0.00025, lon, lat, cover, lambda v: v == 0)
        if "height" in want:
            sample(Path(f"{stem}_p95.tif"), top, left, 0.00025, lon, lat, height, lambda v: v == 0)
        lf = LEAF / f"lat{top}_lon{left}.tif"
        if lf.exists():
            sample(lf, top, left, 0.0005, lon, lat, leaf, lambda v: v == 255)
    # Inside our regions only (per pixel, in Mercator).
    inside = rasterize([(shp_transform(lambda x, y: merc(np.asarray(x), np.asarray(y)), region.intersection(box(w, s, e, n))), 1)],
                       out_shape=(BS, BS), transform=from_bounds(*merc(w, s), *merc(e, n), BS, BS), fill=0, dtype="uint8").astype(bool) \
        if not region.contains(box(w, s, e, n)) else np.ones((BS, BS), bool)
    out, tops = pyramid(bx, by, cover, height, leaf, inside, want)
    return (bx, by), out, tops


def pyramid(bx: int, by: int, cover: np.ndarray, height: np.ndarray, leaf: np.ndarray, inside: np.ndarray, want) -> tuple[list, dict]:
    """A zoom-8 block's tiles, zoom 12 to 8, from the canopy's cover (‰) and height (cm) and the leaf
    type sampled at zoom 12, inside `inside`; and its zoom-8 values, for the zooms above."""
    cov = np.where(inside & (cover <= 1000), cover / 10.0, 0).astype(np.float32)
    hgt = np.where(inside & (cov >= 5), height / 100.0, 0).astype(np.float32)
    lft = np.where(inside, leaf, 255).astype(np.uint8)
    lft = np.where((lft >= 1) & (lft <= 3) & (cov < 5), 0, lft)  # leaf type only where there are trees

    # Leaf type as shares of not forest / broadleaf / conifer / mixed / no data, averaged down the
    # pyramid (a cascaded majority vote inflates forest), classified at each zoom.
    shares = np.stack([(lft == c) for c in (0, 1, 2, 3, 255)]).astype(np.float32) if "leaf" in want else None
    out = []
    tops = {}
    for z in range(ZMAX, ZBLOCK - 1, -1):
        if shares is not None:
            lft = leaf_class(shares)
        k = 1 << (z - ZBLOCK)
        size = TS * k
        for ty in range(k):
            for tx in range(k):
                sl = (slice(ty * TS, (ty + 1) * TS), slice(tx * TS, (tx + 1) * TS))
                x, y = bx * k + tx, by * k + ty
                for name, a in (("cover", cov[sl]), ("height", hgt[sl])):
                    if name in want and a.max() >= STEP[name] / 2:
                        img = terrarium(a, STEP[name])
                        out.append((name, z, x, y, img, len(img)))
                lt = lft[sl]
                if "leaf" in want and ((lt >= 1) & (lt <= 3)).any():
                    img = terrarium(np.where(lt == 255, 0, lt).astype(np.float32), 1.0)
                    out.append(("leaf", z, x, y, img, len(img)))
        if z == ZBLOCK:
            tops = {"cover": cov, "height": hgt, "leaf": shares}
            break
        cov, hgt = down(cov), down(hgt)
        if shares is not None:
            shares = np.stack([down(a) for a in shares])
        assert cov.shape[0] == size // 2
    return out, tops


def down(a: np.ndarray) -> np.ndarray:
    return a.reshape(a.shape[0] // 2, 2, a.shape[1] // 2, 2).mean(axis=(1, 3), dtype=np.float32)


def leaf_class(shares: np.ndarray) -> np.ndarray:
    """Class from shares (not forest, broadleaf, conifer, mixed, no data): the commonest leaf type
    where forest is at least half the known ground, else not forest; no data where nothing is known."""
    forest = shares[1:4].sum(axis=0)
    known = forest + shares[0]
    kind = shares[1:4].argmax(axis=0).astype(np.uint8) + 1
    return np.where(known <= 0, 255, np.where(forest >= 0.5 * known, kind, 0)).astype(np.uint8)


def down_leaf(a: np.ndarray) -> np.ndarray:
    """Majority of each 2×2 among not forest (0) and broadleaf / conifer / mixed, so forest doesn't
    spread as tiles get coarser (ties go to forest); no data (255) only where all four are."""
    q = a.reshape(a.shape[0] // 2, 2, a.shape[1] // 2, 2).transpose(0, 2, 1, 3).reshape(a.shape[0] // 2, a.shape[1] // 2, 4)
    counts = np.stack([(q == c).sum(axis=2) for c in (0, 1, 2, 3)], axis=2)
    leaf = counts[..., 1:].argmax(axis=2) + 1
    forest = counts[..., 1:].max(axis=2)
    out = np.where(forest >= np.maximum(counts[..., 0], 1), leaf, np.where(counts[..., 0] > 0, 0, 255))
    return out.astype(np.uint8)


class Reader:
    """The previous archive (roadcore::archive format), for copying tiles of unchanged blocks."""

    def __init__(self, path: Path):
        import mmap
        self.f = path.open("rb")
        self.m = mmap.mmap(self.f.fileno(), 0, access=mmap.ACCESS_READ)
        off, n = struct.unpack_from("<QQ", self.m, 8)
        self.index = {}
        for i in range(n):
            k, o, ln, raw = struct.unpack_from("<QQII", self.m, off + i * 24)
            self.index[k] = (o, ln, raw)

    def get(self, z: int, x: int, y: int):
        e = self.index.get((z << 58) | (x << 29) | y)
        return (self.m[e[0]:e[0] + e[1]], e[2]) if e else None


def block_sig(bx: int, by: int, region, want) -> str:
    """Fingerprint of a block's inputs: the region outline inside it and the source files."""
    w, s, e, n = tile_bounds(ZBLOCK, bx, by)
    h = hashlib.sha1(f"{VERSION}|{bx},{by}|{','.join(want)}".encode())
    part = region.intersection(box(w, s, e, n))
    h.update(part.wkb if not part.is_empty else b"")
    for top, left in squares():
        if left >= e or left + 10 <= w or top <= s or top - 10 >= n:
            continue
        for f in (CHM / f"meta_chm_lat={top}.0_lon={left}.0_cover5m.tif", CHM / f"meta_chm_lat={top}.0_lon={left}.0_p95.tif",
                  LEAF / f"lat{top}_lon{left}.tif"):
            if f.exists():
                st = f.stat()
                h.update(f"{f.name}:{st.st_size}:{int(st.st_mtime)}".encode())
    return h.hexdigest()


class Writer:
    """roadcore::archive format: magic, index offset, count, metadata JSON, blobs (here WebP, not
    gzip'd), index."""

    def __init__(self, path: Path, meta: str):
        self.path, self.tmp = path, path.with_suffix(".tiles.tmp")
        self.f = self.tmp.open("wb")
        mb = meta.encode()
        self.f.write(b"RDTILES1" + bytes(16) + struct.pack("<I", len(mb)) + mb)
        self.pos = 28 + len(mb)
        self.index = []

    def add(self, z: int, x: int, y: int, blob: bytes, raw: int):
        self.f.write(blob)
        self.index.append(((z << 58) | (x << 29) | y, self.pos, len(blob), raw))
        self.pos += len(blob)

    def finish(self):
        self.index.sort()
        pad = (8 - self.pos % 8) % 8
        self.f.write(bytes(pad))
        off = self.pos + pad
        for e in self.index:
            self.f.write(struct.pack("<QQII", *e))
        self.f.seek(8)
        self.f.write(struct.pack("<QQ", off, len(self.index)))
        self.f.close()
        self.tmp.rename(self.path)
        return len(self.index)


def main():
    if "--z3" in sys.argv:
        a = sys.argv[1:]
        z3_main({a[i]: a[i + 1] for i in range(0, len(a) - 1, 2) if a[i].startswith("--")})
        return
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    build = Path(args[0])
    workers = int(args[1]) if len(args) > 1 else 6
    # --bbox=w,s,e,n: only blocks there (for trying styles out; the archives then hold just that).
    only = next((box(*map(float, a.split("=", 1)[1].split(","))) for a in sys.argv[1:] if a.startswith("--bbox=")), None)
    # --vars=leaf,…: rebuild only these archives (the others are left as they are).
    want = next((tuple(a.split("=", 1)[1].split(",")) for a in sys.argv[1:] if a.startswith("--vars=")), VARS)
    t0 = time.time()
    region = regions()
    rp = prep(region)
    # Zoom-8 blocks touching the regions.
    blocks = []
    for bx in range(1 << ZBLOCK):
        for by in range(1 << ZBLOCK):
            b = tile_bounds(ZBLOCK, bx, by)
            if rp.intersects(box(*b)) and (only is None or only.intersects(box(*b))):
                blocks.append((bx, by))
    print(f"{len(blocks)} zoom-{ZBLOCK} blocks")
    # The canopy squares touching the regions: any the last build had and the cache hasn't now?
    have = sorted(sq for sq in squares() if rp.intersects(box(sq[1], sq[0] - 10, sq[1] + 10, sq[0])))
    used_path = CACHE / "squares.json"
    gone = sorted({tuple(x) for x in json.loads(used_path.read_text())} - set(have)) if used_path.exists() else []
    if gone and "--allow-missing" not in sys.argv:
        sys.exit(f"canopy squares used by the last build are missing from {CHM} (top, left): {gone}; copy them in, or --allow-missing")
    meta = '{"source":"Meta/WRI canopy height; Copernicus HRL DLT 2018; NALCMS 2020","encoding":"terrarium","format":"webp"}'
    # Blocks unchanged since the last run: tiles copied from the previous archives, zoom-8 values
    # from the cache (not for --bbox trial runs).
    use_cache = only is None and tuple(want) == VARS
    (CACHE / "tops").mkdir(parents=True, exist_ok=True)
    sig_path = CACHE / "blocks.json"
    old_sigs = json.loads(sig_path.read_text()) if use_cache and sig_path.exists() else {}
    readers = {}
    for v in want:
        p = build / f"trees-{v}.tiles"
        if use_cache and p.exists():
            readers[v] = Reader(p)
    sigs = {f"{bx},{by}": block_sig(bx, by, region, want) for bx, by in blocks} if use_cache else {}
    reuse, todo = [], []
    for bx, by in blocks:
        k = f"{bx},{by}"
        top_file = CACHE / "tops" / f"{bx}_{by}.npz"
        (reuse if use_cache and len(readers) == len(want) and old_sigs.get(k) == sigs[k] and top_file.exists() else todo).append((bx, by))
    print(f"{len(reuse)} blocks unchanged (copied), {len(todo)} to compute")
    writers = {v: Writer(build / f"trees-{v}.tiles", meta) for v in want}
    tops: dict[str, dict[tuple[int, int], np.ndarray]] = {v: {} for v in want}
    for bx, by in reuse:
        with np.load(CACHE / "tops" / f"{bx}_{by}.npz") as f:
            for v in want:
                tops[v][(bx, by)] = f[v].astype(np.float32)
        for z in range(ZBLOCK, ZMAX + 1):
            k = 1 << (z - ZBLOCK)
            for y in range(by * k, (by + 1) * k):
                for x in range(bx * k, (bx + 1) * k):
                    for v in want:
                        t = readers[v].get(z, x, y)
                        if t:
                            writers[v].add(z, x, y, bytes(t[0]), t[1])
    rw = region.wkb
    with Pool(workers) as pool:
        for i, ((bx, by), out, t) in enumerate(pool.imap_unordered(block, [(bx, by, rw, want) for bx, by in todo])):
            for name, z, x, y, blob, raw in out:
                writers[name].add(z, x, y, blob, raw)
            for v in want:
                tops[v][(bx, by)] = t[v]
            if use_cache:
                np.savez_compressed(CACHE / "tops" / f"{bx}_{by}.npz", **{v: t[v].astype(np.float16) for v in want})
            if i % 10 == 0:
                print(f"\r  {i + 1}/{len(todo)} blocks ({time.time() - t0:.0f} s)", end="", flush=True)
    print()
    if use_cache:
        sig_path.write_text(json.dumps(sigs))
        used_path.write_text(json.dumps(have))
    lower_zooms(tops, want, writers)
    for v, wtr in writers.items():
        n = wtr.finish()
        print(f"trees-{v}.tiles: {n} tiles, {(build / f'trees-{v}.tiles').stat().st_size / 1e9:.2f} GB")
    print(f"done in {time.time() - t0:.0f} s")


def lower_zooms(tops: dict, want, writers: dict) -> None:
    """Zoom 7 → 4 from the zoom-8 blocks' values, into `writers`."""
    level = tops
    for z in range(ZBLOCK - 1, ZMIN - 1, -1):
        nxt = {v: {} for v in want}
        parents = {(bx >> 1, by >> 1) for (bx, by) in level[want[0]]}
        for px, py in parents:
            for v in want:
                if v == "leaf":
                    big = np.zeros((5, TS * 2, TS * 2), np.float32)
                    big[4] = 1  # no data where no child block
                else:
                    big = np.zeros((TS * 2, TS * 2), np.float32)
                for dx in (0, 1):
                    for dy in (0, 1):
                        a = level[v].get((px * 2 + dx, py * 2 + dy))
                        if a is not None:
                            big[..., dy * TS:(dy + 1) * TS, dx * TS:(dx + 1) * TS] = a
                a = np.stack([down(x) for x in big]) if v == "leaf" else down(big)
                nxt[v][(px, py)] = a
                if v == "leaf":
                    cls = leaf_class(a)
                    if ((cls >= 1) & (cls <= 3)).any():
                        img = terrarium(np.where(cls == 255, 0, cls).astype(np.float32), 1.0)
                        writers[v].add(z, px, py, img, len(img))
                elif a.max() >= STEP[v] / 2:
                    img = terrarium(a, STEP[v])
                    writers[v].add(z, px, py, img, len(img))
        level = nxt


# ---- the build agent's: one z3 tile of the coverage ----------------------------------------

CHM10_URL = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg"
UA = "road-elevations/0.1 (personal offline map)"


def download(url: str, path: Path) -> None:
    """`url` into `path` (by a temporary name, flushed), whole: a body shorter than its
    Content-Length (a connection cut), or not a whole TIFF, is tried again. An empty file when the
    server has none (404, or S3's 403 for a key that isn't there), so it says twice, a moment apart
    (it's remembered for good), as scenic-metrics marks it. Anything else is retried, then fails."""
    import os
    import shutil
    import urllib.error
    import urllib.request

    import whole

    tmp = whole.tmp_name(path)
    last: Exception | None = None
    missing = 0
    for attempt in range(6):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA}), timeout=600) as r, tmp.open("wb") as f:
                want = int(r.headers.get("Content-Length", "-1"))
                shutil.copyfileobj(r, f, 16 << 20)
                f.flush()
                os.fsync(f.fileno())
            got = tmp.stat().st_size
            if want >= 0 and got != want:
                raise OSError(f"{got:,} of {want:,} bytes")
            if not whole.tiff_whole(tmp):
                raise OSError("not a whole TIFF")
            tmp.rename(path)
            return
        except urllib.error.HTTPError as e:
            if e.code in (403, 404):
                missing += 1
                if missing == 2:
                    path.write_bytes(b"")
                    return
            last = e
        except OSError as e:
            last = e
        tmp.unlink(missing_ok=True)
        time.sleep(5 if missing else 2 ** attempt)
    raise RuntimeError(f"download failed: {url}: {last}")


def canopy_square(chm: Path, store: Path, top: int, left: int) -> bool:
    """The canopy square's cover and height files in `chm`: copied from the NAS's `store`, or
    downloaded into it first (once); False when Meta has none there. A copy that isn't whole (cut
    short) is deleted and taken again from the next source (crates/pipeline/src/whole.rs)."""
    import os

    import whole

    def kept_whole(f: Path) -> bool:
        if not f.exists():
            return False
        if f.stat().st_size == 0 or whole.tiff_whole(f):
            return True
        print(f"canopy: {f} isn't whole: taken again", file=sys.stderr)
        f.unlink(missing_ok=True)
        return False

    def fetch_once(kept: Path) -> None:
        """The NAS's copy of `kept`; else the right to download it there (`<file>.lock`, made
        exclusively: a unit on the other Mac may want the same square at once), or the copy the
        holder downloads, waited for. A lock not touched for 30 minutes is a holder that died."""
        import socket

        store.mkdir(parents=True, exist_ok=True)
        lock = kept.with_name(kept.name + ".lock")
        while not kept_whole(kept):
            try:
                fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            except FileExistsError:
                try:
                    if time.time() - lock.stat().st_mtime > 1800:
                        print(f"canopy: taking over {lock}", file=sys.stderr)
                        lock.unlink(missing_ok=True)
                        continue
                except FileNotFoundError:
                    continue
                time.sleep(20)
                continue
            os.write(fd, f"{socket.gethostname()} {os.getpid()}".encode())
            os.close(fd)
            try:
                download(f"{CHM10_URL}/{kept.name}", kept)
            finally:
                lock.unlink(missing_ok=True)

    there = True
    for st in ("cover5m", "p95"):
        p = chm / f"meta_chm_lat={top}.0_lon={left}.0_{st}.tif"
        if not kept_whole(p):
            kept = store / p.name
            fetch_once(kept)
            whole.copy(kept, p)
        if p.stat().st_size == 0:
            there = False
        else:
            os.utime(p)  # used now: the agent's room-making deletes the least recently used first
    return there


def shape_mask(shapes: list, w: float, s: float, e: float, n: float) -> np.ndarray:
    """Pixels of a zoom-8 block inside the coverage: inside an odd number of a shape's rings (its
    rings in Mercator metres), for any shape."""
    from rasterio.features import MergeAlg
    from shapely.geometry import Polygon

    tr = from_bounds(*merc(w, s), *merc(e, n), BS, BS)
    inside = np.zeros((BS, BS), bool)
    for rings in shapes:
        if rings:
            hits = rasterize([(Polygon(r), 1) for r in rings], out_shape=(BS, BS), transform=tr, fill=0, dtype="uint16", merge_alg=MergeAlg.add)
            inside |= hits % 2 == 1
    return inside


_SHAPES: list = []


def load_shapes(path: str) -> list:
    """The coverage's shapes (cov.json): per shape, each ring's box (degrees) and the ring in
    Mercator metres. Each worker loads them once (rings can run to millions of points)."""
    global _SHAPES
    _SHAPES = []
    for rings in json.loads(Path(path).read_text())["shapes"]:
        rs = [densify(np.asarray(r, np.float64)) for r in rings if len(r) >= 3]
        _SHAPES.append([(float(r[:, 0].min()), float(r[:, 1].min()), float(r[:, 0].max()), float(r[:, 1].max()), np.stack(merc(r[:, 0], r[:, 1]), axis=1)) for r in rs])
    return _SHAPES


def densify(r: np.ndarray, most: float = 0.05) -> np.ndarray:
    """A ring (degrees) with points added so no edge is longer than `most` degrees: the coverage's
    edges are straight in longitude and latitude, and rasterizing in Mercator draws them straight
    there; short edges make the difference a few metres."""
    out = [r[:1]]
    for a, b in zip(r[:-1], r[1:]):
        k = max(1, int(np.ceil(np.abs(b - a).max() / most)))
        t = (np.arange(1, k + 1) / k)[:, None]
        out.append(a + t * (b - a))
    return np.concatenate(out)


def shapes_meeting(w: float, s: float, e: float, n: float) -> list:
    """Each shape's rings (Mercator metres) whose box meets w, s, e, n (a ring that doesn't can't
    change which points there are inside)."""
    return [[r[4] for r in rings if r[0] <= e and r[2] >= w and r[1] <= n and r[3] >= s] for rings in _SHAPES]


def z3_block(args):
    """One zoom-8 block of the agent's z3 tile: sampled at zoom 12 inside the coverage's shapes."""
    bx, by, chm, leaf_dir, sqs = args
    px = (np.arange(BS) + bx * BS + 0.5).astype(np.float64)
    py = (np.arange(BS) + by * BS + 0.5).astype(np.float64)
    lon, lat = lon_of(px, ZMAX), lat_of(py, ZMAX)
    w, s, e, n = tile_bounds(ZBLOCK, bx, by)
    cover = np.zeros((BS, BS), np.uint16)
    height = np.zeros((BS, BS), np.uint16)
    leaf = np.full((BS, BS), 255, np.uint8)
    for top, left in sqs:
        if left >= e or left + 10 <= w or top <= s or top - 10 >= n:
            continue
        stem = Path(chm) / f"meta_chm_lat={top}.0_lon={left}.0"
        sample(Path(f"{stem}_cover5m.tif"), top, left, 0.00025, lon, lat, cover, lambda v: v == 0)
        sample(Path(f"{stem}_p95.tif"), top, left, 0.00025, lon, lat, height, lambda v: v == 0)
        lf = Path(leaf_dir) / f"lat{top}_lon{left}.tif"
        if lf.exists():
            sample(lf, top, left, 0.0005, lon, lat, leaf, lambda v: v == 255)
    out, tops = pyramid(bx, by, cover, height, leaf, shape_mask(shapes_meeting(w, s, e, n), w, s, e, n), VARS)
    return (bx, by), out, tops


def z3_main(args: dict) -> None:
    import leaftype

    t0 = time.time()
    qx, qy = (int(v) for v in args["--z3"].split(","))
    chm, store, leaf_dir, out = Path(args["--chm"]), Path(args["--chm-store"]), Path(args["--leaf"]), Path(args["--out"])
    for d in (chm, leaf_dir, out):
        d.mkdir(parents=True, exist_ok=True)
    workers = int(args.get("--workers", "6"))
    load_shapes(args["--coverage"])

    def meets(w, s, e, n):
        return any(shapes_meeting(w, s, e, n))

    # The zoom-8 blocks of the z3 tile that the coverage meets.
    k = 1 << (ZBLOCK - 3)
    blocks = [(bx, by) for bx in range(qx * k, (qx + 1) * k) for by in range(qy * k, (qy + 1) * k) if meets(*tile_bounds(ZBLOCK, bx, by))]
    # The canopy squares they touch, fetched when missing, and their leaf types.
    want = set()
    for bx, by in blocks:
        w, s, e, n = tile_bounds(ZBLOCK, bx, by)
        for top in range(math.ceil(n / 10) * 10, math.floor(s / 10) * 10, -10):
            for left in range(math.floor(w / 10) * 10, math.ceil(e / 10) * 10, 10):
                if top > s and top - 10 < n and left < e and left + 10 > w:
                    want.add((top, left))
    sqs = []
    for i, (top, left) in enumerate(sorted(want)):
        print(f"progress: {i}/{len(want)} canopy squares", file=sys.stderr, flush=True)
        if canopy_square(chm, store, top, left):
            sqs.append((top, left))
    leaftype.make(sqs, leaf_dir, leaf_dir.parent)
    print(f"trees z3 {qx},{qy}: {len(blocks)} zoom-8 blocks, {len(sqs)} canopy squares ({time.time() - t0:.0f} s)", file=sys.stderr, flush=True)
    meta = '{"source":"Meta/WRI canopy height; Copernicus HRL DLT 2018; NALCMS 2020","encoding":"terrarium","format":"webp"}'
    writers = {v: Writer(out / f"trees-{v}.tiles", meta) for v in VARS}
    tops: dict[str, dict] = {v: {} for v in VARS}
    with Pool(workers, initializer=load_shapes, initargs=(args["--coverage"],)) as pool:
        for i, ((bx, by), tiles, t) in enumerate(pool.imap_unordered(z3_block, [(bx, by, str(chm), str(leaf_dir), sqs) for bx, by in blocks])):
            for name, z, x, y, blob, raw in tiles:
                writers[name].add(z, x, y, blob, raw)
            for v in VARS:
                tops[v][(bx, by)] = t[v]
            print(f"progress: {i + 1}/{len(blocks)} zoom-8 blocks", file=sys.stderr, flush=True)
    lower_zooms(tops, VARS, writers)
    for v, wtr in writers.items():
        print(f"trees-{v}.tiles: {wtr.finish()} tiles", file=sys.stderr)
    print(f"trees z3 {qx},{qy}: done in {time.time() - t0:.0f} s", file=sys.stderr)


if __name__ == "__main__":
    main()
