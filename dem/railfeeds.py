#!/usr/bin/env python3
"""The GTFS feeds that run passenger rail (trains, metros, trams, funiculars) where the coverage
is, each fetched once. The `rail-feeds` job runs it (docs/plan.md §6, Rail service); railgtfs.py
reads what it finds.

The feeds:
  - the Mobility Database catalogue's (--catalogue: feeds_v2.csv, files.mobilitydatabase.org, as
    downloaded once): active GTFS feeds of the countries the coverage is in (--countries, ISO
    3166-1, which the job works out from the pass's outlines) whose bounding box meets the coverage
    (--coverage), with a download that needs no key. Of those, the ones with rail routes: only
    routes.txt is read, straight out of the zip by HTTP range requests (the catalogue's mirror, else
    the operator's URL), once per feed (--checked keeps every feed's answer);
  - national operators the catalogue lacks (EXTRA), and feeds behind an API key (KEYED) whose key
    --keys holds (the NAS's inputs/keys.env, KEY=value lines: a key is read only to fetch its feed,
    and never written anywhere), each where its box meets the coverage, in the same countries.
A feed another `replaces` (an operator's own feed, over an aggregate republishing it) is left out,
and so is one whose trains come from elsewhere (ELSEWHERE: the MTR's lines, researched by hand).

Each feed is downloaded once: --cache lists the zips the NAS already has (feed id → its path and
the day it was fetched), and only a feed not there is fetched, into --out/gtfs/<id>.zip (a zip left
there by a run cut short is used as it is). A cached zip without rail service in its window (see
railgtfs.py: it was out of date when fetched) is fetched again once it's a week old; its old copy
stays if the new one is the same or can't be had. A request that fails for want of an answer (no
connection, a 5xx) is tried three times; still failing, it fails the run, after the checks and
downloads that worked are written, so the job tries again later. A definite refusal (a 404, a file
that isn't a zip) leaves that feed out.

Writes --out/feeds.json, {"feeds": […]}: the feeds in the order railgtfs.py reads them (national
operators first, so a train they share with a regional aggregate counts as theirs; the catalogue's by
country and provider), each with its facts, `zip` ("cache" for the NAS's, "new" for a download,
none when left out, with `status` saying why) and `fetched`; and --out/checked.json (--checked with
this run's answers).

usage: railfeeds.py --catalogue feeds_v2.csv --coverage coverage.geojson --countries FR,ES,…
                    --checked checked.json --cache cache.json --keys keys.env --out dir
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import struct
import subprocess
import sys
import time
import zipfile
import zlib
from concurrent.futures import ThreadPoolExecutor
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

from shapely.geometry import Polygon, box
from shapely.ops import unary_union

import railgtfs

UA = "road-elevations/0.1 (personal offline map)"
# A stale cached zip is fetched again once it's this old.
REFETCH_DAYS = 7
# Tries of a request that gets no answer (no connection, a 5xx), and the waits between them.
TRIES = 3
WAIT_S = 20

# National operators the catalogue lacks, each with its country and the box its trains run in.
EXTRA = [
    {"id": "sncf", "provider": "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)", "url": "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip", "licence": "Licence Ouverte 2.0",
     "country": "FR", "bbox": [-5.2, 41.3, 9.6, 51.2]},
    {"id": "renfe-av-ld-md", "provider": "Renfe (AVE, Larga y Media Distancia)", "url": "https://ssl.renfe.com/gtransit/Fichero_AV_LD/google_transit.zip", "licence": "CC BY 4.0",
     "country": "ES", "bbox": [-9.4, 35.9, 3.4, 43.8]},
    {"id": "renfe-cercanias", "provider": "Renfe Cercanías / Rodalies", "url": "https://ssl.renfe.com/ftransit/Fichero_CER_FOMENTO/fomento_transit.zip", "licence": "CC BY 4.0",
     "country": "ES", "bbox": [-9.4, 35.9, 3.4, 43.8]},
    # Great Britain: the national rail timetable (RDG's ATOC CIF) as GTFS, rebuilt daily by Catenary
    # Transit (the official download needs a Rail Data Marketplace account).
    {"id": "gb-national-rail", "provider": "National Rail (GB), RDG timetable via Catenary Transit", "url": "https://github.com/catenarytransit/pfaedled-gtfs-actions/releases/download/latest/nationalrailuk.zip", "licence": "RDG open data (attribution)",
     "country": "GB", "bbox": [-6.5, 49.8, 1.8, 60.9]},
    # (The Transport Department's own feed, with Catenary Transit's shapes.)
    {"id": "hk-pfaedle", "provider": "Hong Kong Transport Department (trams, Peak Tram), via Catenary Transit", "url": "https://github.com/catenarytransit/pfaedled-gtfs-actions/releases/download/latest/hk-gtfs-pfaedle.zip", "licence": "DATA.GOV.HK terms",
     "country": "HK", "bbox": [113.8, 22.1, 114.5, 22.6], "replaces": ["mdb-1924"]},
]

# Catalogue feeds whose trains the map takes from elsewhere, and where: Just Use Wheels' Hong Kong
# feed has the MTR's lines, which come from the hand-researched MTR pairs (railgtfs.py would add
# them to those).
ELSEWHERE = {"mdb-2933": "the MTR's lines, researched by hand"}

# Feeds behind an API key, used when --keys has their key; `get` names how the download link is
# obtained. (The Mobility Database's Singapore feed has the same trains, mostly at other times.)
KEYED = [
    {"id": "sg-lta-train", "provider": "Land Transport Authority, LTA DataMall (MRT and LRT)", "licence": "Singapore Open Data Licence 1.0",
     "url": "https://datamall2.mytransport.sg/ltaodataservice/GTFSScheduleTrain", "key": "LTA_ACCOUNT_KEY", "get": "lta", "replaces": ["mdb-1076"],
     "country": "SG", "bbox": [103.5, 1.1, 104.2, 1.5]},
]


def today() -> date:
    return datetime.now(timezone.utc).date()


def progress(done: int, total: int, unit: str) -> None:
    print(f"progress: {min(done, total)}/{total} {unit}", file=sys.stderr, flush=True)


def coverage(path: str):
    """The coverage, as the job writes it (a feature per outline: its rings under the even–odd rule,
    its buffer in metres), as one geometry."""
    shapes = []
    for f in json.loads(Path(path).read_text())["features"]:
        g = None
        for poly in f["geometry"]["coordinates"]:
            for ring in poly:
                if len(ring) >= 3:
                    p = Polygon(ring).buffer(0)
                    g = p if g is None else g.symmetric_difference(p)
        if g is None:
            continue
        b = f["properties"].get("buffer_m") or 0
        shapes.append(g.buffer(b / 111_000) if b else g)
    return unary_union(shapes)


def keys(path: str | None) -> dict[str, str]:
    """inputs/keys.env's keys (KEY=value lines; # comments)."""
    p = Path(path) if path else None
    if not p or not p.exists():
        return {}
    return {k.strip(): v.strip() for k, _, v in (l.partition("=") for l in p.read_text().splitlines()) if v.strip() and not k.strip().startswith("#")}


def transient(code: int) -> bool:
    """An HTTP status that's worth asking again for: none (no connection), too many requests, 5xx."""
    return code == 0 or code == 429 or code >= 500


def curl(url: str, rng: str | None = None, head: bool = False) -> tuple[int, bytes, dict]:
    args = ["curl", "-sS", "-L", "-m", "90", "-A", UA, "-D", "-", "-o", "-"]
    if rng:
        args += ["-r", rng]
    if head:
        args += ["-I"]
    for attempt in range(TRIES):
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
        if not transient(code) or attempt + 1 == TRIES:
            return code, body, headers
        time.sleep(WAIT_S * (attempt + 1))
    return 0, b"", {}


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
    """A catalogue feed's rail routes, from its routes.txt alone."""
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


RAIL_TYPES = {0, 1, 2, 5, 7, 12} | set(range(100, 200)) | set(range(400, 500)) | set(range(900, 1000)) | {1400}


def unanswered(c: dict) -> bool:
    """A check that got no answer (asked again next time)."""
    s = c.get("status", "")
    return s.startswith("http ") and transient(int(s.split()[1]))


def catalogue_feeds(path: str, cover, countries: set[str]) -> list[dict]:
    """The catalogue's active GTFS feeds of `countries` whose box meets the coverage, in its order."""
    out = []
    with open(path, encoding="utf-8") as f:
        for row in csv.DictReader(f):
            if row["data_type"] != "gtfs" or row["location.country_code"] not in countries or row["status"] in ("deprecated", "inactive"):
                continue
            if row["redirect.id"]:
                continue
            try:
                bb = box(float(row["location.bounding_box.minimum_longitude"]), float(row["location.bounding_box.minimum_latitude"]),
                         float(row["location.bounding_box.maximum_longitude"]), float(row["location.bounding_box.maximum_latitude"]))
            except ValueError:
                continue
            if not cover.intersects(bb):
                continue
            url = row["urls.latest"] or (row["urls.direct_download"] if row["urls.authentication_type"] in ("", "0") else "")
            if not url:
                continue
            out.append({"id": row["id"], "provider": row["provider"], "name": row["name"], "country": row["location.country_code"],
                        "subdivision": row["location.subdivision_name"], "url": url, "licence": row["urls.license"]})
    return out


def keyed_link(feed: dict, key: str) -> str | None:
    """The download link of a keyed feed (LTA: a signed link, valid 15 minutes, in the answer)."""
    import ast
    import urllib.request

    if feed["get"] == "lta":
        req = urllib.request.Request(feed["url"], headers={"User-Agent": UA, "AccountKey": key, "accept": "application/json"})
        v = json.loads(urllib.request.urlopen(req, timeout=60).read())["value"]
        v = ast.literal_eval(v) if isinstance(v, str) else v
        return v[0]["link"] if v else None
    return None


def same_file(a: Path, b: Path) -> bool:
    if a.stat().st_size != b.stat().st_size:
        return False
    with a.open("rb") as fa, b.open("rb") as fb:
        while True:
            x, y = fa.read(1 << 20), fb.read(1 << 20)
            if x != y:
                return False
            if not x:
                return True


def fetch(feed: dict, dest: Path, key: str | None) -> tuple[bool, str]:
    """Downloads a feed's zip to `dest`: (whether it's there, why not). Raises on a request that got
    no answer after its tries."""
    import urllib.error

    if dest.exists() and zipfile.is_zipfile(dest):
        return True, "ok"
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".part")
    for attempt in range(TRIES):
        try:
            url = keyed_link(feed, key or "") if feed.get("get") else feed["url"]
        except urllib.error.HTTPError as e:
            # (Errors name the API's URL and status, never the key, which travels in a header.)
            if not transient(e.code):
                return False, f"its download link: http {e.code}"
            if attempt + 1 == TRIES:
                raise RuntimeError(f"{feed['id']}: no answer for its download link (http {e.code})") from None
            time.sleep(WAIT_S * (attempt + 1))
            continue
        except OSError as e:
            if attempt + 1 == TRIES:
                raise RuntimeError(f"{feed['id']}: no answer for its download link ({e.__class__.__name__})") from None
            time.sleep(WAIT_S * (attempt + 1))
            continue
        except (ValueError, KeyError, IndexError, TypeError, SyntaxError):
            return False, "its download link: an answer it can't read"
        if not url:
            return False, "no download link"
        r = subprocess.run(["curl", "-sSL", "-m", "3600", "-A", UA, "-o", str(tmp), "-w", "%{http_code}", url], capture_output=True, text=True)
        code = int(r.stdout.strip() or 0) if r.stdout.strip().isdigit() else 0
        if r.returncode == 0 and code == 200 and zipfile.is_zipfile(tmp):
            tmp.rename(dest)
            return True, "ok"
        tmp.unlink(missing_ok=True)
        if not transient(code if r.returncode == 0 else 0):
            return False, f"download failed: http {code}" if code != 200 else "download failed: not a zip"
        if attempt + 1 == TRIES:
            raise RuntimeError(f"{feed['id']}: no answer from its server (http {code}, curl {r.returncode})")
        time.sleep(WAIT_S * (attempt + 1))
    return False, "download failed"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--catalogue", required=True)
    ap.add_argument("--coverage", required=True, help="the coverage's outlines (GeoJSON, from the job)")
    ap.add_argument("--countries", required=True, help="ISO 3166-1 codes, comma-separated")
    ap.add_argument("--checked", required=True, help="the catalogue feeds checked so far")
    ap.add_argument("--cache", required=True, help="the NAS's zips: {id: {path, fetched}}")
    ap.add_argument("--keys", help="KEY=value lines (inputs/keys.env)")
    ap.add_argument("--out", required=True, help="a folder: feeds.json, checked.json, gtfs/")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    cover = coverage(a.coverage)
    countries = {c for c in a.countries.split(",") if c}
    have = keys(a.keys)
    cache = json.loads(Path(a.cache).read_text())
    checked = {c["id"]: c for c in json.loads(Path(a.checked).read_text())} if Path(a.checked).exists() else {}

    # The catalogue's feeds here, each checked once.
    cands = catalogue_feeds(a.catalogue, cover, countries)
    todo = [dict(c) for c in cands if c["id"] not in checked or unanswered(checked[c["id"]])]
    print(f"{len(cands)} catalogue feeds where the coverage is ({', '.join(sorted(countries))}); {len(todo)} to check", file=sys.stderr, flush=True)
    done = []
    with ThreadPoolExecutor(8) as ex:
        for k, c in enumerate(ex.map(check, todo)):
            progress(k + 1, len(todo), "feeds checked")
            done.append(c)
    for c in done:
        checked[c["id"]] = c
    (out / "checked.json").write_text(json.dumps(sorted(checked.values(), key=lambda c: c["id"]), ensure_ascii=False, indent=1))
    no_answer = [c["id"] for c in done if unanswered(c)]

    def here(f: dict) -> bool:
        return f["country"] in countries and cover.intersects(box(*f["bbox"]))

    rail = sorted((checked[c["id"]] for c in cands if checked[c["id"]].get("rail_routes")), key=lambda f: (f["country"], f["provider"]))
    order = [dict(f) for f in EXTRA if here(f)] + [dict(f) for f in KEYED if here(f) and have.get(f["key"])] + [dict(f) for f in rail]

    # Each feed's zip: the NAS's, else fetched; a stale one again once it's a week old.
    replaced = {r: f["id"] for f in order for r in f.get("replaces", ())}
    feeds, failed = [], []
    for k, f in enumerate(order):
        progress(k, len(order), "feeds' zips")
        rec = {key: f[key] for key in ("id", "provider", "name", "country", "url", "licence", "replaces", "rail_routes") if f.get(key)}
        if f["id"] in replaced or f["id"] in ELSEWHERE:
            feeds.append({**rec, "status": f"replaced by {replaced.get(f['id']) or ELSEWHERE[f['id']]}"})
            continue
        c = cache.get(f["id"])
        key = have.get(f["key"]) if f.get("key") else None
        if c:
            fetched = date.fromisoformat(c["fetched"])
            try:
                fresh = railgtfs.has_service(Path(c["path"]), fetched)
            except (zipfile.BadZipFile, KeyError, csv.Error, OSError):
                fresh = False
            if fresh or (today() - fetched).days < REFETCH_DAYS:
                feeds.append({**rec, "zip": "cache", "fetched": c["fetched"]})
                continue
            dest = out / "gtfs" / f"{f['id']}.zip"
            try:
                ok, why = fetch(f, dest, key)
            except RuntimeError as e:
                ok, why = False, str(e)
            if ok and not same_file(dest, Path(c["path"])):
                print(f"{f['id']}: out of date in the cache; fetched again", file=sys.stderr, flush=True)
                feeds.append({**rec, "zip": "new", "fetched": today().isoformat()})
            else:
                dest.unlink(missing_ok=True)
                print(f"{f['id']}: out of date in the cache, and {'the same' if ok else why} upstream; the cached copy stays", file=sys.stderr, flush=True)
                feeds.append({**rec, "zip": "cache", "fetched": c["fetched"]})
            continue
        try:
            ok, why = fetch(f, out / "gtfs" / f"{f['id']}.zip", key)
        except RuntimeError as e:
            failed.append(str(e))
            continue
        if ok:
            print(f"{f['id']}: fetched", file=sys.stderr, flush=True)
            feeds.append({**rec, "zip": "new", "fetched": today().isoformat()})
        else:
            print(f"{f['id']}: left out ({why})", file=sys.stderr, flush=True)
            feeds.append({**rec, "status": why})
    progress(len(order), len(order), "feeds' zips")
    n_new = sum(1 for f in feeds if f.get("zip") == "new")
    print(f"{len(order)} feeds: {len(order) - n_new - len(failed)} from the cache or left out, {n_new} fetched", file=sys.stderr, flush=True)
    if no_answer or failed:
        # What worked is written (checked.json, gtfs/) for the job to keep; the list isn't.
        for m in failed + [f"{i}: no answer to its check" for i in no_answer]:
            print(m, file=sys.stderr)
        sys.exit(3)
    (out / "feeds.json").write_text(json.dumps({"feeds": feeds}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
