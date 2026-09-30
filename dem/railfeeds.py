#!/usr/bin/env python3
"""Find the GTFS feeds that run passenger rail (trains, metros, trams, funiculars) in our regions.

From the Mobility Database catalogue (data/rail/feeds_v2.csv, files.mobilitydatabase.org): active
GTFS feeds in our countries whose bounding box touches our regions. For each, only routes.txt is
read, straight out of the zip by HTTP range requests (the catalogue's mirror, else the operator's
URL when it needs no key), to count its rail routes. Writes data/rail/feeds.json.

usage: railfeeds.py
"""
from __future__ import annotations

import csv
import io
import json
import struct
import subprocess
import sys
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from shapely.geometry import box

from leaftype import regions

ROOT = Path(__file__).resolve().parent.parent
R = ROOT / "data" / "rail"
UA = "road-elevations/0.1 (personal offline map)"
COUNTRIES = {"CA", "US", "FR", "ES", "PT", "GB", "IE", "IM", "JE", "GG", "HK", "MC", "AD", "GI", "JP", "TW", "SG"}
RAIL_TYPES = {0, 1, 2, 5, 7, 12} | set(range(100, 200)) | set(range(400, 500)) | set(range(900, 1000)) | {1400}


def curl(url: str, rng: str | None = None, head: bool = False) -> tuple[int, bytes, dict]:
    args = ["curl", "-sS", "-L", "-m", "90", "-A", UA, "-D", "-", "-o", "-"]
    if rng:
        args += ["-r", rng]
    if head:
        args += ["-I"]
    r = subprocess.run(args + [url], capture_output=True)
    out = r.stdout
    # Split the (last) header block from the body.
    headers, body, code = {}, out, 0
    while body.startswith(b"HTTP/"):
        end = body.find(b"\r\n\r\n")
        if end < 0:
            break
        block = body[:end].decode("latin-1").split("\r\n")
        code = int(block[0].split()[1])
        headers = {k.lower(): v.strip() for k, _, v in (l.partition(":") for l in block[1:])}
        body = body[end + 4:]
    return code, body, headers


def zip_member(url: str, name: str, size: int) -> bytes | None:
    """One member of a remote zip, by range requests (central directory, then the member)."""
    tail_len = min(size, 1 << 20)
    code, tail, _ = curl(url, f"{size - tail_len}-{size - 1}")
    if code not in (200, 206):
        return None
    if code == 200:
        tail = tail[-tail_len:]
    i = tail.rfind(b"PK\x05\x06")
    if i < 0:
        return None
    cd_size, cd_off = struct.unpack("<II", tail[i + 12:i + 20])
    if 0xFFFFFFFF in (cd_size, cd_off):
        j = tail.rfind(b"PK\x06\x06")
        cd_size, cd_off = struct.unpack("<QQ", tail[j + 40:j + 56])
    start = size - tail_len
    cd = tail[cd_off - start:cd_off - start + cd_size] if cd_off >= start else curl(url, f"{cd_off}-{cd_off + cd_size - 1}")[1]
    p = 0
    while p + 46 <= len(cd) and cd[p:p + 4] == b"PK\x01\x02":
        method, = struct.unpack("<H", cd[p + 10:p + 12])
        csz, usz = struct.unpack("<II", cd[p + 20:p + 28])
        nl, el, cl = struct.unpack("<HHH", cd[p + 28:p + 34])
        off, = struct.unpack("<I", cd[p + 42:p + 46])
        fname = cd[p + 46:p + 46 + nl].decode("utf-8", "replace")
        extra = cd[p + 46 + nl:p + 46 + nl + el]
        q = 0
        while q + 4 <= len(extra):
            hid, hl = struct.unpack("<HH", extra[q:q + 4])
            if hid == 1:
                vals = iter(struct.unpack("<" + "Q" * (hl // 8), extra[q + 4:q + 4 + hl - hl % 8]))
                if usz == 0xFFFFFFFF:
                    usz = next(vals)
                if csz == 0xFFFFFFFF:
                    csz = next(vals)
                if off == 0xFFFFFFFF:
                    off = next(vals)
            q += 4 + hl
        if fname.split("/")[-1] == name:
            _, lh, _ = curl(url, f"{off}-{off + 29}")
            n2, e2 = struct.unpack("<HH", lh[26:30])
            data_start = off + 30 + n2 + e2
            _, comp, _ = curl(url, f"{data_start}-{data_start + csz - 1}")
            return comp if method == 0 else zlib.decompressobj(-15).decompress(comp)
        p += 46 + nl + el + cl
    return None


def check(feed: dict) -> dict:
    url = feed["url"]
    code, _, h = curl(url, head=True)
    size = int(h.get("content-length", 0) or 0)
    feed["size_mb"] = round(size / 1e6, 1)
    if code != 200 or not size:
        feed["status"] = f"http {code}"
        return feed
    routes = zip_member(url, "routes.txt", size)
    if routes is None:
        feed["status"] = "no routes.txt (or no range requests)"
        return feed
    rail = []
    for r in csv.DictReader(io.StringIO(routes.decode("utf-8-sig", "replace"))):
        try:
            t = int(r.get("route_type", ""))
        except ValueError:
            continue
        if t in RAIL_TYPES:
            rail.append(f"{r.get('route_short_name') or r.get('route_long_name') or r.get('route_id')} ({t})")
    feed["rail_routes"] = len(rail)
    feed["examples"] = rail[:8]
    feed["status"] = "ok"
    return feed


def main():
    region = regions()
    feeds = []
    with (R / "feeds_v2.csv").open(encoding="utf-8") as f:
        for row in csv.DictReader(f):
            if row["data_type"] != "gtfs" or row["location.country_code"] not in COUNTRIES or row["status"] in ("deprecated", "inactive"):
                continue
            if row["redirect.id"]:
                continue
            try:
                bb = box(float(row["location.bounding_box.minimum_longitude"]), float(row["location.bounding_box.minimum_latitude"]),
                         float(row["location.bounding_box.maximum_longitude"]), float(row["location.bounding_box.maximum_latitude"]))
            except ValueError:
                continue
            if not region.intersects(bb):
                continue
            url = row["urls.latest"] or (row["urls.direct_download"] if row["urls.authentication_type"] in ("", "0") else "")
            if not url:
                continue
            feeds.append({"id": row["id"], "provider": row["provider"], "name": row["name"], "country": row["location.country_code"],
                          "subdivision": row["location.subdivision_name"], "url": url, "licence": row["urls.license"]})
    print(f"{len(feeds)} candidate feeds", file=sys.stderr)
    with ThreadPoolExecutor(8) as ex:
        done = list(ex.map(check, feeds))
    rail = [f for f in done if f.get("rail_routes")]
    (R / "feeds.json").write_text(json.dumps(sorted(rail, key=lambda f: (f["country"], f["provider"])), ensure_ascii=False, indent=1))
    (R / "feeds-checked.json").write_text(json.dumps(done, ensure_ascii=False, indent=1))
    print(f"{len(rail)} feeds with rail, {sum(f['size_mb'] for f in rail):.0f} MB; "
          f"{sum(1 for f in done if f.get('status') != 'ok')} unreadable", file=sys.stderr)


if __name__ == "__main__":
    main()
