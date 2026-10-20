"""The depth grids' packs (docs/inputs.md §6.10; task #452).

A source's grids are kept in packs, never one file a grid: the NAS (three spinning disks over SMB)
pays a directory lookup, a stat and an open for every file, and seeks between them, so tens of
thousands of small GeoTIFFs made it seek-bound. A pack is a plain tar (GNU format, every header's
time, owner and mode fixed, so the same grids give the same bytes) of whole GeoTIFFs, one a source
per z6 tile (the tile of each grid's footprint's centre; an area may have several packs, one per
producer run or chunk), named `<area>.<hash16>.tar` (SHA-256 of its bytes): a depth job reads its
z6 tile's grids from one file. The index names each grid's pack (`pack`) and its files' byte ranges
in it and their hashes (`at`: {"file": [offset, length, hash16], "zoom_file": …, "low_file": …}: every
member's bytes recorded as packed, SHA-256's first 16 digits); the build opens a pack once
and reads each GeoTIFF in place by its range (crate::depth::grids), as it would the loose file.
"""
from __future__ import annotations

import hashlib
import io
import math
import os
import tarfile
from pathlib import Path

FILE_KEYS = ("file", "zoom_file", "low_file")


def area(feature: dict) -> str:
    """The z6 tile of the centre of a feature's footprint: `6-<x>-<y>`."""
    xs, ys = [], []

    def walk(v):
        if isinstance(v, list) and len(v) >= 2 and isinstance(v[0], (int, float)):
            xs.append(v[0])
            ys.append(v[1])
        elif isinstance(v, list):
            for w in v:
                walk(w)

    walk(feature["geometry"]["coordinates"])
    lon = (min(xs) + max(xs)) / 2
    lat = max(-85.05112878, min(85.05112878, (min(ys) + max(ys)) / 2))
    n = 64
    x = min(n - 1, max(0, int((lon + 180) / 360 * n)))
    y = min(n - 1, max(0, int((1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n)))
    return f"6-{x}-{y}"


def by_area(features: list[dict]) -> dict[str, list[dict]]:
    """The features not yet packed, by area (each area's in file order)."""
    out: dict[str, list[dict]] = {}
    for f in sorted(features, key=lambda f: f["properties"]["file"]):
        if "pack" not in f["properties"]:
            out.setdefault(area(f), []).append(f)
    return out


def names(feature: dict) -> list[str]:
    """A feature's files, in FILE_KEYS order."""
    return [feature["properties"][k] for k in FILE_KEYS if k in feature["properties"]]


class PackWriter:
    """One pack being written into `folder` (as `.<area>.writing.tar`, renamed by its hash once
    whole): add each feature's files' bytes, then `finish`."""

    def __init__(self, folder: Path, area_: str):
        self.folder, self.area = Path(folder), area_
        self.tmp = self.folder / f".{area_}.writing.tar"
        self.folder.mkdir(parents=True, exist_ok=True)
        self.f = open(self.tmp, "wb")
        self.tar = tarfile.open(fileobj=self.f, mode="w", format=tarfile.GNU_FORMAT)
        self.at: dict[str, tuple[int, int, str]] = {}

    def add(self, name: str, data: bytes):
        if name in self.at:
            raise ValueError(f"{name} twice in one pack")
        ti = tarfile.TarInfo(name)
        ti.size, ti.mtime, ti.mode, ti.uid, ti.gid, ti.uname, ti.gname = len(data), 0, 0o644, 0, 0, "", ""
        start = self.tar.offset + len(ti.tobuf(self.tar.format, self.tar.encoding, self.tar.errors))
        self.tar.addfile(ti, io.BytesIO(data))
        self.at[name] = (start, len(data), hashlib.sha256(data).hexdigest()[:16])

    def finish(self) -> tuple[str, dict[str, tuple[int, int, str]]]:
        """The pack's name and each file's (offset, length, SHA-256's first 16 digits) in it."""
        self.tar.close()
        self.f.flush()
        os.fsync(self.f.fileno())
        self.f.close()
        h = hashlib.sha256()
        with open(self.tmp, "rb") as f:
            for b in iter(lambda: f.read(1 << 20), b""):
                h.update(b)
        name = f"{self.area}.{h.hexdigest()[:16]}.tar"
        os.replace(self.tmp, self.folder / name)
        return name, self.at


def set_packed(feature: dict, pack: str, at: dict[str, tuple[int, int, str]]):
    """Names a feature's pack and its files' ranges in it."""
    p = feature["properties"]
    p["pack"] = pack
    p["at"] = {k: list(at[p[k]]) for k in FILE_KEYS if k in p}


def check_hash(feature: dict, data: bytes):
    """A grid's depth file against its index's `hash` (grid.file_hash: SHA-256's first 16 digits)."""
    want = feature["properties"].get("hash")
    if want and hashlib.sha256(data).hexdigest()[:16] != want:
        raise SystemExit(f"{feature['properties']['file']}: its bytes aren't its index's hash {want}")


def pack_folder(folder: Path, features: list[dict], delete: bool = True) -> list[str]:
    """The loose grids of `features` in `folder` packed there by area (their features updated), the
    loose files deleted once their pack is whole (`delete`); the packs written."""
    folder = Path(folder)
    made = []
    for a, feats in by_area(features).items():
        w = PackWriter(folder, a)
        for f in feats:
            for k, n in zip([k for k in FILE_KEYS if k in f["properties"]], names(f)):
                data = (folder / n).read_bytes()
                if k == "file":
                    check_hash(f, data)
                w.add(n, data)
        pack, at = w.finish()
        for f in feats:
            set_packed(f, pack, at)
        made.append(pack)
        if delete:
            for f in feats:
                for n in names(f):
                    (folder / n).unlink(missing_ok=True)
    return made


def gdal_path(folder: Path, feature: dict, key: str = "file") -> str:
    """A packed grid's file `key` as GDAL (rasterio) opens it in place: `/vsisubfile/`."""
    p = feature["properties"]
    off, n = p["at"][key][:2]
    return f"/vsisubfile/{off}_{n},{Path(folder) / p['pack']}"


def read_member(folder: Path, feature: dict, key: str = "file") -> bytes:
    """A packed grid's file `key`, read from its pack by its range (tests, checks)."""
    p = feature["properties"]
    off, n = p["at"][key][:2]
    with open(Path(folder) / p["pack"], "rb") as f:
        f.seek(off)
        return f.read(n)
