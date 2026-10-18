"""The depth grids' shape (docs/inputs.md §6.10), shared by the depth producers.

A depth source is a set of GeoTIFFs listed by an index (GeoJSON) beside its declaration:

- each grid on the global Web Mercator pixel grid of one zoom (EPSG:3857; a pixel is
  40075016.686 / 256 / 2^zoom m and its edges fall on multiples of it from (−20037508.34, 20037508.34)),
  so the build reads it by pixel arithmetic alone;
- `file`: float32, depth below the water's surface in metres, negative below it (the sea's mean
  level, or a lake's mean surface), drying heights positive; NaN where there's none; tiled 256,
  Deflate with the floating-point predictor, overviews by averaging;
- `zoom_file` (optional): uint8, the zoom each cell's data supports (its own where measured, coarser
  where the source's flags say interpolated or predicted), 0 where there's none; overviews nearest;
  without it every cell supports the grid's zoom;
- `low_file` (optional): float32, how far the chart's low water (its datum: MLLW, LAT, chart datum)
  lies below the mean sea level, metres ≥ 0, for the "dries at low tide" flag;
- the index feature: its footprint (lon/lat), `url` (where the data came from), `file`,
  `zoom_file`, `low_file`, `zoom`, and `hash` (BLAKE3-free: SHA-256's first 16 hex digits of the
  depth file, so a changed grid changes the index and the keys reading it).

Files go in `sources/fetched/<id>/` on the NAS (the producer writes them to `--out`; the owner
copies them there with the index into `inputs/sources/`).
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import ssl
import subprocess
import sys
import time
import urllib.request
import urllib.robotparser
from pathlib import Path
from urllib.parse import urlsplit

import numpy as np

EARTH = 40075016.685578488
HALF = EARTH / 2
TILE = 256
UA = "scenic-roads/0.1 (personal offline map)"
os.environ.setdefault("GDAL_HTTP_USERAGENT", UA)
os.environ.setdefault("GDAL_CACHEMAX", "512")


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def px_size(z: int) -> float:
    """Web Mercator metres a pixel of zoom z."""
    return EARTH / (TILE * 2 ** z)


def native_zoom(cell_m: float, lat: float) -> int:
    """A source's own zoom: the coarsest whose pixels (ground metres at `lat`) are no larger than 1.5 ×
    its cells (the lab's rule)."""
    for z in range(0, 21):
        if px_size(z) * math.cos(math.radians(lat)) <= 1.5 * cell_m:
            return z
    return 20


def lonlat_to_merc(lon, lat):
    lon = np.asarray(lon, dtype="float64")
    lat = np.clip(np.asarray(lat, dtype="float64"), -85.05112878, 85.05112878)
    return lon / 180.0 * HALF, np.log(np.tan(np.pi / 4 + np.radians(lat) / 2)) * HALF / np.pi


def merc_to_lonlat(x, y):
    x = np.asarray(x, dtype="float64")
    y = np.asarray(y, dtype="float64")
    return x / HALF * 180.0, np.degrees(2 * np.arctan(np.exp(y / HALF * np.pi)) - np.pi / 2)


class Window:
    """A block of the zoom-z global pixel grid: columns c0..c0+w, rows r0..r0+h."""

    def __init__(self, z: int, c0: int, r0: int, w: int, h: int):
        self.z, self.c0, self.r0, self.w, self.h = z, c0, r0, w, h

    @classmethod
    def around(cls, z: int, bounds_merc, pad: int = 0) -> "Window":
        r = px_size(z)
        x0, y0, x1, y1 = bounds_merc
        c0 = max(0, int(math.floor((x0 + HALF) / r)) - pad)
        c1 = min(TILE * 2 ** z, int(math.ceil((x1 + HALF) / r)) + pad)
        r0 = max(0, int(math.floor((HALF - y1) / r)) - pad)
        r1 = min(TILE * 2 ** z, int(math.ceil((HALF - y0) / r)) + pad)
        return cls(z, c0, r0, max(0, c1 - c0), max(0, r1 - r0))

    def transform(self):
        from rasterio.transform import Affine

        r = px_size(self.z)
        return Affine(r, 0, -HALF + self.c0 * r, 0, -r, HALF - self.r0 * r)

    def bounds_merc(self):
        r = px_size(self.z)
        return (-HALF + self.c0 * r, HALF - (self.r0 + self.h) * r, -HALF + (self.c0 + self.w) * r, HALF - self.r0 * r)

    def bounds_lonlat(self):
        x0, y0, x1, y1 = self.bounds_merc()
        w, s = merc_to_lonlat(x0, y0)
        e, n = merc_to_lonlat(x1, y1)
        return [round(float(w), 7), round(float(s), 7), round(float(e), 7), round(float(n), 7)]

    def centres(self):
        """Lon/lat of every pixel's centre (h × w)."""
        r = px_size(self.z)
        xs = -HALF + (self.c0 + np.arange(self.w) + 0.5) * r
        ys = HALF - (self.r0 + np.arange(self.h) + 0.5) * r
        return merc_to_lonlat(*np.meshgrid(xs, ys))


def warp_onto(src, band: int, win: Window, resampling: str, src_nodata=None, dtype="float32") -> np.ndarray:
    """A band of `src` (a path, an open dataset, or (array, transform, crs)) onto the window; NaN
    outside its data."""
    import rasterio
    from rasterio.enums import Resampling
    from rasterio.warp import reproject

    out = np.full((win.h, win.w), np.nan, dtype=dtype)
    rs = getattr(Resampling, resampling)
    if isinstance(src, tuple):
        arr, tr, crs = src
        reproject(arr, out, src_transform=tr, src_crs=crs, src_nodata=src_nodata, dst_transform=win.transform(), dst_crs="EPSG:3857", dst_nodata=np.nan, resampling=rs)
        return out
    ds = rasterio.open(src) if isinstance(src, (str, Path)) else src
    try:
        nod = src_nodata if src_nodata is not None else ds.nodata
        reproject(rasterio.band(ds, band), out, src_nodata=nod, dst_transform=win.transform(), dst_crs="EPSG:3857", dst_nodata=np.nan, resampling=rs)
    finally:
        if ds is not src:
            ds.close()
    return out


def _write(path: Path, a: np.ndarray, win: Window, dtype: str, nodata, overview_resampling: str):
    import rasterio
    from rasterio.enums import Resampling

    tmp = path.with_suffix(".tmp.tif")
    prof = dict(driver="GTiff", height=win.h, width=win.w, count=1, dtype=dtype, crs="EPSG:3857", transform=win.transform(), nodata=nodata,
                tiled=True, blockxsize=256, blockysize=256, compress="deflate", predictor=3 if dtype == "float32" else 2, zlevel=6,
                BIGTIFF="IF_SAFER", SPARSE_OK=True)
    with rasterio.open(tmp, "w", **prof) as o:
        o.write(a.astype(dtype), 1)
        f, levels = 2, []
        while min(win.w, win.h) // f >= 64 and len(levels) < 8:
            levels.append(f)
            f *= 2
        if levels:
            o.build_overviews(levels, getattr(Resampling, overview_resampling))
    os.replace(tmp, path)


def file_hash(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()[:16]


class Writer:
    """Writes one source's grids into `out` and gathers its index."""

    def __init__(self, out: Path):
        self.out = Path(out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.features: list[dict] = []

    def put(self, name: str, url: str, win: Window, depth: np.ndarray, zoom: np.ndarray | None = None, low: np.ndarray | None = None, crop=True) -> bool:
        """One grid: `depth` (h × w on `win`), optional per-cell `zoom` and `low`. Cropped to its
        data; nothing written when it has none."""
        ok = np.isfinite(depth)
        if not ok.any():
            return False
        if crop:
            # (To its data, its edges on its zoom's tile edges where the window allows, so the
            # overviews' cells (up to 256 of its own) line up with the coarser zooms' pixels.)
            rows = np.flatnonzero(ok.any(1))
            cols = np.flatnonzero(ok.any(0))
            snap = lambda v, o: (v + o) // TILE * TILE - o
            r0, c0 = max(0, snap(rows[0], win.r0)), max(0, snap(cols[0], win.c0))
            r1 = min(win.h, snap(rows[-1] + TILE, win.r0))
            c1 = min(win.w, snap(cols[-1] + TILE, win.c0))
            sub = Window(win.z, win.c0 + c0, win.r0 + r0, c1 - c0, r1 - r0)
            depth = depth[r0:r1, c0:c1]
            zoom = None if zoom is None else zoom[r0:r1, c0:c1]
            low = None if low is None else low[r0:r1, c0:c1]
            win = sub
        props = {"url": url, "zoom": int(win.z), "file": f"{name}.tif"}
        _write(self.out / props["file"], np.where(np.isfinite(depth), depth, np.nan), win, "float32", float("nan"), "average")
        if zoom is not None:
            z = np.where(np.isfinite(depth), np.clip(zoom, 0, 255), 0).astype("uint8")
            props["zoom_file"] = f"{name}.zoom.tif"
            _write(self.out / props["zoom_file"], z, win, "uint8", 0, "nearest")
        if low is not None:
            props["low_file"] = f"{name}.low.tif"
            _write(self.out / props["low_file"], np.where(np.isfinite(depth), low, np.nan), win, "float32", float("nan"), "average")
        props["hash"] = file_hash(self.out / props["file"])
        w, s, e, n = win.bounds_lonlat()
        ring = [[w, s], [e, s], [e, n], [w, n], [w, s]]
        self.features = [f for f in self.features if f["properties"]["file"] != props["file"]]
        self.features.append({"type": "Feature", "properties": props, "geometry": {"type": "Polygon", "coordinates": [ring]}})
        return True

    def write_index(self, path: Path):
        """The index, its features in file order (so the same grids give the same bytes)."""
        feats = sorted(self.features, key=lambda f: f["properties"]["file"])
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            json.dump({"type": "FeatureCollection", "features": feats}, f, separators=(",", ":"))
            f.write("\n")
        log(f"{path}: {len(feats)} grids")


def tiles_of(win: Window, piece_z: int):
    """The window cut along the tiles of zoom `piece_z` (so no grid is huge): (x, y, sub-window)."""
    k = win.z - piece_z
    span = TILE * 2 ** k
    for ty in range(win.r0 // span, (win.r0 + win.h - 1) // span + 1):
        for tx in range(win.c0 // span, (win.c0 + win.w - 1) // span + 1):
            c0, r0 = max(win.c0, tx * span), max(win.r0, ty * span)
            c1, r1 = min(win.c0 + win.w, (tx + 1) * span), min(win.r0 + win.h, (ty + 1) * span)
            if c1 > c0 and r1 > r0:
                yield tx, ty, Window(win.z, c0, r0, c1 - c0, r1 - r0)


# ---- polite fetching ------------------------------------------------------------------------------

_robots: dict[str, urllib.robotparser.RobotFileParser] = {}
_tls: ssl.SSLContext | None = None


def urlopen(url: str, timeout: float):
    """A GET with the User-Agent. On macOS the system's root certificates are trusted too (read from
    its keychain): uv's Python carries OpenSSL's own store, which lacks some roots the hosts use (CHS's
    station service's chain ends at Sectigo Public Server Authentication Root R46). `SSL_CERT_FILE` set: that alone."""
    global _tls
    if _tls is None:
        _tls = ssl.create_default_context()
        if sys.platform == "darwin" and not os.environ.get("SSL_CERT_FILE"):
            try:
                pem = subprocess.run(["security", "find-certificate", "-a", "-p", "/System/Library/Keychains/SystemRootCertificates.keychain"],
                                     capture_output=True, text=True, check=True).stdout
                _tls.load_verify_locations(cadata=pem)
            except (OSError, subprocess.CalledProcessError, ssl.SSLError) as e:
                log(f"the system's root certificates not read ({e}): OpenSSL's alone")
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA}), timeout=timeout, context=_tls)


def robots_allow(url: str) -> bool:
    """Whether the host's robots.txt (read first, once, as a request of its own) lets us fetch `url`.
    Only a robots.txt that was read can disallow: one that's missing, refused (401, 403) or can't be
    read at all allows everything (the owner's rule)."""
    p = urlsplit(url)
    host = f"{p.scheme}://{p.netloc}"
    if host not in _robots:
        rp = urllib.robotparser.RobotFileParser()
        try:
            with urlopen(host + "/robots.txt", 60) as r:
                rp.parse(r.read().decode("utf-8", "replace").splitlines())
            how = "read"
        except (urllib.error.HTTPError, OSError) as e:
            rp.parse([])
            how = f"not readable ({e}): allows everything"
        _robots[host] = rp
        log(f"robots.txt of {host}: {how}")
    return _robots[host].can_fetch(UA, url)


def fetch(url: str, dest: Path, pause: float = 1.0) -> Path:
    """`url` into `dest` (kept if there), after its host's robots.txt; refuses a disallowed path."""
    dest = Path(dest)
    if dest.exists():
        return dest
    if not robots_allow(url):
        raise SystemExit(f"robots.txt disallows {url}: download it by hand (a 'You / waiting' task), then pass --from")
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    with urlopen(url, 300) as r, open(tmp, "wb") as f:
        while b := r.read(1 << 20):
            f.write(b)
    os.replace(tmp, dest)
    time.sleep(pause)
    return dest
