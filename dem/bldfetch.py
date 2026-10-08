#!/usr/bin/env python3
"""The 3D buildings' sources onto the NAS (docs/buildings3d.md §2), each file whole, as the source
has it, each fetched once:

  - Overture Maps' building and building-part files of one release (theme=buildings, type=building
    and type=building_part, GeoParquet on Overture's public S3 bucket, read anonymously over HTTPS)
    that hold a building within --margin-km of the coverage: a file is taken when one of its row
    groups' boxes (its footer's bbox statistics) meets the coverage grown by the margin. Into
      sources/overture/<release>/theme=buildings/type=<type>/<file>   (the release's dot a dash,
                                                                       as sources/buildings/ has it)
    with sources/overture/<release>/buildings.json: the files here, each with its size, S3 ETag,
    rows and row groups' boxes, and the coverage they were chosen for; and footers.json.gz, every
    file of the release's row-group boxes (read once: a release never changes).
  - GHSL's building-height tiles (GHS-BUILT-H R2023A, ANBH: the average height of the buildings in
    each 3 arcsec cell, epoch 2018, WGS84; EC JRC, CC BY 4.0) meeting the coverage grown by the
    margin, as JRC's 10° zips. Into sources/ghsl/R2023A/<file>.zip, with index.json.

The coverage is the regions' outlines as the map's server has them (--server: /api/regions, and
/api/areas/<id> for each osm: entry; place: entries are drawn here; poly: and geofabrik: entries are
read from the NAS's inputs/outlines/), or a GeoJSON file (--coverage). Adding a region and running
this again fetches only the files the new coverage needs.

Each file is downloaded to `<file>.<host>.tmp` beside its place, resumed from where a run cut short
left it (an HTTP range; for S3, only while the object is the one it was: If-Match its ETag), flushed,
checked, then renamed into place: an Overture file's length and its ETag, the MD5 of its 64 MiB
parts as S3 computes a multipart ETag (hashed as it comes in; what a resumed file had is re-read
from the NAS); a GHSL zip's length and every member's CRC-32 (zipfile's test, which reads it back).
A file already in place at its listed size is skipped: only checked files are put there. A file
that fails its check is fetched again once from the start, then fails the run.

Gentle with the NAS and the build: at most --jobs transfers at once (2-4), and at most --max-mb-s
MB/s in all (0: no cap). The defaults, 2 and 3 MB/s, leave the build some of the home line, which
gave ~2-4 MB/s from S3 on 2026-10-05. A request that gets no answer (no connection, a timeout, a
429 or a 5xx, an answer cut short) is tried again with a growing wait (Retry-After honoured); a
refusal (403, 404) fails that file. When the NAS goes away, the run waits for it (up to --nas-wait-h hours), then goes
on from the temporary files. One run at a time per Mac (a lock in $TMPDIR). The User-Agent says who
asks; TLS certificates are always verified; nothing needs an account or a key.

Progress goes to stdout, a line a file and a summary each minute ("progress: d/t files"); run it with
its output appended to the NAS's state/logs/bldfetch.log.

usage: bldfetch.py --root <NAS project folder> [--release 2026-09-23.1] [--server URL | --coverage f]
                   [--margin-km 20] [--jobs 2] [--max-mb-s 3] [--only overture|ghsl] [--limit n]
                   [--nas-wait-h 6] [--dry-run]
       --limit n: only the n smallest files of each source (a test)
"""
from __future__ import annotations

import argparse
import fcntl
import gzip
import hashlib
import http.client
import io
import json
import math
import os
import re
import signal
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

UA = "scenic-roads/0.1 (personal offline map)"
S3 = "https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/"
TYPES = ("building_part", "building")
GHSL_RELEASE = "R2023A"
GHSL_PRODUCT = "GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_4326_3ss_V1_0"
GHSL_URL = ("https://jeodpp.jrc.ec.europa.eu/ftp/jrc-opendata/GHSL/GHS_BUILT_H_GLOBE_R2023A/"
            "GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_4326_3ss/V1-0/tiles/")
# The tiles' grid (R1_C1's corner, read from R1_C8's GeoTIFF): 12,000 × 12,000 cells of 3", 10° a side.
GHSL_TOP, GHSL_LEFT, GHSL_DEG = 89.09958317764332, -180.00791620856731, 10.0
CTX = ssl.create_default_context()  # certificates and host names verified
CHUNK = 1 << 20
# Part sizes S3 uploaders use, for a multipart ETag ("<md5 of the parts' md5s>-<parts>").
PART_SIZES = [n << 20 for n in (5, 8, 16, 32, 64, 128, 256, 512)] + [100_000_000]


def _host() -> str:
    """This Mac's name as the agent's status has it (its local host name), for temporary names."""
    try:
        name = subprocess.run(["scutil", "--get", "LocalHostName"], capture_output=True, text=True, timeout=5).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        name = ""
    return re.sub(r"[^A-Za-z0-9-]", "-", name or socket.gethostname().split(".")[0]) or "mac"


HOST = _host()
STOP = threading.Event()


def log(msg: str) -> None:
    print(f"{datetime.now().strftime('%Y-%m-%d %H:%M:%S')} {msg}", flush=True)


def gb(n: float) -> str:
    return f"{n / 1e9:.2f} GB"


class Refused(Exception):
    """A definite answer: the file isn't there for us (403, 404, 410) or changed (412)."""


class NasGone(Exception):
    pass


# ---- HTTP ------------------------------------------------------------------------------------------

def _open(url: str, headers: dict | None = None, method: str = "GET", timeout: float = 60):
    req = urllib.request.Request(url, method=method, headers={"User-Agent": UA, **(headers or {})})
    return urllib.request.urlopen(req, context=CTX, timeout=timeout)


def _wait(e: Exception, k: int) -> float | None:
    """How long to wait before asking again after `e` (the k-th failure in a row); None: don't."""
    if isinstance(e, urllib.error.HTTPError):
        if e.code in (403, 404, 410, 412, 416):
            return None
        if e.code not in (408, 429, 500, 502, 503, 504):
            return None
        ra = e.headers.get("Retry-After") if e.headers else None
        if ra and ra.strip().isdigit():
            return min(3600.0, float(ra))
    return min(600.0, 5.0 * 2 ** min(k, 7))


def retried(what: str, f, tries: int = 10):
    """f(), asked again after the failures that may pass (network, timeouts, 429, 5xx)."""
    for k in range(tries):
        if STOP.is_set():
            raise SystemExit(f"{what}: stopped")
        try:
            return f()
        except (urllib.error.URLError, http.client.HTTPException, TimeoutError, ConnectionError, ssl.SSLError, socket.timeout) as e:
            w = _wait(e, k)
            if w is None or k == tries - 1:
                if isinstance(e, urllib.error.HTTPError) and e.code in (403, 404, 410, 412, 416):
                    raise Refused(f"{what}: HTTP {e.code}") from e
                raise
            log(f"{what}: {e}; asking again in {w:.0f} s")
            time.sleep(w)


def get_bytes(url: str, rng: tuple[int, int] | None = None, timeout: float = 120) -> bytes:
    def once():
        h = {"Range": f"bytes={rng[0]}-{rng[1]}"} if rng else {}
        with _open(url, h, timeout=timeout) as r:
            d = r.read()
        if rng and len(d) != rng[1] - rng[0] + 1:
            raise http.client.IncompleteRead(d, rng[1] - rng[0] + 1 - len(d))
        return d
    return retried(url.rsplit("/", 1)[-1][:60] or url, once)


def get_json(url: str):
    return json.loads(get_bytes(url))


# ---- the coverage ----------------------------------------------------------------------------------

def read_poly(path: Path) -> list:
    """An osmosis .poly file's polygons (shapely), holes ('!' sections) taken out of the one before."""
    from shapely.geometry import Polygon
    lines = [l.strip() for l in path.read_text().splitlines()]
    out, i = [], 1
    while i < len(lines) and lines[i] != "END":
        hole = lines[i].startswith("!")
        i += 1
        ring = []
        while lines[i] != "END":
            x, y = lines[i].split()[:2]
            ring.append((float(x), float(y)))
            i += 1
        i += 1
        if hole and out:
            out[-1] = out[-1].difference(Polygon(ring))
        elif len(ring) >= 3:
            out.append(Polygon(ring).buffer(0))
    return out


def circle(lon: float, lat: float, km: float):
    from shapely.geometry import Polygon
    pts = []
    for k in range(64):
        a = 2 * math.pi * k / 64
        pts.append((lon + km / (111.32 * max(0.01, math.cos(math.radians(lat)))) * math.sin(a), lat + km / 110.57 * math.cos(a)))
    return Polygon(pts)


def coverage(args) -> tuple[object, list[str]]:
    """The coverage grown by the margin (a prepared shapely geometry), and what it's made of."""
    from shapely.geometry import shape
    from shapely.ops import unary_union
    from shapely.prepared import prep
    parts, what = [], []
    if args.coverage:
        d = json.loads(Path(args.coverage).read_text())
        feats = d["features"] if d.get("type") == "FeatureCollection" else [d]
        for f in feats:
            parts.append(shape(f.get("geometry", f)).buffer(0))
        what.append(f"file {args.coverage}")
    else:
        server = args.server.rstrip("/")
        regs = None
        for k in range(8):
            d = get_json(f"{server}/api/regions")
            if "regions" in d:
                regs = d["regions"]
                break
            log(f"the map's server has no regions now ({d.get('error', d)}); asking again")
            time.sleep(15 * (k + 1))
        if regs is None:
            raise SystemExit("the map's server didn't list the regions")
        for r in regs:
            what.append(r["id"])
            for entry in r["outline"]:
                kind, _, arg = entry.partition(":")
                if kind == "osm":
                    g = None
                    for k in range(8):
                        d = get_json(f"{server}/api/areas/{arg}")
                        if d.get("geometry"):
                            g = d["geometry"]
                            break
                        time.sleep(15 * (k + 1))
                    if g is None:
                        raise SystemExit(f"{r['id']}: the server has no outline for {entry}")
                    parts.append(shape(g).buffer(0))
                elif kind == "place":
                    lon, lat, km = (float(v) for v in arg.split(","))
                    parts.append(circle(lon, lat, km))
                elif kind == "poly":
                    parts.extend(read_poly(Path(args.root) / "inputs/outlines" / arg))
                elif kind == "geofabrik":
                    parts.extend(read_poly(Path(args.root) / "inputs/outlines/geofabrik" / f"{arg}.poly"))
                else:
                    raise SystemExit(f"{r['id']}: an outline entry this can't read: {entry}")
    grown = []
    for p in parts:
        if p.is_empty:
            continue
        # Degrees of longitude shrink with latitude: grow by the margin at the shape's highest
        # latitude, so it's at least the margin everywhere.
        lat = min(80.0, max(abs(p.bounds[1]), abs(p.bounds[3])))
        grown.append(p.buffer(args.margin_km / (111.32 * math.cos(math.radians(lat))), resolution=4))
    if not grown:
        raise SystemExit("no coverage")
    return prep(unary_union(grown)), what


# ---- Overture --------------------------------------------------------------------------------------

def s3_list(prefix: str) -> list[dict]:
    ns = {"s": "http://s3.amazonaws.com/doc/2006-03-01/"}
    out, token = [], None
    while True:
        q = {"list-type": "2", "prefix": prefix}
        if token:
            q["continuation-token"] = token
        x = ET.fromstring(get_bytes(S3 + "?" + urllib.parse.urlencode(q)))
        for c in x.findall("s:Contents", ns):
            key = c.findtext("s:Key", namespaces=ns)
            if key.endswith(".parquet"):
                out.append({"key": key, "size": int(c.findtext("s:Size", namespaces=ns)), "etag": c.findtext("s:ETag", namespaces=ns).strip('"')})
        if x.findtext("s:IsTruncated", namespaces=ns) != "true":
            return out
        token = x.findtext("s:NextContinuationToken", namespaces=ns)


class _Tail(io.RawIOBase):
    """A remote file whose footer alone is read (pyarrow asks for nothing else)."""

    def __init__(self, url: str, size: int):
        self.size, self.pos = size, 0
        tail = get_bytes(url, (max(0, size - 65536), size - 1))
        if tail[-4:] != b"PAR1":
            raise ValueError(f"{url}: not a parquet file")
        need = int.from_bytes(tail[-8:-4], "little") + 8
        if need > len(tail):
            tail = get_bytes(url, (size - need, size - 1))
        self.off, self.buf = size - len(tail), tail

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, o, whence=0):
        self.pos = o if whence == 0 else self.pos + o if whence == 1 else self.size + o
        return self.pos

    def readinto(self, b):
        n = min(len(b), self.size - self.pos)
        if self.pos < self.off:
            raise OSError("read before the footer")
        b[:n] = self.buf[self.pos - self.off:self.pos - self.off + n]
        self.pos += n
        return n


def row_groups(url: str, size: int) -> dict:
    """A parquet file's rows and its row groups' boxes [w, s, e, n, rows], from its footer."""
    import pyarrow.parquet as pq
    md = pq.read_metadata(_Tail(url, size))
    col = {md.schema.column(i).path: i for i in range(md.num_columns)}
    rgs = []
    for g in range(md.num_row_groups):
        rg = md.row_group(g)
        st = {}
        for name in ("bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax"):
            s = rg.column(col[name]).statistics
            st[name] = (s.min, s.max) if s is not None and s.has_min_max else None
        if None in st.values():
            # No statistics: the whole world, so it's taken.
            rgs.append([-180.0, -90.0, 180.0, 90.0, rg.num_rows])
        else:
            rgs.append([st["bbox.xmin"][0], st["bbox.ymin"][0], st["bbox.xmax"][1], st["bbox.ymax"][1], rg.num_rows])
    return {"rows": md.num_rows, "rgs": rgs}


def write_json(path: Path, obj, gz: bool = False) -> None:
    """Written whole: a temporary name, flushed, renamed."""
    tmp = path.with_name(f"{path.name}.{HOST}.{os.getpid()}.tmp")
    data = json.dumps(obj, indent=None if gz else 1, sort_keys=True).encode()
    with tmp.open("wb") as f:
        f.write(gzip.compress(data, mtime=0) if gz else data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def overture_jobs(args, cov) -> tuple[list[dict], Path, dict]:
    rel = args.release
    base = Path(args.root) / "sources/overture" / rel.replace(".", "-")
    base.mkdir(parents=True, exist_ok=True)
    # Every file's row groups (cached: a release's files never change).
    cache_path = base / "footers.json.gz"
    cache = {}
    if cache_path.exists():
        try:
            cache = json.loads(gzip.decompress(cache_path.read_bytes()))
        except (OSError, ValueError) as e:
            log(f"{cache_path}: unreadable ({e}); reading the footers again")
    listed = []
    for t in TYPES:
        prefix = f"release/{rel}/theme=buildings/type={t}/"
        files = s3_list(prefix)
        if not files and cache:
            # The release gone from S3 (Overture keeps about two months): the footers read before
            # list its files, so the coverage's files here are known whole and skipped; one the
            # coverage needs and that isn't here fails (it waits for the next pinned release).
            files = [{"key": k, "size": v["size"], "etag": v["etag"]} for k, v in sorted(cache.items()) if k.startswith(prefix)]
            log(f"overture {rel}: no {t} files on S3 now; going by the {len(files)} footers read before")
        if not files:
            raise SystemExit(f"Overture's release {rel} has no {t} files on S3 (gone, or another name?)")
        listed += files
    log(f"overture {rel}: {len(listed)} files listed, {gb(sum(f['size'] for f in listed))}")
    todo = [f for f in listed if cache.get(f["key"], {}).get("etag") != f["etag"]]
    if todo:
        log(f"reading {len(todo)} files' footers")
        done = [0]

        def one(f):
            r = row_groups(S3 + f["key"], f["size"])
            done[0] += 1
            if done[0] % 50 == 0:
                log(f"footers: {done[0]}/{len(todo)}")
            return f, r

        with ThreadPoolExecutor(min(4, args.jobs + 1)) as ex:
            for f, r in ex.map(one, todo):
                cache[f["key"]] = {"etag": f["etag"], "size": f["size"], **r}
        write_json(cache_path, cache, gz=True)
    from shapely.geometry import box
    jobs = []
    for f in listed:
        c = cache[f["key"]]
        hit = [g for g in c["rgs"] if cov.intersects(box(g[0], g[1], g[2], g[3]))]
        if not hit:
            continue
        rel_path = f["key"].split(f"release/{rel}/", 1)[1]
        jobs.append({
            "src": "overture", "url": S3 + f["key"], "dest": base / rel_path, "size": f["size"], "etag": f["etag"],
            "name": rel_path, "rows": c["rows"], "row_groups": len(c["rgs"]), "rows_near": sum(g[4] for g in hit),
            "bbox": [min(g[0] for g in c["rgs"]), min(g[1] for g in c["rgs"]), max(g[2] for g in c["rgs"]), max(g[3] for g in c["rgs"])],
        })
    return jobs, base, cache


# ---- GHSL ------------------------------------------------------------------------------------------

def ghsl_jobs(args, cov) -> tuple[list[dict], Path]:
    from shapely.geometry import box
    base = Path(args.root) / "sources/ghsl" / GHSL_RELEASE
    base.mkdir(parents=True, exist_ok=True)
    page = get_bytes(GHSL_URL).decode("utf-8", "replace")
    names = sorted(set(re.findall(rf'href="({GHSL_PRODUCT}_R(\d+)_C(\d+)\.zip)"', page)))
    if not names:
        raise SystemExit(f"GHSL: no tiles listed at {GHSL_URL}")
    jobs = []
    for name, r, c in names:
        top = GHSL_TOP - GHSL_DEG * (int(r) - 1)
        left = GHSL_LEFT + GHSL_DEG * (int(c) - 1)
        if top - GHSL_DEG < -90 or top > 90.5:
            continue
        b = [left, top - GHSL_DEG, left + GHSL_DEG, top]
        if cov.intersects(box(*b)):
            jobs.append({"src": "ghsl", "url": GHSL_URL + name, "dest": base / name, "size": None, "etag": None, "name": name, "bbox": b})

    # Each tile's size (the listing rounds them), so one already here is known whole.
    def head(j):
        def once():
            with _open(j["url"], method="HEAD", timeout=60) as r:
                return int(r.headers["Content-Length"])
        j["size"] = retried(j["name"], once)

    with ThreadPoolExecutor(3) as ex:
        list(ex.map(head, jobs))
    return jobs, base


# ---- downloading -----------------------------------------------------------------------------------

class Throttle:
    """At most `rate` bytes a second over every transfer (0: no cap)."""

    def __init__(self, rate: float):
        self.rate, self.lock, self.t, self.debt = rate, threading.Lock(), time.monotonic(), 0.0

    def take(self, n: int) -> None:
        if self.rate <= 0:
            return
        with self.lock:
            now = time.monotonic()
            self.debt = max(0.0, self.debt - (now - self.t) * self.rate) + n
            self.t = now
            wait = self.debt / self.rate - 1.0  # a second's burst allowed
        if wait > 0:
            time.sleep(wait)


class Progress:
    def __init__(self, files: int, total: int):
        self.files, self.total = files, total
        self.done_files, self.done, self.lock = 0, 0, threading.Lock()
        self.t0 = time.monotonic()
        self.window = [(self.t0, 0)]

    def add(self, n: int) -> None:
        with self.lock:
            self.done += n

    def file_done(self) -> None:
        with self.lock:
            self.done_files += 1

    def line(self) -> str:
        with self.lock:
            now = time.monotonic()
            self.window.append((now, self.done))
            while len(self.window) > 2 and now - self.window[0][0] > 300:
                self.window.pop(0)
            t, d = self.window[0]
            rate = (self.done - d) / max(1e-9, now - t)
            left = (self.total - self.done) / rate if rate > 0 else float("inf")
            eta = f"~{left / 60:.0f} min left" if left != float("inf") else "time left unknown"
            return (f"progress: {self.done_files}/{self.files} files, {gb(self.done)} of {gb(self.total)} "
                    f"({rate / 1e6:.1f} MB/s over the last {min(300, now - t):.0f} s, {eta})")


class PartHasher:
    """The S3 ETag of a file as it comes in: its MD5, or for a multipart upload ("…-N") the MD5 of
    its parts' MD5s, for each part size that makes N parts of the file's size."""

    def __init__(self, size: int, etag: str):
        self.etag = etag
        if "-" in etag:
            n = int(etag.rsplit("-", 1)[1])
            self.sizes = [p for p in PART_SIZES if -(-size // p) == n] or [-(-size // n)]
        else:
            self.sizes = [0]
        self.state = [{"p": p, "md5": hashlib.md5(), "in": 0, "parts": []} for p in self.sizes]

    def update(self, b: bytes) -> None:
        for s in self.state:
            p, mv = s["p"], memoryview(b)
            if p == 0:
                s["md5"].update(mv)
                continue
            while len(mv):
                k = min(len(mv), p - s["in"])
                s["md5"].update(mv[:k])
                s["in"] += k
                mv = mv[k:]
                if s["in"] == p:
                    s["parts"].append(s["md5"].digest())
                    s["md5"], s["in"] = hashlib.md5(), 0

    def ok(self) -> bool:
        for s in self.state:
            if s["p"] == 0:
                if s["md5"].hexdigest() == self.etag:
                    return True
                continue
            parts = s["parts"] + ([s["md5"].digest()] if s["in"] else [])
            if f"{hashlib.md5(b''.join(parts)).hexdigest()}-{len(parts)}" == self.etag:
                return True
        return False


def wait_for_nas(root: Path, hours: float) -> None:
    t0 = time.monotonic()
    while not root.is_dir():
        if time.monotonic() - t0 > hours * 3600:
            raise NasGone(f"the NAS ({root}) has been away {hours} h")
        log(f"the NAS ({root}) isn't there; waiting")
        time.sleep(60)


def fetch(job: dict, root: Path, throttle: Throttle, prog: Progress, nas_wait_h: float) -> tuple[str, int]:
    """One file into place (see the module's docstring): what was done, and the bytes this run
    fetched of it (the rest were here from a run cut short)."""
    dest: Path = job["dest"]
    size = job["size"]
    if dest.exists() and dest.stat().st_size == size:
        prog.add(size)
        prog.file_done()
        return "here", 0
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(f"{dest.name}.{HOST}.tmp")
    fresh_starts, k = 0, 0
    kept = None  # the bytes a run before left in the temporary file
    while True:
        if STOP.is_set():
            raise SystemExit("stopped")
        counted = 0  # what this attempt added to the progress (taken off again if it fails)
        try:
            have = tmp.stat().st_size if tmp.exists() else 0
            if have > size:
                tmp.unlink()
                have = 0
            if kept is None:
                kept = have
            hasher = PartHasher(size, job["etag"]) if job["etag"] else None
            if have and hasher:
                # The bytes already here, hashed again (read back from the NAS).
                with tmp.open("rb") as f:
                    while b := f.read(8 * CHUNK):
                        hasher.update(b)
            prog.add(have)
            counted = have
            got = have
            if have < size:
                headers = {"Range": f"bytes={have}-"} if have else {}
                if job["etag"]:
                    headers["If-Match"] = f'"{job["etag"]}"'
                try:
                    with _open(job["url"], headers, timeout=60) as r:
                        if have and r.status != 206:
                            # The range wasn't honoured: from the start.
                            prog.add(-have)
                            counted -= have
                            have = got = kept = 0
                            hasher = PartHasher(size, job["etag"]) if job["etag"] else None
                        with tmp.open("ab" if have else "wb") as f:
                            while True:
                                if STOP.is_set():
                                    raise SystemExit("stopped")
                                b = r.read(CHUNK)
                                if not b:
                                    break
                                throttle.take(len(b))
                                f.write(b)
                                if hasher:
                                    hasher.update(b)
                                got += len(b)
                                prog.add(len(b))
                                counted += len(b)
                            f.flush()
                            os.fsync(f.fileno())
                except urllib.error.HTTPError as e:
                    if e.code == 412:
                        # The object isn't the one listed (replaced since?): from the start, next run.
                        tmp.unlink(missing_ok=True)
                        raise Refused(f"{job['name']}: changed on the server since it was listed (412); run again") from e
                    raise
            if got != size:
                raise http.client.IncompleteRead(b"", size - got)
            # Checked before it's put in place.
            if hasher and not hasher.ok():
                why = "its ETag doesn't match"
            elif job["src"] == "ghsl" and (why := zip_bad(tmp)):
                pass
            else:
                why = None
            if why:
                tmp.unlink(missing_ok=True)
                prog.add(-counted)
                kept = 0
                fresh_starts += 1
                if fresh_starts > 1:
                    raise RuntimeError(f"{job['name']}: {why}, twice")
                log(f"{job['name']}: {why}; fetching it again from the start")
                continue
            os.replace(tmp, dest)
            prog.file_done()
            return "fetched", size - kept
        except (urllib.error.URLError, http.client.HTTPException, TimeoutError, ConnectionError, ssl.SSLError, socket.timeout) as e:
            prog.add(-counted)
            w = _wait(e, k)
            if w is None:
                if isinstance(e, urllib.error.HTTPError) and e.code in (403, 404, 410):
                    raise Refused(f"{job['name']}: HTTP {e.code}") from e
                raise
            k += 1
            if k > 12:
                raise
            log(f"{job['name']}: {type(e).__name__}: {e}; going on from where it is in {w:.0f} s")
            time.sleep(w)
        except OSError as e:
            # The NAS (the network's errors are above).
            prog.add(-counted)
            log(f"{job['name']}: {e}; checking the NAS")
            time.sleep(10)
            wait_for_nas(root, nas_wait_h)
            k += 1
            if k > 12:
                raise


def zip_bad(path: Path) -> str | None:
    try:
        with zipfile.ZipFile(path) as z:
            bad = z.testzip()
            if bad:
                return f"{bad} fails its CRC"
            if not any(n.endswith(".tif") for n in z.namelist()):
                return "no GeoTIFF in it"
    except (zipfile.BadZipFile, OSError) as e:
        return f"not a whole zip ({e})"
    return None


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", required=True, help="the NAS's project folder")
    ap.add_argument("--release", default="2026-09-23.1", help="Overture's release")
    ap.add_argument("--server", default="http://localhost:8080", help="the map's server, for the coverage")
    ap.add_argument("--coverage", help="the coverage as a GeoJSON file instead")
    ap.add_argument("--margin-km", type=float, default=20.0)
    ap.add_argument("--jobs", type=int, default=2, help="transfers at once (2-4)")
    ap.add_argument("--max-mb-s", type=float, default=3.0, help="MB/s in all (0: no cap)")
    ap.add_argument("--only", choices=["overture", "ghsl"])
    ap.add_argument("--limit", type=int, default=0, help="only the n smallest files of each source (a test)")
    ap.add_argument("--nas-wait-h", type=float, default=6.0)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    args.jobs = max(1, min(4, args.jobs))
    root = Path(args.root)
    if not root.is_dir() or not (root / "sources").is_dir():
        raise SystemExit(f"{root}: not the NAS's project folder (no sources/)")

    lock = open(Path(tempfile.gettempdir()) / "bldfetch.lock", "a+")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        raise SystemExit("another bldfetch.py runs on this Mac")
    for s in (signal.SIGTERM, signal.SIGINT):
        signal.signal(s, lambda *_: STOP.set())

    log(f"bldfetch: pid {os.getpid()} on {HOST}, root {root}, {args.jobs} transfers at once, "
        f"{'no cap' if args.max_mb_s <= 0 else f'at most {args.max_mb_s:g} MB/s'}")
    cov, what = coverage(args)
    log(f"coverage: {len(what)} regions ({', '.join(what[:6])}{', …' if len(what) > 6 else ''}), grown {args.margin_km:g} km")

    jobs, ov_base, ov_cache, gh_base = [], None, {}, None
    if args.only in (None, "overture"):
        ov, ov_base, ov_cache = overture_jobs(args, cov)
        if args.limit:
            ov = sorted(ov, key=lambda j: j["size"])[:args.limit]
        log(f"overture: {len(ov)} files meet the coverage, {gb(sum(j['size'] for j in ov))}, "
            f"{sum(j['rows_near'] for j in ov) / 1e6:.1f} M buildings in their row groups that meet it")
        jobs += ov
    if args.only in (None, "ghsl"):
        gh, gh_base = ghsl_jobs(args, cov)
        if args.limit:
            gh = sorted(gh, key=lambda j: j["size"])[:args.limit]
        log(f"ghsl: {len(gh)} tiles meet the coverage, {gb(sum(j['size'] for j in gh))}")
        jobs += gh

    known = sum(j["size"] for j in jobs)
    here = {j["name"] for j in jobs if j["dest"].exists() and j["dest"].stat().st_size == j["size"]}
    log(f"{len(jobs)} files, {len(here)} already here; {gb(known - sum(j['size'] for j in jobs if j['name'] in here))} to fetch")
    # (What's left to fetch: the files here already take no more room.)
    left = known - sum(j["size"] for j in jobs if j["name"] in here)
    free = os.statvfs(root)
    if free.f_bavail * free.f_frsize < left + (100 << 30):
        raise SystemExit(f"the NAS has {gb(free.f_bavail * free.f_frsize)} free: too little for {gb(left)} and 100 GB to spare")
    if args.dry_run:
        for j in jobs:
            print(f"  {'here ' if j['name'] in here else 'fetch'} {j['src']:8s} {j['size'] / 1e6:9.1f} MB  {j['name']}")
        return

    throttle = Throttle(args.max_mb_s * 1e6)
    prog = Progress(len(jobs), known)
    stop_report = threading.Event()

    def report():
        while not stop_report.wait(60):
            log(prog.line())

    threading.Thread(target=report, daemon=True).start()
    failed: list[str] = []
    done_meta: dict[str, dict] = {}
    lock_meta = threading.Lock()

    def save_indexes():
        at = datetime.now(timezone.utc).isoformat(timespec="seconds")
        if ov_base is not None:
            idx_path = ov_base / "buildings.json"
            try:
                idx = json.loads(idx_path.read_text()) if idx_path.exists() else {}
            except (OSError, ValueError):
                idx = {}
            files = idx.get("files", {})
            for j in jobs:
                if j["src"] == "overture" and j["dest"].exists() and j["dest"].stat().st_size == j["size"]:
                    f = files.get(j["name"], {})
                    files[j["name"]] = {**f, "size": j["size"], "etag": j["etag"], "rows": j["rows"], "row_groups": j["row_groups"],
                                        "rows_near": j["rows_near"], "bbox": j["bbox"], "checked": f.get("checked") or done_meta.get(j["name"], {}).get("checked", "size")}
            write_json(idx_path, {"fmt": 1, "release": args.release, "source": S3 + f"release/{args.release}/theme=buildings/",
                                  "terms": "ODbL 1.0 (the theme; its sources' licences per row in `sources`)",
                                  "coverage": {"regions": what, "margin_km": args.margin_km, "at": at}, "files": dict(sorted(files.items()))})
        if gh_base is not None:
            idx_path = gh_base / "index.json"
            try:
                idx = json.loads(idx_path.read_text()) if idx_path.exists() else {}
            except (OSError, ValueError):
                idx = {}
            tiles = idx.get("tiles", {})
            for j in jobs:
                if j["src"] == "ghsl" and j["dest"].exists() and j["dest"].stat().st_size == j["size"]:
                    tiles[j["name"]] = {"size": j["size"], "bbox": j["bbox"], "checked": "zip CRC-32"}
            write_json(idx_path, {"fmt": 1, "product": GHSL_PRODUCT, "source": GHSL_URL,
                                  "terms": "© European Union, 1995-2026; CC BY 4.0", "tiles": dict(sorted(tiles.items()))})

    def save():
        try:
            save_indexes()
        except OSError as e:
            log(f"the indexes weren't written now ({e}); the next save writes them")

    def one(job):
        if STOP.is_set():
            return
        try:
            t0 = time.monotonic()
            what, new = fetch(job, root, throttle, prog, args.nas_wait_h)
            if what == "fetched":
                dt = time.monotonic() - t0
                check = "ETag (MD5 of its parts)" if job["etag"] else "zip CRC-32"
                kept = f", {(job['size'] - new) / 1e6:.1f} MB of it kept from before" if new < job["size"] else ""
                log(f"{job['name']}: {new / 1e6:.1f} MB in {dt:.0f} s ({new / 1e6 / max(dt, 1e-9):.1f} MB/s){kept}; {check} checked")
                with lock_meta:
                    done_meta[job["name"]] = {"checked": check}
                    if len(done_meta) % 10 == 0:
                        save()
        except SystemExit:
            if not STOP.is_set():
                raise
        except (Refused, RuntimeError, NasGone, OSError, urllib.error.URLError, http.client.HTTPException) as e:
            log(f"{job['name']}: FAILED: {e}")
            with lock_meta:
                failed.append(job["name"])

    # GHSL's small tiles first, then Overture's parts, then its buildings.
    order = sorted(jobs, key=lambda j: (j["src"] != "ghsl", "type=building_part/" not in j["name"], j["name"]))
    with ThreadPoolExecutor(args.jobs) as ex:
        list(ex.map(one, order))
    stop_report.set()
    log(prog.line())
    if STOP.is_set():
        log("stopped; what was fetched is kept, and a run again goes on from there")
    save()
    if failed:
        raise SystemExit(f"{len(failed)} files failed: {', '.join(failed[:5])}{' …' if len(failed) > 5 else ''} (run again to retry)")
    if not STOP.is_set():
        log(f"done: {len(jobs)} files in place")


if __name__ == "__main__":
    main()
