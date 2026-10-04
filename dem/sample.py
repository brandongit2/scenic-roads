#!/usr/bin/env python3
"""Sample national DEMs at every road vertex.

Priority per vertex (first source with valid data wins):
  North America (west of 40° W)
  1. NRCan HRDEM 2 m lidar mosaic, read at its 8 m overview  (Canada, where lidar exists)
  2. USGS 3DEP 1/3 arc-second (~10 m)                         (United States)
  3. NRCan MRDEM 30 m                                         (Canada + border fallback)
  Japan (GSI's own order, per pixel)
  5. GSI lidar DEMs (Geospatial Information Authority of Japan elevation tiles, read at z15, ~4 m
     pixels): 1A (1 m, averaged by GSI to z15), then 5A (5 m); then 5B / 5C photogrammetry
  6. GSI 10 m DEM (10B, z14: dem_png)
  Taiwan
  7. MOI 20 m DTM (Ministry of the Interior; Open Government Data License): GeoTIFFs put by hand in
     the NAS's inputs/moi-dtm/ (tgos.tw answers 403 outside Taiwan), which the unit step passes as
     $SCENIC_MOI_DTM (else data/sources/moi-dtm); FABDEM without them, and cached FABDEM values
     in Taiwan are sampled again once they're there
  Elsewhere (Europe, Hong Kong, Singapore), and points none of the above cover
  4. FABDEM v1-2 30 m: Copernicus DEM with forests and buildings removed (University of
     Bristol; CC BY-NC-SA 4.0, personal use). Its 1° tiles are read in place inside the
     official 10° zips (stored uncompressed, so GDAL range-reads them through /vsizip).

Only the COG blocks (and GSI tiles) that contain road vertices are fetched (HTTP range
requests); nothing is stored except the per-vertex results, and Taiwan's DTM file.

Incremental: after a run, a sorted (vertex → elevation, source) cache is kept. Densified
geometry is deterministic, so unchanged roads reproduce identical vertices and are served
from the cache; only new or edited roads trigger DEM downloads.

Outputs are written to *.tmp memory-maps (checkpointed per DEM file, so an interrupted run
resumes) and renamed into place at the end, so a running server is never disturbed.

usage: sample.py <build_dir> [--workers N] [--cache DIR] [--no-cache]
"""
from __future__ import annotations

import argparse
import io
import json
import math
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

os.environ.update(
    GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
    CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif,.zip",
    GDAL_HTTP_MAX_RETRY="10",
    GDAL_HTTP_RETRY_DELAY="2",
    GDAL_HTTP_TIMEOUT="90",
    VSI_CACHE="FALSE",
    GDAL_CACHEMAX="256",
)

import numpy as np  # noqa: E402
import rasterio  # noqa: E402
from pyproj import Transformer  # noqa: E402
from rasterio.windows import Window  # noqa: E402
from tqdm import tqdm  # noqa: E402

HERE = Path(__file__).resolve().parent
NRCAN = "https://canelevation-dem.s3.ca-central-1.amazonaws.com"
MRDEM = f"/vsicurl/{NRCAN}/mrdem-30/mrdem-30-dtm.tif"
USGS = "/vsicurl/https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/TIFF/current/{t}/USGS_13_{t}.tif"
FABDEM = "/vsizip//vsicurl/https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn/{z}_FABDEM_V1-2.zip/{t}_FABDEM_V1-2.tif"

GSI = "cyberjapandata.gsi.go.jp"
UA = "road-elevations/0.1 (personal offline map)"
GSI_WORKERS = 16  # concurrent tile requests to GSI (S3 behind CloudFront; latency-bound)
# Taiwan's MOI 20 m DTM, put by hand (see above). Changing its files changes Taiwan's units' keys
# (crates/pipeline/src/rules.rs, "moi-dtm").
MOI_DTM = sorted(Path(os.environ.get("SCENIC_MOI_DTM") or HERE.parent / "data" / "sources" / "moi-dtm").glob("*.tif"))

SRC_HRDEM, SRC_3DEP, SRC_MRDEM, SRC_FABDEM, SRC_GSI5A, SRC_GSI5, SRC_GSI10, SRC_MOI = 1, 2, 3, 4, 5, 6, 7, 8
# Changing which DEM serves where, or how: bump the rule's version in crates/pipeline/src/rules.rs
# ("dem-north-america", "dem-japan", "dem-taiwan", "dem-fabdem"), so its units rerun and their kept
# samples from it are sampled again.
NA_WEST_OF = -40.0  # North America: the national DEMs above; elsewhere FABDEM


def in_japan(lon, lat):
    return (lon > 122.5) & (lon < 154.0) & (lat > 20.0) & (lat < 46.5)


def in_taiwan(lon, lat):
    return (lon > 118.0) & (lon <= 122.5) & (lat > 21.5) & (lat < 26.6)
NODATA_BELOW = -1000.0  # all three sources use large negative nodata values

_tls = threading.local()
# A file's handles' generation: bumped when the file is replaced (a damaged copy taken again), so
# no thread reads on through a handle to the old one.
_GEN: dict[str, int] = {}


def open_ds(url: str, level: int | None):
    """Thread-local dataset handles (GDAL handles are not thread-safe)."""
    cache = getattr(_tls, "ds", None)
    if cache is None:
        cache = _tls.ds = {}
    key = (url, level, _GEN.get(url, 0))
    ds = cache.get(key)
    if ds is None:
        ds = rasterio.open(url, overview_level=level) if level is not None else rasterio.open(url)
        cache[key] = ds
    return ds


def zip_names(url: str) -> set[str]:
    """The member names of the zip at `url`, from its central directory (read by byte range: the
    end records, then the directory). Raises when it can't be read whole."""
    import http.client
    import struct
    import urllib.error
    import urllib.request

    def get(rng: str) -> bytes:
        last: Exception | None = None
        for attempt in range(4):
            try:
                req = urllib.request.Request(url, headers={"User-Agent": UA, "Range": f"bytes={rng}"})
                with urllib.request.urlopen(req, timeout=60) as r:
                    b = r.read()
                    want = r.headers.get("Content-Length")
                    if r.status != 206 or (want is not None and int(want) != len(b)):
                        raise OSError(f"range {rng}: status {r.status}, {len(b):,} bytes")
                    return b
            except (OSError, http.client.HTTPException) as e:
                last = e
            time.sleep(10 * (attempt + 1))
        raise RuntimeError(f"{url}: its file list can't be read ({last}); the unit is tried again later")

    tail = get("-65558")
    i = tail.rfind(b"PK\x05\x06")
    if i < 0 or len(tail) < i + 22:
        raise RuntimeError(f"{url}: no end of central directory")
    n, cd_size, cd_off = struct.unpack("<HII", tail[i + 10:i + 20])
    if 0xFFFF in (n,) or 0xFFFFFFFF in (cd_size, cd_off):
        j = tail.rfind(b"PK\x06\x07", 0, i)
        if j < 0:
            raise RuntimeError(f"{url}: no zip64 end locator")
        (rec_off,) = struct.unpack("<Q", tail[j + 8:j + 16])
        rec = get(f"{rec_off}-{rec_off + 55}")
        if rec[:4] != b"PK\x06\x06":
            raise RuntimeError(f"{url}: no zip64 end record")
        n, cd_size, cd_off = struct.unpack("<QQQ", rec[32:56])
    cd = get(f"{cd_off}-{cd_off + cd_size - 1}")
    names, p = set(), 0
    for _ in range(n):
        if cd[p:p + 4] != b"PK\x01\x02":
            raise RuntimeError(f"{url}: a damaged central directory")
        nlen, xlen, clen = struct.unpack("<HHH", cd[p + 28:p + 34])
        names.add(cd[p + 46:p + 46 + nlen].decode("utf-8", "replace"))
        p += 46 + nlen + xlen + clen
    return names


def absent(url: str) -> bool:
    """Whether a DEM tile that wouldn't open isn't there at all: the server answers 404 or 403 (S3's
    answer for a key that doesn't exist), or for a tile inside a zip, the zip's own file list (its
    central directory) lacks it. A timeout, a 5xx, a zip whose list can't be read, or a file that's
    there but wouldn't open raises instead: the unit's job then fails and is tried again later,
    rather than keeping a coarser source's value (or FABDEM's `.none`) for good."""
    import http.client
    import urllib.error
    import urllib.request

    plain = url.replace("/vsizip/", "").replace("/vsicurl/", "")
    target = plain[: plain.index(".zip/") + 4] if ".zip/" in plain else plain
    last: Exception | None = None
    for attempt in range(4):
        try:
            req = urllib.request.Request(target, method="HEAD", headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60):
                pass
            if target == plain:
                # A plain file that answers is there: the open failed in passing.
                return False
            return plain[len(target) + 1:] not in zip_names(target)
        except urllib.error.HTTPError as e:
            if e.code in (403, 404, 410):
                return True
            last = e
        except (OSError, http.client.HTTPException) as e:
            last = e
        time.sleep(10 * (attempt + 1))
    raise RuntimeError(f"{target}: no answer ({last}); the unit is tried again later")


def bilinear(a: np.ndarray, c: np.ndarray, r: np.ndarray) -> np.ndarray:
    """Bilinear sample of block `a` at pixel-centre coords (c, r); edge pixels clamp.
    Falls back to the nearest pixel if any of the four neighbours is nodata."""
    h, w = a.shape
    c0 = np.floor(c).astype(np.int32)
    r0 = np.floor(r).astype(np.int32)
    fx = (c - c0).astype(np.float32)
    fy = (r - r0).astype(np.float32)
    c0c, c1 = np.clip(c0, 0, w - 1), np.clip(c0 + 1, 0, w - 1)
    r0c, r1 = np.clip(r0, 0, h - 1), np.clip(r0 + 1, 0, h - 1)
    v00, v01, v10, v11 = a[r0c, c0c], a[r0c, c1], a[r1, c0c], a[r1, c1]
    val = (v00 * (1 - fx) + v01 * fx) * (1 - fy) + (v10 * (1 - fx) + v11 * fx) * fy
    bad = (v00 < NODATA_BELOW) | (v01 < NODATA_BELOW) | (v10 < NODATA_BELOW) | (v11 < NODATA_BELOW)
    bad |= ~np.isfinite(val)
    if bad.any():
        rn = np.clip(np.rint(r[bad]).astype(np.int32), 0, h - 1)
        cn = np.clip(np.rint(c[bad]).astype(np.int32), 0, w - 1)
        vn = a[rn, cn]
        vn = np.where((vn < NODATA_BELOW) | ~np.isfinite(vn), np.nan, vn)
        val[bad] = vn
    return val


def sample_raster(url, level, idx, px, py, elev, src, code, pool, desc, block=512):
    """Sample one raster at points `idx` (projected coords px, py in the raster's CRS).
    Writes valid samples into elev/src. Returns number of valid samples."""
    ds = open_ds(url, level)
    t = ds.transform
    W, H = ds.width, ds.height
    cf = (px - t.c) / t.a - 0.5
    rf = (py - t.f) / t.e - 0.5
    inside = (cf > -0.5) & (rf > -0.5) & (cf < W - 0.5) & (rf < H - 0.5)
    idx, cf, rf = idx[inside], cf[inside], rf[inside]
    if idx.size == 0:
        return 0
    bx = np.clip(np.floor(cf).astype(np.int64), 0, W - 1) // block
    by = np.clip(np.floor(rf).astype(np.int64), 0, H - 1) // block
    key = by * ((W + block - 1) // block) + bx
    order = np.argsort(key, kind="stable")
    key, idx, cf, rf = key[order], idx[order], cf[order], rf[order]
    starts = np.flatnonzero(np.r_[True, key[1:] != key[:-1]])
    ends = np.r_[starts[1:], key.size]
    nbx = (W + block - 1) // block

    def work(s, e):
        k = int(key[s])
        bxi, byi = k % nbx, k // nbx
        x0, y0 = bxi * block, byi * block
        w, h = min(block, W - x0), min(block, H - y0)
        ds = open_ds(url, level)
        a = ds.read(1, window=Window(x0, y0, w, h), out_dtype="float32")
        if ds.nodata is not None and ds.nodata >= NODATA_BELOW:
            a[a == np.float32(ds.nodata)] = -1e9
        return s, e, bilinear(a, cf[s:e] - x0, rf[s:e] - y0)

    good = 0
    futs = [pool.submit(work, s, e) for s, e in zip(starts, ends)]
    for f in tqdm(as_completed(futs), total=len(futs), desc=desc, unit="blk", leave=False, mininterval=0.5):
        s, e, v = f.result()
        ok = np.isfinite(v)
        ii = idx[s:e][ok]
        elev[ii] = v[ok]
        src[ii] = code
        good += int(ok.sum())
    return good


def fabdem_stored(store: Path, zname: str, tname: str, again: bool = False) -> str | None:
    """FABDEM tile `tname` from the NAS's store ($SCENIC_FABDEM_STORE), copied there from Bristol's
    10° zip the first time (a compressed GeoTIFF, read back and compared before it takes the name),
    so each tile is downloaded once; None for a tile the zip doesn't have (open sea), remembered as
    `<tile>.none`. A stored copy that isn't whole, or (`again`) won't read, is taken again."""
    import socket

    import whole

    f = store / f"{tname}_FABDEM_V1-2.tif"
    if f.exists():
        if not again and whole.tiff_whole(f):
            return str(f)
        print(f"FABDEM {tname}: the stored copy {'won’t read' if again else 'isn’t whole'}: taken again", flush=True)
        f.unlink()
    elif (store / f"{tname}.none").exists():
        return None
    store.mkdir(parents=True, exist_ok=True)
    url = FABDEM.format(z=zname, t=tname)
    tmp = store / f"{tname}.{socket.gethostname()}.{os.getpid()}.tmp.tif"
    try:
        with rasterio.open(url) as src:
            profile = src.profile | {"driver": "GTiff", "compress": "deflate", "predictor": 3, "tiled": True, "blockxsize": 512, "blockysize": 512}
            data = src.read()
        with rasterio.open(tmp, "w", **profile) as dst:
            dst.write(data)
        whole.sync(tmp)
        with rasterio.open(tmp) as back:
            same = np.array_equal(back.read(), data, equal_nan=True)
        if not same:
            raise OSError(f"FABDEM {tname}: the copy written to the store reads back different")
    except rasterio.errors.RasterioIOError:
        tmp.unlink(missing_ok=True)
        if not absent(url):
            raise
        (store / f"{tname}.none").write_bytes(b"")
        return None
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    tmp.rename(f)
    return str(f)


def fabdem_name(lat0: int, lon0: int) -> str:
    """FABDEM tile / zip corner name, e.g. N43E007 (latitude and longitude of the SW corner)."""
    return f"{'N' if lat0 >= 0 else 'S'}{abs(lat0):02d}{'E' if lon0 >= 0 else 'W'}{abs(lon0):03d}"


def fabdem_groups(lon, lat, idx):
    """Group point indices by FABDEM 1° tile: {(tile, zip): indices}. Tiles are named by their
    SW corner; the 10° zips by their SW and NE corners."""
    if idx.size == 0:
        return {}
    la, lo = np.floor(lat[idx]).astype(np.int64), np.floor(lon[idx]).astype(np.int64)
    tkey = (la + 90) * 1000 + (lo + 180)
    order = np.argsort(tkey, kind="stable")
    tk = tkey[order]
    starts = np.flatnonzero(np.r_[True, tk[1:] != tk[:-1]])
    groups = {}
    for s, e in zip(starts, np.r_[starts[1:], tk.size]):
        k = int(tk[s])
        lat0, lon0 = k // 1000 - 90, k % 1000 - 180
        la10, lo10 = lat0 // 10 * 10, lon0 // 10 * 10
        z = f"{fabdem_name(la10, lo10)}-{fabdem_name(la10 + 10, lo10 + 10)}"
        groups[(fabdem_name(lat0, lon0), z)] = idx[order[s:e]]
    return groups


def usgs_groups(lon, lat, idx):
    """Group point indices by USGS 1° tile name (nXXwYYY = upper-left corner)."""
    if idx.size == 0:
        return {}
    tkey = np.ceil(lat[idx]).astype(np.int64) * 1000 + np.ceil(-lon[idx]).astype(np.int64)
    order = np.argsort(tkey, kind="stable")
    tk = tkey[order]
    starts = np.flatnonzero(np.r_[True, tk[1:] != tk[:-1]])
    groups = {}
    for s, e in zip(starts, np.r_[starts[1:], tk.size]):
        k = int(tk[s])
        groups[f"n{k // 1000:02d}w{k % 1000:03d}"] = idx[order[s:e]]
    return groups


def gsi_tile(layer: str, z: int, x: int, y: int) -> np.ndarray | None:
    """One GSI elevation tile (256 × 256, metres; NaN where there's no data), or None if there's
    no tile. PNG tiles: x = R·2¹⁶ + G·2⁸ + B, h = 0.01·x below 2²³, 0.01·(x − 2²⁴) above, and
    2²³ is no data. One kept-alive connection per thread."""
    import http.client

    for attempt in range(8):
        conn = getattr(_tls, "gsi", None)
        if conn is None:
            conn = _tls.gsi = http.client.HTTPSConnection(GSI, timeout=60)
        try:
            conn.request("GET", f"/xyz/{layer}/{z}/{x}/{y}.png", headers={"User-Agent": UA})
            r = conn.getresponse()
            body = r.read()
        except (OSError, http.client.HTTPException):
            conn.close()
            _tls.gsi = None
            time.sleep(2 * (attempt + 1))
            continue
        if r.status == 404:
            return None
        if r.status != 200:
            time.sleep(2 * (attempt + 1))
            continue
        from PIL import Image

        a = np.asarray(Image.open(io.BytesIO(body)).convert("RGB"), dtype=np.int64)
        v = (a[..., 0] << 16) | (a[..., 1] << 8) | a[..., 2]
        h = np.where(v < 1 << 23, v, v - (1 << 24)).astype(np.float32) * np.float32(0.01)
        h[v == 1 << 23] = np.nan
        return h
    raise RuntimeError(f"GSI {layer}/{z}/{x}/{y}: no answer")


def gsi_pass(layer, z, code, lon, lat, idx, elev, src, pool, desc):
    """Sample the GSI layer at points `idx`; fills points it has data for. Values sit at pixel
    centres; a point near a tile edge clamps to it (under a pixel off)."""
    if idx.size == 0:
        return 0
    n = 2**z
    fx = (lon[idx] + 180.0) / 360.0 * n
    fy = (1.0 - np.log(np.tan(np.radians(lat[idx])) + 1.0 / np.cos(np.radians(lat[idx]))) / math.pi) / 2.0 * n
    tx, ty = np.floor(fx).astype(np.int64), np.floor(fy).astype(np.int64)
    key = tx * n + ty
    order = np.argsort(key, kind="stable")
    key = key[order]
    starts = np.flatnonzero(np.r_[True, key[1:] != key[:-1]])
    ends = np.r_[starts[1:], key.size]

    def work(s, e):
        sel = order[s:e]
        k = int(key[s])
        a = gsi_tile(layer, z, k // n, k % n)
        if a is None:
            return sel, None
        c = (fx[sel] - k // n) * 256 - 0.5
        r = (fy[sel] - k % n) * 256 - 0.5
        return sel, bilinear(np.where(np.isnan(a), np.float32(-1e9), a), c, r)

    good = 0
    for f in tqdm(as_completed([pool.submit(work, s, e) for s, e in zip(starts, ends)]), total=starts.size, desc=desc, unit="tile", mininterval=1):
        sel, v = f.result()
        if v is None:
            continue
        ok = np.isfinite(v)
        elev[idx[sel[ok]]] = v[ok]
        src[idx[sel[ok]]] = code
        good += int(ok.sum())
    return good


def pack(v: np.ndarray) -> np.ndarray:
    """Vertex (lon_e7, lat_e7) → sortable uint64 key."""
    return ((v[:, 0].astype(np.int64) + 2**31).astype(np.uint64) << np.uint64(32)) | (v[:, 1].astype(np.int64) + 2**31).astype(np.uint64)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build", type=Path)
    ap.add_argument("--workers", type=int, default=48)
    ap.add_argument("--cache", type=Path, default=None)
    ap.add_argument("--no-cache", action="store_true", help="ignore cached elevations (e.g. after a DEM update)")
    args = ap.parse_args()
    b: Path = args.build
    cache_dir: Path = args.cache or (b.parent / "cache")
    t_start = time.time()

    verts = np.memmap(b / "verts.bin", dtype=np.int32, mode="r").reshape(-1, 2)
    n = verts.shape[0]
    stamp = f"{n}:{(b / 'verts.bin').stat().st_mtime_ns}"
    print(f"{n:,} vertices")

    ckpt_path = b / "dem-progress.json"
    ckpt = json.loads(ckpt_path.read_text()) if ckpt_path.exists() else {}
    resume = ckpt.get("stamp") == stamp and (b / "elev.f32.tmp").exists() and (b / "src.u8.tmp").exists()
    done = set(ckpt.get("done", [])) if resume else set()
    elev = np.memmap(b / "elev.f32.tmp", dtype=np.float32, mode="r+" if resume else "w+", shape=(n,))
    src = np.memmap(b / "src.u8.tmp", dtype=np.uint8, mode="r+" if resume else "w+", shape=(n,))

    def mark(name):
        elev.flush()
        src.flush()
        done.add(name)
        ckpt_path.write_text(json.dumps({"stamp": stamp, "done": sorted(done)}))

    if not resume:
        elev[:] = np.nan
        src[:] = 0
        keys = pack(verts)
        ck = cache_dir / "dem-cache.keys.u64"
        if not args.no_cache and ck.exists() and ck.stat().st_size > 0:
            ckeys = np.memmap(ck, dtype=np.uint64, mode="r")
            celev = np.memmap(cache_dir / "dem-cache.elev.f32", dtype=np.float32, mode="r")
            csrc = np.memmap(cache_dir / "dem-cache.src.u8", dtype=np.uint8, mode="r")
            hit_total = 0
            for i in tqdm(range(0, n, 10_000_000), desc="reuse cached elevations", unit="chunk"):
                k = keys[i : i + 10_000_000]
                pos = np.searchsorted(ckeys, k)
                pos_c = np.minimum(pos, ckeys.size - 1)
                hit = (pos < ckeys.size) & (ckeys[pos_c] == k)
                elev[i : i + 10_000_000][hit] = celev[pos_c[hit]]
                src[i : i + 10_000_000][hit] = csrc[pos_c[hit]]
                hit_total += int(hit.sum())
            print(f"cache: reused {hit_total:,} of {n:,} vertices ({hit_total / max(n, 1) * 100:.1f} %)")
            if MOI_DTM:
                # Taiwan's roads sampled from FABDEM before the MOI DTM was here: sample them again.
                redo = 0
                for i in range(0, n, 10_000_000):
                    v = verts[i : i + 10_000_000]
                    bad = (src[i : i + 10_000_000] == SRC_FABDEM) & in_taiwan(v[:, 0] * 1e-7, v[:, 1] * 1e-7)
                    elev[i : i + 10_000_000][bad] = np.nan
                    src[i : i + 10_000_000][bad] = 0
                    redo += int(bad.sum())
                if redo:
                    print(f"cache: {redo:,} Taiwanese vertices sampled from FABDEM, now from the MOI DTM")
        del keys
        mark("cache")

    miss = np.flatnonzero(np.isnan(elev))
    print(f"{miss.size:,} vertices need DEM sampling")
    lon = verts[miss, 0].astype(np.float64) * 1e-7
    lat = verts[miss, 1].astype(np.float64) * 1e-7
    # Local arrays are indexed by position in `miss`; results are scattered back via `miss`.
    loc_elev = np.full(miss.size, np.nan, np.float32)
    loc_src = np.zeros(miss.size, np.uint8)

    def scatter():
        ok = np.isfinite(loc_elev)
        elev[miss[ok]] = loc_elev[ok]
        src[miss[ok]] = loc_src[ok]

    # North American vertices use the national DEMs, the rest FABDEM.
    na = lon < NA_WEST_OF
    local = np.flatnonzero(na)
    elsewhere = np.flatnonzero(~na)
    print(f"  {local.size:,} in North America, {elsewhere.size:,} elsewhere")

    # Project to EPSG:3979 (Canada Atlas Lambert, used by HRDEM and MRDEM); NaN outside North America.
    tr = Transformer.from_crs("EPSG:4326", "EPSG:3979", always_xy=True)
    x = np.full(miss.size, np.nan, np.float64)
    y = np.full(miss.size, np.nan, np.float64)
    step = 5_000_000
    for i in tqdm(range(0, local.size, step), desc="project → EPSG:3979", unit="chunk"):
        sel = local[i : i + step]
        x[sel], y[sel] = tr.transform(lon[sel], lat[sel])

    pool = ThreadPoolExecutor(max_workers=args.workers)

    # ---- 1. HRDEM lidar (8 m overview of the 2 m mosaic) ----------------------------
    index = json.loads((HERE / "hrdem_tile_index.geojson").read_text())
    have = {line.split("/")[-1].split("-")[0] for line in (HERE / "hrdem_2m_tiles.txt").read_text().split()}
    tiles = []
    for f in index["features"]:
        tid = f["properties"]["id"]
        if tid not in have:
            continue
        xs = [p[0] for p in f["geometry"]["coordinates"][0]]
        ys = [p[1] for p in f["geometry"]["coordinates"][0]]
        sel = local[(x[local] >= min(xs)) & (x[local] < max(xs)) & (y[local] > min(ys)) & (y[local] <= max(ys))]
        if sel.size:
            tiles.append((tid, sel))
    tiles.sort(key=lambda t: -t[1].size)
    print(f"HRDEM: {len(tiles)} mosaic tiles contain roads to sample")
    for tid, sel in tqdm(tiles, desc="HRDEM lidar tiles", unit="tile"):
        name = f"hrdem:{tid}"
        if name in done:
            continue
        url = f"/vsicurl/{NRCAN}/hrdem-mosaic-2m/{tid}-mosaic-2m-dtm.tif"
        got = sample_raster(url, 1, sel, x[sel], y[sel], loc_elev, loc_src, SRC_HRDEM, pool, f"  {tid} ({sel.size:,} pts)")
        tqdm.write(f"  HRDEM {tid}: {got:,}/{sel.size:,} vertices with lidar")
        scatter()
        mark(name)

    # ---- 2. USGS 3DEP 1/3" -----------------------------------------------------------
    left = local[np.isnan(loc_elev[local])]
    groups = usgs_groups(lon, lat, left)
    print(f"3DEP: {left.size:,} vertices left, {len(groups)} candidate 1° tiles")
    for tname, sel in tqdm(sorted(groups.items(), key=lambda kv: -kv[1].size), desc="USGS 3DEP tiles", unit="tile"):
        name = f"3dep:{tname}"
        if name in done:
            continue
        try:
            got = sample_raster(USGS.format(t=tname), None, sel, lon[sel], lat[sel], loc_elev, loc_src, SRC_3DEP, pool, f"  {tname} ({sel.size:,} pts)")
        except rasterio.errors.RasterioIOError:
            if not absent(USGS.format(t=tname)):
                raise
            got = 0  # no 3DEP tile here (Canada / ocean)
        if got:
            tqdm.write(f"  3DEP {tname}: {got:,}/{sel.size:,}")
        scatter()
        mark(name)

    # ---- 3. MRDEM 30 m fallback ---------------------------------------------------------
    if "mrdem" not in done:
        left = local[np.isnan(loc_elev[local])]
        print(f"MRDEM: {left.size:,} vertices left")
        if left.size:
            got = sample_raster(MRDEM, None, left, x[left], y[left], loc_elev, loc_src, SRC_MRDEM, pool, "MRDEM 30 m blocks")
            print(f"  MRDEM: {got:,}/{left.size:,}")
        scatter()
        mark("mrdem")

    # ---- 5. Japan: GSI 5 m (5A lidar, then 5B / 5C photogrammetry), then 10 m ------------------
    jp = elsewhere[in_japan(lon[elsewhere], lat[elsewhere])]
    if jp.size:
        gpool = ThreadPoolExecutor(max_workers=GSI_WORKERS)
        for layer, z, code in (("dem1a_png", 15, SRC_GSI5A), ("dem5a_png", 15, SRC_GSI5A), ("dem5b_png", 15, SRC_GSI5), ("dem5c_png", 15, SRC_GSI5), ("dem_png", 14, SRC_GSI10)):
            name = f"gsi:{layer}"
            if name in done:
                continue
            left = jp[np.isnan(loc_elev[jp])]
            got = gsi_pass(layer, z, code, lon, lat, left, loc_elev, loc_src, gpool, f"GSI {layer} ({left.size:,} pts)")
            print(f"  GSI {layer}: {got:,}/{left.size:,}")
            scatter()
            mark(name)
        gpool.shutdown()

    # ---- 6. Taiwan: MOI 20 m DTM (a local file per island group) --------------------------------
    tw = elsewhere[in_taiwan(lon[elsewhere], lat[elsewhere])]
    for path in MOI_DTM if tw.size else []:
        name = f"moi:{path.name}"
        if name in done:
            continue
        left = tw[np.isnan(loc_elev[tw])]
        with rasterio.open(path) as ds:
            crs = ds.crs
        px, py = Transformer.from_crs("EPSG:4326", crs, always_xy=True).transform(lon[left], lat[left])
        got = sample_raster(str(path), None, left, np.asarray(px), np.asarray(py), loc_elev, loc_src, SRC_MOI, pool, f"MOI {path.name}")
        print(f"  MOI {path.name}: {got:,}/{left.size:,}")
        scatter()
        mark(name)

    # ---- 4. FABDEM 30 m (the rest outside North America, and North American points none of the
    # national DEMs cover, e.g. Saint-Pierre-et-Miquelon) -----------------------------------------
    uncovered = local[np.isnan(loc_elev[local])]
    if uncovered.size:
        print(f"FABDEM fallback: {uncovered.size:,} North American vertices without a national DEM")
    elsewhere = elsewhere[np.isnan(loc_elev[elsewhere])]
    groups = fabdem_groups(lon, lat, np.concatenate([elsewhere, uncovered]))
    print(f"FABDEM: {elsewhere.size:,} vertices, {len(groups)} 1° tiles")
    store = os.environ.get("SCENIC_FABDEM_STORE")
    for (tname, zname), sel in tqdm(sorted(groups.items(), key=lambda kv: -kv[1].size), desc="FABDEM tiles", unit="tile"):
        name = f"fabdem:{tname}"
        if name in done:
            continue
        # From the NAS's store, where each tile is downloaded once (else read in place at Bristol).
        path = fabdem_stored(Path(store), zname, tname) if store else FABDEM.format(z=zname, t=tname)
        if path is None:
            got = 0  # no tile: open sea
        else:
            try:
                got = sample_raster(path, None, sel, lon[sel], lat[sel], loc_elev, loc_src, SRC_FABDEM, pool, f"  {tname} ({sel.size:,} pts)")
            except rasterio.errors.RasterioIOError:
                if not store:
                    if not absent(path):
                        raise
                    got = 0  # no tile: open sea
                else:
                    # The stored copy is damaged: taken again (once), its old handles dropped.
                    path = fabdem_stored(Path(store), zname, tname, again=True)
                    _GEN[path] = _GEN.get(path, 0) + 1
                    got = sample_raster(path, None, sel, lon[sel], lat[sel], loc_elev, loc_src, SRC_FABDEM, pool, f"  {tname} ({sel.size:,} pts)") if path else 0
        tqdm.write(f"  FABDEM {tname}: {got:,}/{sel.size:,}")
        scatter()
        mark(name)
    pool.shutdown()

    # ---- finish: stats, atomic rename, refresh cache ---------------------------------
    elev.flush()
    src.flush()
    counts = np.bincount(src, minlength=9)
    stats = {
        "vertices": int(n),
        "hrdem": int(counts[1]),
        "usgs3dep": int(counts[2]),
        "mrdem": int(counts[3]),
        "fabdem": int(counts[4]),
        "gsi5a": int(counts[5]),
        "gsi5": int(counts[6]),
        "gsi10": int(counts[7]),
        "moi": int(counts[8]),
        "missing": int(counts[0]),
        "sampled_this_run": int(miss.size),
        "seconds": round(time.time() - t_start, 1),
    }
    del elev, src
    os.replace(b / "elev.f32.tmp", b / "elev.f32")
    os.replace(b / "src.u8.tmp", b / "src.u8")
    (b / "dem-stats.json").write_text(json.dumps(stats, indent=2))
    ckpt_path.unlink(missing_ok=True)

    print("updating elevation cache")
    cache_dir.mkdir(parents=True, exist_ok=True)
    keys = pack(verts)
    order = np.argsort(keys, kind="stable")
    ev = np.memmap(b / "elev.f32", dtype=np.float32, mode="r")
    sv = np.memmap(b / "src.u8", dtype=np.uint8, mode="r")
    for name, arr in [("keys.u64", keys[order]), ("elev.f32", ev[order]), ("src.u8", sv[order])]:
        tmp = cache_dir / f"dem-cache.{name}.tmp"
        arr.tofile(tmp)
        os.replace(tmp, cache_dir / f"dem-cache.{name}")
    print(json.dumps(stats, indent=2))


if __name__ == "__main__":
    main()
