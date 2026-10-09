#!/usr/bin/env python3
"""Dominant leaf type of forests, on the canopy files' 10° squares (for the tree cover layer).

Classes (u8): 0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data. Written per 10° square,
at 0.0005° (~50 m): <dir>/lat<top>_lon<left>.tif.

Sources:
  Europe         Copernicus HRL Dominant Leaf Type 2018, 10 m (broadleaf / coniferous), read at
                 0.0005° from the EEA's public image service (exportImage, nearest neighbour).
  North America  NALCMS 2020 land cover, 30 m (CEC; NRCan, USGS, INEGI…): needleleaf forest →
                 conifer, broadleaf deciduous → broadleaf, mixed forest → mixed. The GeoTIFF is
                 streamed out of CEC's 3.9 GB zip by byte range, its size and CRC checked against
                 the zip's, and kept beside the squares (in <dir>'s parent: the NAS's
                 sources/trees/), so it's downloaded once.
  Hong Kong      none (no data).

usage: leaftype.py --make <dir> <top>,<left>…

The build agent's trees job has it make the squares its z3 tile needs (the trees program,
crates/pipeline/src/trees; dem/trees.py --z3 calls it too): whole squares, tagged complete (a square
not tagged so, or not whole, is made again, keeping its chunks fetched whole). An EEA square's
chunks are kept on the NAS as they come (`parts/`), so a square that fails part way asks again only
for what it lacks.
"""
from __future__ import annotations

import math
import os
import struct
import subprocess
import sys
import time
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.warp import Resampling, reproject

UA = "scenic-roads/0.1 (personal offline map)"
RES = 0.0005
N = int(round(10 / RES))

EEA = "https://image.discomap.eea.europa.eu/arcgis/rest/services/GioLandPublic/HRL_DominantLeafType2018/ImageServer/exportImage"
MAX_W, MAX_H = 7500, 4100
NALCMS_ZIP = "https://www.cec.org/files/atlas_layers/1_terrestrial_ecosystems/1_01_0_land_cover_2020_30m/land_cover_2020v2_30m_tif.zip"
NALCMS_MEMBER_OFFSET = 38952  # local header of …/data/NA_NALCMS_landcover_2020v2_30m.tif
NALCMS_COMPRESSED = 2668629085
# NALCMS classes → ours.
NA_MAP = np.zeros(256, np.uint8)
NA_MAP[[1, 2]] = 2
NA_MAP[[3, 4, 5]] = 1
NA_MAP[6] = 3
NA_MAP[[0, 127, 255]] = 255


# The sources' extents (w, s, e, n): a square outside both has no leaf type.
EEA_BOX = (-40, 20, 40, 75)
NALCMS_BOX = (-180, 14, -50, 84)


def complete(path: Path) -> bool:
    """Whether a square was made whole (tagged so; NALCMS's always are), not only over some regions,
    and is whole on the disk (whole.py)."""
    import whole

    if not whole.tiff_whole(path):
        return False
    try:
        with rasterio.open(path) as d:
            t = d.tags()
    except rasterio.errors.RasterioIOError:
        return False
    return t.get("complete") == "1" or t.get("source", "").startswith("NALCMS")


def save(top: int, left: int, a: np.ndarray, source: str, out: Path):
    """A square, tagged complete, written by a temporary name (this Mac's and the process's), flushed
    and checked whole before it takes its name."""
    import socket

    import whole as wh

    out.mkdir(parents=True, exist_ok=True)
    path = out / f"lat{top}_lon{left}.tif"
    tmp = out / f"lat{top}_lon{left}.{socket.gethostname()}.{os.getpid()}.tmp.tif"
    try:
        with rasterio.open(tmp, "w", driver="GTiff", width=N, height=N, count=1, dtype="uint8", crs="EPSG:4326",
                           transform=from_origin(left, top, RES, RES), nodata=255, compress="deflate", tiled=True,
                           blockxsize=512, blockysize=512) as d:
            d.write(a, 1)
            d.update_tags(source=source, classes="0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data", complete="1")
        wh.sync(tmp)
        if not wh.tiff_whole(tmp):
            raise OSError(f"{tmp}: written short")
        tmp.rename(path)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    counts = np.bincount(a.ravel(), minlength=256)
    print(f"  lat{top}_lon{left}: broadleaf {counts[1] / a.size:.1%}, conifer {counts[2] / a.size:.1%}, mixed {counts[3] / a.size:.1%}, no data {counts[255] / a.size:.1%}")


def eea_chunk(w, s, e, n, width, height) -> np.ndarray | None:
    """One exportImage request; a request that keeps failing is split in four, and any part that
    fails fails it all (None)."""
    a = eea_request(w, s, e, n, width, height)
    if a is not None or width < 1000 or height < 1000:
        return a
    hw, hh = width // 2, height // 2
    xm, ym = w + hw * RES, n - hh * RES
    out = np.full((height, width), 255, np.uint8)
    for (r0, c0, h, wd, bb) in [(0, 0, hh, hw, (w, ym, xm, n)), (0, hw, hh, width - hw, (xm, ym, e, n)),
                                (hh, 0, height - hh, hw, (w, s, xm, ym)), (hh, hw, height - hh, width - hw, (xm, s, e, ym))]:
        part = eea_chunk(*bb, wd, h)
        if part is None:
            return None
        out[r0:r0 + h, c0:c0 + wd] = part
    return out


def eea_request(w, s, e, n, width, height) -> np.ndarray | None:
    url = (f"{EEA}?bbox={w},{s},{e},{n}&bboxSR=4326&imageSR=4326&size={width},{height}&format=tiff&pixelType=U8"
           f"&interpolation=RSP_NearestNeighbor&f=image")
    for attempt in range(3):
        r = subprocess.run(["curl", "-sS", "--fail", "-m", "240", "-A", UA, url], capture_output=True)
        if r.returncode == 0 and r.stdout[:2] in (b"II", b"MM"):
            with rasterio.MemoryFile(r.stdout) as mf, mf.open() as d:
                a = d.read(1)
            if a.shape == (height, width):
                return a
        time.sleep(10 * (attempt + 1))
    return None


def europe_square(top: int, left: int, out: Path, progress=None) -> None:
    """One square from the EEA, whole: every chunk in the EEA's box, each first asked at a fifth of
    the resolution, so a chunk with no EEA data at all (Russia, the open sea) costs one small
    request; a chunk that keeps failing fails the square (to be tried again). `progress`, when given,
    is told how much of the square is done (0–1) as its chunks come."""
    a = np.full((N, N), 255, np.uint8)
    lut = np.full(256, 255, np.uint8)
    lut[[0, 1, 2]] = [0, 1, 2]
    jobs = []
    for r0 in range(0, N, MAX_H):
        for c0 in range(0, N, MAX_W):
            h, w = min(MAX_H, N - r0), min(MAX_W, N - c0)
            bb = (left + c0 * RES, top - (r0 + h) * RES, left + (c0 + w) * RES, top - r0 * RES)
            if bb[0] < EEA_BOX[2] and bb[2] > EEA_BOX[0] and bb[1] < EEA_BOX[3] and bb[3] > EEA_BOX[1]:
                jobs.append((r0, c0, h, w, bb))

    # The chunks, kept on the NAS as they come (a chunk: its array, or `.none` where the
    # EEA has no data), until the square is saved.
    parts = out / "parts" / f"lat{top}_lon{left}"
    # Today's square, made over some regions only (not tagged complete): a chunk of it fetched whole
    # then is kept rather than asked for again. Whole: no holes where the probe shows the EEA has data
    # (a request that failed then left one: asked again; a fifth of a chunk's land, or of its smallest
    # split, is far more than the coastline's 0.05 %). The probe comes from the EEA's coarser levels,
    # so its classes aren't compared, only where there's data.
    old = None
    path = out / f"lat{top}_lon{left}.tif"
    if path.exists():
        import whole

        if whole.tiff_whole(path):
            try:
                with rasterio.open(path) as d:
                    if (d.width, d.height) == (N, N):
                        old = d.read(1)
            except rasterio.errors.RasterioIOError:
                old = None
    kept_old = 0

    def fetch(j):
        r0, c0, h, w, bb = j
        kept = parts / f"{r0}-{c0}.npy"
        if (parts / f"{r0}-{c0}.none").exists():
            return np.full((h, w), 255, np.uint8)
        try:
            a = np.load(kept)
            if a.shape == (h, w) and a.dtype == np.uint8:
                return a
        except (OSError, ValueError, EOFError):
            pass
        # Asked first at a fifth of the resolution (~250 m): a chunk with no EEA data at all (Russia,
        # the open sea) costs one small request; land of a quarter kilometre shows.
        probe = eea_chunk(*bb, max(1, w // 5), max(1, h // 5))
        if probe is None:
            return None
        parts.mkdir(parents=True, exist_ok=True)
        if not np.isin(probe, (0, 1, 2)).any():
            (parts / f"{r0}-{c0}.none").write_bytes(b"")
            return np.full((h, w), 255, np.uint8)  # no EEA data here
        if old is not None:
            here = old[r0:r0 + h, c0:c0 + w]
            s = here[2::5, 2::5][:probe.shape[0], :probe.shape[1]]
            data = lut[probe] != 255
            if s.shape == probe.shape and np.count_nonzero(data & (s == 255)) <= 0.002 * np.count_nonzero(data):
                nonlocal kept_old
                kept_old += 1
                return here.copy()
        a = eea_chunk(*bb, w, h)
        if a is not None:
            import whole

            tmp = whole.tmp_name(kept)
            with tmp.open("wb") as f:
                np.save(f, a)
                f.flush()
                os.fsync(f.fileno())
            tmp.rename(kept)
        return a

    print(f"lat{top}_lon{left}: {len(jobs)} EEA chunks", flush=True)
    with ThreadPoolExecutor(2) as ex:
        for i, ((r0, c0, h, w, bb), chunk) in enumerate(zip(jobs, ex.map(fetch, jobs))):
            if chunk is None:
                raise RuntimeError(f"EEA leaf type: the request for {bb} keeps failing")
            a[r0:r0 + h, c0:c0 + w] = lut[chunk]
            # (The square's save counts as one chunk more.)
            if progress:
                progress((i + 1) / (len(jobs) + 1))
    if kept_old:
        print(f"  {kept_old} chunks kept from today's square", flush=True)
    save(top, left, a, "Copernicus HRL Dominant Leaf Type 2018 (EEA), 10 m, read at 0.0005°", out)
    import shutil

    shutil.rmtree(parts, ignore_errors=True)


def fetch_nalcms(tif: Path):
    """NALCMS's GeoTIFF, streamed out of CEC's zip, whole: the deflate stream to its end, and its
    size and CRC-32 those the zip records, before it takes its name."""
    import whole

    if tif.exists():
        return
    tif.parent.mkdir(parents=True, exist_ok=True)
    head = subprocess.run(["curl", "-sS", "--fail", "-A", UA, "-r", f"{NALCMS_MEMBER_OFFSET}-{NALCMS_MEMBER_OFFSET + 511}", NALCMS_ZIP],
                          capture_output=True, check=True).stdout
    if len(head) < 30 or head[:4] != b"PK\x03\x04":
        raise SystemExit("NALCMS: unexpected zip layout")
    crc, csize, usize, nlen, elen = struct.unpack("<IIIHH", head[14:30])
    if csize != NALCMS_COMPRESSED:
        raise SystemExit(f"NALCMS: the zip's member is {csize:,} bytes, not {NALCMS_COMPRESSED:,}")
    start = NALCMS_MEMBER_OFFSET + 30 + nlen + elen
    tmp = whole.tmp_name(tif)
    print(f"streaming NALCMS GeoTIFF ({NALCMS_COMPRESSED / 1e9:.1f} GB compressed)…")
    p = subprocess.Popen(["curl", "-sS", "--fail", "-A", UA, "-r", f"{start}-{start + NALCMS_COMPRESSED - 1}", NALCMS_ZIP], stdout=subprocess.PIPE)
    dec = zlib.decompressobj(-15)
    got = out_n = 0
    out_crc = 0
    try:
        with tmp.open("wb") as f:
            while chunk := p.stdout.read(8 << 20):
                b = dec.decompress(chunk)
                f.write(b)
                out_n += len(b)
                out_crc = zlib.crc32(b, out_crc)
                got += len(chunk)
                print(f"\r  {got / NALCMS_COMPRESSED:.0%}", end="", flush=True)
            b = dec.flush()
            f.write(b)
            out_n += len(b)
            out_crc = zlib.crc32(b, out_crc)
            f.flush()
            os.fsync(f.fileno())
        print()
        if p.wait() != 0:
            raise SystemExit("NALCMS download failed")
        if not dec.eof or out_n != usize or out_crc != crc:
            raise SystemExit(f"NALCMS: {out_n:,} bytes, CRC {out_crc:08x}; the zip says {usize:,}, {crc:08x}")
        tmp.rename(tif)
    except BaseException:
        p.kill()
        tmp.unlink(missing_ok=True)
        raise


def north_america_squares(todo: list, out: Path, tif: Path, progress=None) -> None:
    """Squares from NALCMS (its GeoTIFF streamed to `tif` first, and kept).
    `progress`, when given, is told how many are done after each."""
    if not todo:
        return
    fetch_nalcms(tif)
    with rasterio.open(tif) as src:
        for k, (top, left) in enumerate(todo):
            t0 = time.time()
            dst = np.full((N, N), 255, np.uint8)
            # NALCMS has no class 0: it is the background outside the continent (no data).
            reproject(rasterio.band(src, 1), dst, dst_transform=from_origin(left, top, RES, RES), dst_crs="EPSG:4326",
                      resampling=Resampling.nearest, src_nodata=0, dst_nodata=255, num_threads=4)
            save(top, left, NA_MAP[dst], "NALCMS 2020 land cover 30 m (CEC), resampled to 0.0005°", out)
            print(f"    ({time.time() - t0:.0f} s)")
            if progress:
                progress(k + 1)


def make(sqs: list, out: Path, store: Path, progress=None) -> None:
    """The leaf-type squares among `sqs` ((top, left)) that `out` lacks whole: inside the EEA's box
    from the EEA, inside NALCMS's from NALCMS (its GeoTIFF kept in `store`, downloaded once); none
    elsewhere (no source). `progress`, when given, is told how many of the squares to make are done,
    and of how many (an EEA square counted by its chunks as they come)."""
    def meets(box, top, left):
        return left < box[2] and left + 10 > box[0] and top - 10 < box[3] and top > box[1]

    missing = [(t, l) for t, l in sqs if not ((out / f"lat{t}_lon{l}.tif").exists() and complete(out / f"lat{t}_lon{l}.tif"))]
    eea = [(t, l) for t, l in missing if meets(EEA_BOX, t, l)]
    na = [(t, l) for t, l in missing if meets(NALCMS_BOX, t, l) and not meets(EEA_BOX, t, l)]

    def said(done: float) -> None:
        # (Nothing when there's none to make.)
        if progress and eea + na:
            progress(done, len(eea) + len(na))

    for k, (top, left) in enumerate(eea):
        said(k)
        europe_square(top, left, out, progress=lambda f, k=k: said(k + f))
    # (The EEA's squares made: all of them, when NALCMS has none to make.)
    said(len(eea))
    north_america_squares(na, out, store / "nalcms-2020.tif", progress=lambda d: said(len(eea) + d))


def progress(done: float, total: int, unit: str) -> None:
    """A line the build agent shows as the job's progress (crates/pipeline/src/agent/jobs.rs): the
    item under way counted by how much of it is done, to 3 decimals (down, as the agent's own)."""
    d = math.floor(min(done, total) * 1000) / 1000
    print(f"progress: {str(d).removesuffix('.0')}/{total} {unit}", file=sys.stderr, flush=True)


def main():
    # --make <dir> <top>,<left>…: the trees job's (crates/pipeline/src/trees/squares.rs).
    if sys.argv[1:2] != ["--make"] or len(sys.argv) < 3:
        raise SystemExit("usage: leaftype.py --make <dir> <top>,<left>…")
    out = Path(sys.argv[2])
    sqs = [tuple(int(v) for v in a.split(",")) for a in sys.argv[3:]]
    make(sqs, out, out.parent, lambda done, total: progress(done, total, "leaf-type squares"))


if __name__ == "__main__":
    main()
