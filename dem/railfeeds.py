#!/usr/bin/env python3
"""The GTFS feeds that run passenger rail (trains, metros, trams, funiculars) where the coverage
is, each fetched once. The `rail-feeds` job runs it (docs/plan.md §6, Rail service); railgtfs.py
reads what it finds.

The feeds:
  - the Mobility Database catalogue's (--catalogue: feeds_v2.csv, files.mobilitydatabase.org, as
    downloaded once): active GTFS feeds of the countries the coverage is in (--countries, ISO
    3166-1, which the job works out from the pass's outlines; COUNTRY corrects the catalogue's
    country of a few) whose bounding box meets the coverage (--coverage), with a download that needs
    no key. Of those, the ones with rail routes: only routes.txt is read, straight out of the zip by
    HTTP range requests (the catalogue's mirror, else the operator's URL), once per feed (--checked
    keeps every feed's answer; one that got no answer is asked again next time);
  - national operators the catalogue lacks (EXTRA), and feeds behind an API key (KEYED) whose key
    --keys holds (the NAS's inputs/keys.env, KEY=value lines: a key is read only to fetch its feed,
    and never written anywhere), each where its box meets the coverage, in the same countries.
A feed another `replaces` (an operator's own feed over an aggregate republishing it, and REPLACES
among the catalogue's copies of one timetable) is left out while that one has a zip (or is tried
again), and read when it has none. A feed whose trains come from elsewhere is left out (ELSEWHERE:
the MTR's lines, researched by hand).

Each feed is downloaded once: --cache lists the zips the NAS already has (feed id → its path and
the day it counts from), and only a feed not there is fetched, into --out/gtfs/<id>.zip (a zip left
there by a run cut short is used as it is). A cached zip without rail service in its window (see
railgtfs.py: it was out of date when fetched) is fetched again when this runs REFETCH_DAYS or more
after its day: the new zip replaces it, counting from today (the NAS keeps one copy if it's the
same file, which then isn't fetched again for another REFETCH_DAYS); a refusal or no answer leaves
the old one, asked again the next time.

A request that gets no answer (no connection, a 429 or a 5xx, an answer cut short) is tried three
times. A feed still without one (or whose file changed while it was read) fails the run, after the
checks and downloads that worked are written, so the job tries again later; once its server has
given none for NO_ANSWER_DAYS (--out/unanswered.json keeps the first day from run to run, until the
job completes and removes --out), it's left out instead, its status naming the first and the last
day, and it's tried again the next time rail-feeds runs. A definite refusal (a 404, a file that
isn't a zip) leaves that feed out. A cached zip the NAS can't read now fails the run.

Writes --out/feeds.json, {"feeds": […]}: the feeds in the order railgtfs.py reads them (national
operators first, so a train they share with a regional aggregate counts as theirs; the catalogue's by
country and provider), each with its facts, `zip` ("cache" for the NAS's, "new" for a download,
none when left out, with `status` saying why) and `fetched`; and --out/checked.json (--checked with
this run's answers). Each is written by a temporary name, then renamed.

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
from datetime import date, datetime, timezone
from pathlib import Path

from shapely.geometry import Polygon, box
from shapely.ops import unary_union

import railgtfs

UA = "scenic-roads/0.1 (personal offline map)"
# A stale cached zip is fetched again when this runs this many days or more after its day.
REFETCH_DAYS = 7
# Tries of a request that gets no answer, and the waits between them.
TRIES = 3
WAIT_S = 20
# Days a feed's server may give no answer before the feed is left out, rather than failing the run.
NO_ANSWER_DAYS = 3

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
# obtained. (The Mobility Database's Singapore feeds have the same trains, mostly at other times.)
KEYED = [
    {"id": "sg-lta-train", "provider": "Land Transport Authority, LTA DataMall (MRT and LRT)", "licence": "Singapore Open Data Licence 1.0",
     "url": "https://datamall2.mytransport.sg/ltaodataservice/GTFSScheduleTrain", "key": "LTA_ACCOUNT_KEY", "get": "lta", "replaces": ["mdb-1076", "mdb-3051", "mdb-3409"],
     "country": "SG", "bbox": [103.5, 1.1, 104.2, 1.5]},
]

# Catalogue feeds filed under another country than the one their trains run in (by their boxes):
# two of Singapore's, under Malaysia.
COUNTRY = {"mdb-3051": "SG", "mdb-3409": "SG"}

# Catalogue feeds that are copies of one timetable (Singapore's, from LTA's data, each with trips of
# its own, which needn't match another's): each in a list is left out while the feed listing it has
# a zip, so one copy is read.
REPLACES = {"mdb-1076": ["mdb-3051", "mdb-3409"], "mdb-3051": ["mdb-3409"]}


class NoAnswer(Exception):
    """A request still without an answer after its tries: asked again later."""


class Refused(Exception):
    """A definite answer that a feed has nothing to give: its status."""


def today() -> date:
    return datetime.now(timezone.utc).date()


def progress(done: int, total: int, unit: str) -> None:
    print(f"progress: {min(done, total)}/{total} {unit}", file=sys.stderr, flush=True)


def write_json(path: Path, v) -> None:
    """Writes `path` by a temporary name, then renames it: a run cut short leaves the old file or
    the new one, never part of one."""
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(v, ensure_ascii=False, indent=1))
    tmp.replace(path)


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
    """An HTTP status that's worth asking again for: none (no answer), too many requests, 5xx."""
    return code == 0 or code == 429 or code >= 500


def length(headers: dict) -> int | None:
    try:
        return int(headers["content-length"])
    except (KeyError, ValueError):
        return None


def answer(out: bytes) -> tuple[int, bytes, dict]:
    """curl's `-D - -o -` output: the last header block's status and headers, and the body (status
0 when the headers were cut short)."""
    code, headers, body = 0, {}, out
    while body.startswith(b"HTTP/"):
        end = body.find(b"\r\n\r\n")
        if end < 0:
            return 0, b"", {}
        block = body[:end].decode("latin-1").split("\r\n")
        try:
            code = int(block[0].split()[1])
        except (IndexError, ValueError):
            code = 0
        headers = {k.strip().lower(): v.strip() for k, _, v in (l.partition(":") for l in block[1:])}
        body = body[end + 4:]
    return code, body, headers


def curl(url: str, rng: str | None = None, head: bool = False) -> tuple[int, bytes, dict]:
    """A request's HTTP status, body and (last) headers, tried up to TRIES times while it gets no
    answer. The status is 0 for none: curl failed (no connection, a timeout, an answer cut short) or
    the body isn't as long as its Content-Length. A range request answered with more than the range
    (a server ignoring ranges sends the whole file) stops before the body: its status, no body."""
    args = ["curl", "-sS", "-L", "-m", "90", "-A", UA, "-D", "-", "-o", "-"]
    if rng:
        a, b = (int(x) for x in rng.split("-"))
        args += ["-r", rng, "--max-filesize", str(b - a + 1)]
    if head:
        args += ["-I"]
    code, body, headers = 0, b"", {}
    for attempt in range(TRIES):
        r = subprocess.run(args + [url], capture_output=True)
        code, body, headers = answer(r.stdout)
        if r.returncode == 63 and rng:
            body = b""  # (more than the range: the status stands)
        elif r.returncode != 0 or (not head and length(headers) not in (None, len(body))):
            code = 0
        if not transient(code):
            break
        if attempt + 1 < TRIES:
            time.sleep(WAIT_S * (attempt + 1))
    return code, body, headers


def no_answer(code: int) -> NoAnswer:
    return NoAnswer(f"http {code}" if code else "none")


def ranged(url: str, start: int, end: int, size: int) -> bytes:
    """Bytes start..end (inclusive) of a remote file of `size` bytes, by a range request."""
    code, body, h = curl(url, f"{start}-{end}")
    if transient(code):
        raise no_answer(code)
    if code == 200:
        raise Refused("no range requests")
    if code != 206:
        raise Refused(f"http {code}")
    if h.get("content-range", "*").rpartition("/")[2] not in (str(size), "*"):
        raise NoAnswer("the file changed while it was read")
    if len(body) != end - start + 1:
        raise NoAnswer("an answer cut short")
    return body


def zip_member(url: str, name: str, size: int) -> bytes:
    """A member of a remote zip of `size` bytes, by range requests (its central directory, then the
    member). Raises NoAnswer, or Refused with why there's none."""
    start = size - min(size, 1 << 20)
    code, tail, h = curl(url, f"{start}-{size - 1}")
    if code == 200 and len(tail) == size:
        # The server ignores ranges, and the whole file came (it's no bigger than the range).
        try:
            with zipfile.ZipFile(io.BytesIO(tail)) as z:
                names = {Path(n).name: n for n in z.namelist()}
                if name not in names:
                    raise Refused(f"no {name}")
                return z.read(names[name])
        except (zipfile.BadZipFile, zlib.error, EOFError):
            raise Refused("not a zip") from None
        except NotImplementedError:
            raise Refused("an unknown compression") from None
    if transient(code):
        raise no_answer(code)
    if code == 200:
        raise Refused("no range requests")
    if code != 206:
        raise Refused(f"http {code}")
    if h.get("content-range", "*").rpartition("/")[2] not in (str(size), "*"):
        raise NoAnswer("the file changed while it was read")
    if len(tail) != size - start:
        raise NoAnswer("an answer cut short")
    i = tail.rfind(b"PK\x05\x06")
    if i < 0 or len(tail) - i < 22:
        raise Refused("not a zip")
    cd_size, cd_off = struct.unpack("<II", tail[i + 12:i + 20])
    if 0xFFFFFFFF in (cd_size, cd_off):
        j = tail.rfind(b"PK\x06\x06", 0, i)
        if j < 0 or len(tail) - j < 56:
            raise Refused("not a zip")
        cd_size, cd_off = struct.unpack("<QQ", tail[j + 40:j + 56])
    if cd_off + cd_size > size:
        raise Refused("not a zip")
    if cd_off >= start:
        cd = tail[cd_off - start:cd_off - start + cd_size]
    else:
        cd = ranged(url, cd_off, cd_off + cd_size - 1, size) if cd_size else b""
    p = 0
    while p + 46 <= len(cd) and cd[p:p + 4] == b"PK\x01\x02":
        method, = struct.unpack("<H", cd[p + 10:p + 12])
        crc, csz, usz = struct.unpack("<III", cd[p + 16:p + 28])
        nl, el, cl = struct.unpack("<HHH", cd[p + 28:p + 34])
        off, = struct.unpack("<I", cd[p + 42:p + 46])
        fname = cd[p + 46:p + 46 + nl].decode("utf-8", "replace")
        extra = cd[p + 46 + nl:p + 46 + nl + el]
        q = 0
        while q + 4 <= len(extra):
            hid, hl = struct.unpack("<HH", extra[q:q + 4])
            if hid == 1:
                n = min(hl, len(extra) - q - 4) // 8
                vals = iter(struct.unpack(f"<{n}Q", extra[q + 4:q + 4 + 8 * n]))
                if usz == 0xFFFFFFFF:
                    usz = next(vals, usz)
                if csz == 0xFFFFFFFF:
                    csz = next(vals, csz)
                if off == 0xFFFFFFFF:
                    off = next(vals, off)
            q += 4 + hl
        if fname.split("/")[-1] == name:
            return member(url, size, method, crc, csz, usz, off)
        p += 46 + nl + el + cl
    raise Refused(f"no {name}")


def member(url: str, size: int, method: int, crc: int, csz: int, usz: int, off: int) -> bytes:
    """A zip member's bytes, from its local header (at `off`) and the central directory's facts:
    exactly as many as the zip says, with its CRC, or NoAnswer."""
    if off + 30 > size:
        raise Refused("not a zip")
    lh = ranged(url, off, off + 29, size)
    if lh[:4] != b"PK\x03\x04":
        raise NoAnswer("its local header came back damaged")
    n2, e2 = struct.unpack("<HH", lh[26:30])
    data_start = off + 30 + n2 + e2
    if data_start + csz > size:
        raise Refused("not a zip")
    comp = ranged(url, data_start, data_start + csz - 1, size) if csz else b""
    if method == 0:
        data = comp
    elif method == 8:
        try:
            d = zlib.decompressobj(-15)
            data = d.decompress(comp) + d.flush()
        except zlib.error:
            raise NoAnswer("its data came back damaged") from None
    else:
        raise Refused(f"an unknown compression (method {method})")
    if len(data) != usz or zlib.crc32(data) != crc:
        raise NoAnswer("its data came back damaged")
    return data


def check(feed: dict) -> dict:
    """A catalogue feed's rail routes, from its routes.txt alone: `status` "ok" with `rail_routes`
    and `examples`, a definite answer why there are none, or "no answer (…)" (asked again next
    time). Never raises: something unforeseen counts as no answer."""
    try:
        code, _, h = curl(feed["url"], head=True)
        if transient(code):
            raise no_answer(code)
        if code != 200:
            raise Refused(f"http {code}")
        size = length(h) or 0
        feed["size_mb"] = round(size / 1e6, 1)
        if not size:
            raise Refused("no size given")
        routes = zip_member(feed["url"], "routes.txt", size)
        rail = []
        for r in csv.DictReader(io.StringIO(routes.decode("utf-8-sig", "replace"))):
            try:
                t = int(r.get("route_type", ""))
            except (TypeError, ValueError):
                continue
            if t in RAIL_TYPES:
                rail.append(f"{r.get('route_short_name') or r.get('route_long_name') or r.get('route_id')} ({t})")
        feed["rail_routes"] = len(rail)
        feed["examples"] = rail[:8]
        feed["status"] = "ok"
    except NoAnswer as e:
        feed["status"] = f"no answer ({e})"
    except Refused as e:
        feed["status"] = str(e)
    except csv.Error as e:
        feed["status"] = f"routes.txt unreadable ({e})"
    except Exception as e:
        feed["status"] = f"no answer ({e.__class__.__name__}: {e})"
    return feed


RAIL_TYPES = {0, 1, 2, 5, 7, 12} | set(range(100, 200)) | set(range(400, 500)) | set(range(900, 1000)) | {1400}


def unanswered(c: dict) -> bool:
    """A check that got no answer (asked again next time): "no answer (…)", or an earlier check's
    "http <code>" with a code worth asking again for."""
    s = c.get("status", "")
    if s.startswith("no answer"):
        return True
    w = s.split()
    return len(w) == 2 and w[0] == "http" and w[1].isdigit() and transient(int(w[1]))


def catalogue_feeds(path: str, cover, countries: set[str]) -> list[dict]:
    """The catalogue's active GTFS feeds of `countries` (as COUNTRY corrects them) whose box meets the
    coverage, in its order."""
    out = []
    with open(path, encoding="utf-8") as f:
        for row in csv.DictReader(f):
            country = COUNTRY.get(row["id"], row["location.country_code"])
            if row["data_type"] != "gtfs" or country not in countries or row["status"] in ("deprecated", "inactive"):
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
            out.append({"id": row["id"], "provider": row["provider"], "name": row["name"], "country": country,
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


def fetch(feed: dict, dest: Path, key: str | None) -> tuple[bool, str]:
    """Downloads a feed's zip to `dest`: (whether it's there, why not). Raises NoAnswer for a
    request that got none after its tries."""
    import urllib.error

    if dest.exists() and zipfile.is_zipfile(dest):
        return True, "ok"
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".part")
    for attempt in range(TRIES):
        last = attempt + 1 == TRIES
        try:
            url = keyed_link(feed, key or "") if feed.get("get") else feed["url"]
        except urllib.error.HTTPError as e:
            # (Errors name the API's status, never the key, which travels in a header.)
            if not transient(e.code):
                return False, f"its download link: http {e.code}"
            if last:
                raise NoAnswer(f"its download link: http {e.code}") from None
            time.sleep(WAIT_S * (attempt + 1))
            continue
        except OSError as e:
            if last:
                raise NoAnswer(f"its download link: {e.__class__.__name__}") from None
            time.sleep(WAIT_S * (attempt + 1))
            continue
        except (ValueError, KeyError, IndexError, TypeError, SyntaxError):
            return False, "its download link: an answer it can't read"
        if not url:
            return False, "no download link"
        r = subprocess.run(["curl", "-sSL", "-m", "3600", "-A", UA, "-o", str(tmp), "-w", "%{http_code}", url], capture_output=True, text=True)
        code = int(r.stdout.strip()) if r.stdout.strip().isdigit() else 0
        if r.returncode == 0 and code == 200 and zipfile.is_zipfile(tmp):
            tmp.rename(dest)
            return True, "ok"
        tmp.unlink(missing_ok=True)
        if r.returncode == 0 and not transient(code):
            return False, f"download failed: http {code}" if code != 200 else "download failed: not a zip"
        if last:
            raise NoAnswer(f"http {code}, curl exit {r.returncode}")
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
    ap.add_argument("--out", required=True, help="a folder: feeds.json, checked.json, gtfs/, unanswered.json")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    cover = coverage(a.coverage)
    countries = {c for c in a.countries.split(",") if c}
    have = keys(a.keys)
    cache = json.loads(Path(a.cache).read_text())
    checked = {c["id"]: c for c in json.loads(Path(a.checked).read_text())} if Path(a.checked).exists() else {}
    # The first day each feed's server gave no answer, kept from run to run until the job completes.
    silent_path = out / "unanswered.json"
    silent: dict[str, str] = json.loads(silent_path.read_text()) if silent_path.exists() else {}
    waiting: list[str] = []  # no answer yet: the run fails, and is tried again
    failed: list[str] = []  # other failures of the run

    def no_answer_since(i: str) -> str | None:
        """Feed i got no answer today: None while it's tried again (the run fails), else the first
        day it got none, NO_ANSWER_DAYS or more ago (it's left out)."""
        since = silent.setdefault(i, today().isoformat())
        return since if (today() - date.fromisoformat(since)).days >= NO_ANSWER_DAYS else None

    # The catalogue's feeds here, each checked once (again while one gets no answer).
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
    write_json(out / "checked.json", sorted(checked.values(), key=lambda c: c["id"]))
    left_out: dict[str, str] = {}  # a feed's status, when its check has had no answer for too long
    for c in done:
        if not unanswered(c):
            silent.pop(c["id"], None)
        elif since := no_answer_since(c["id"]):
            left_out[c["id"]] = f"its check: no answer since {since} (last tried {today()})"
        else:
            waiting.append(f"{c['id']}: its check got {c['status']}")

    def here(f: dict) -> bool:
        return f["country"] in countries and cover.intersects(box(*f["bbox"]))

    listed = sorted((checked[c["id"]] for c in cands if checked[c["id"]].get("rail_routes") or c["id"] in left_out), key=lambda f: (f["country"], f["provider"]))
    order = [dict(f) for f in EXTRA if here(f)] + [dict(f) for f in KEYED if here(f) and have.get(f["key"])] + [dict(f) for f in listed]
    for f in order:
        if f["id"] in REPLACES:
            f["replaces"] = REPLACES[f["id"]]
    by_id = {f["id"]: f for f in order}
    replacers: dict[str, list[str]] = {}
    for f in order:
        for r in f.get("replaces", ()):
            replacers.setdefault(r, []).append(f["id"])

    # Each feed's zip: the NAS's, else fetched; a stale one again once it's REFETCH_DAYS old. A feed
    # is placed as "zip" (it has one), "trying" (tried again later: the run fails) or "out".
    placed: dict[str, tuple[str, dict]] = {}

    def place(f: dict) -> tuple[str, dict]:
        i = f["id"]
        if i not in placed:
            # (Meanwhile, as one tried again: a cycle of replacements, or a failure, holds back the
            # feeds it replaces.)
            placed[i] = ("trying", {})
            try:
                placed[i] = settle(f)
            except Exception as e:
                # Unforeseen: this feed fails the run, the others go on. (A keyed feed's error is
                # named by its class alone, never its text.)
                failed.append(f"{i}: {e.__class__.__name__}" + ("" if f.get("key") else f": {e}"))
        return placed[i]

    def settle(f: dict) -> tuple[str, dict]:
        i = f["id"]
        rec = {key: f[key] for key in ("id", "provider", "name", "country", "url", "licence", "replaces", "rail_routes") if f.get(key)}
        # Left out while a feed replacing it has a zip, or is tried again (not fetched for nothing).
        for g in replacers.get(i, ()):
            if place(by_id[g])[0] != "out":
                return "out", {**rec, "status": f"replaced by {g}"}
        if i in ELSEWHERE:
            return "out", {**rec, "status": f"replaced by {ELSEWHERE[i]}"}
        if i in left_out:
            print(f"{i}: left out ({left_out[i]})", file=sys.stderr, flush=True)
            return "out", {**rec, "status": left_out[i]}
        key = have.get(f["key"]) if f.get("key") else None
        dest = out / "gtfs" / f"{i}.zip"
        c = cache.get(i)
        if c:
            cached = {**rec, "zip": "cache", "fetched": c["fetched"]}
            day = date.fromisoformat(c["fetched"])
            try:
                fresh = railgtfs.has_service(Path(c["path"]), day)
            except OSError as e:
                # Not read now (the NAS): the run fails, and is tried again, rather than fetching it.
                failed.append(f"{i}: its zip on the NAS can't be read now ({e.__class__.__name__}: {e})")
                return "trying", rec
            except (zipfile.BadZipFile, KeyError, csv.Error, zlib.error, EOFError) as e:
                # Its tables can't be read (railgtfs.py can't count it either): as one out of date.
                print(f"{i}: its zip can't be read ({e.__class__.__name__}: {e})", file=sys.stderr, flush=True)
                fresh = False
            if fresh or (today() - day).days < REFETCH_DAYS:
                return "zip", cached
            try:
                ok, why = fetch(f, dest, key)
            except NoAnswer as e:
                why = f"no answer ({e})"
                ok = False
            if not ok:
                print(f"{i}: out of date in the cache, and {why} upstream; the cached copy stays", file=sys.stderr, flush=True)
                return "zip", cached
            # (A file the same as the cached one is kept on the NAS once; it then counts from today.)
            print(f"{i}: out of date in the cache; fetched again", file=sys.stderr, flush=True)
            return "zip", {**rec, "zip": "new", "fetched": today().isoformat()}
        try:
            ok, why = fetch(f, dest, key)
        except NoAnswer as e:
            if since := no_answer_since(i):
                status = f"no answer since {since} (last tried {today()}): {e}"
                print(f"{i}: left out ({status})", file=sys.stderr, flush=True)
                return "out", {**rec, "status": status}
            waiting.append(f"{i}: no answer ({e})")
            return "trying", rec
        silent.pop(i, None)
        if ok:
            print(f"{i}: fetched", file=sys.stderr, flush=True)
            return "zip", {**rec, "zip": "new", "fetched": today().isoformat()}
        print(f"{i}: left out ({why})", file=sys.stderr, flush=True)
        return "out", {**rec, "status": why}

    feeds = []
    for k, f in enumerate(order):
        progress(k, len(order), "feeds' zips")
        state, rec = place(f)
        if state != "trying":
            feeds.append(rec)
    progress(len(order), len(order), "feeds' zips")
    write_json(silent_path, dict(sorted(silent.items())))
    n_new = sum(1 for f in feeds if f.get("zip") == "new")
    n_cache = sum(1 for f in feeds if f.get("zip") == "cache")
    print(f"{len(order)} feeds: {n_cache} from the cache, {n_new} fetched, {len(feeds) - n_cache - n_new} left out, {len(order) - len(feeds)} to try again", file=sys.stderr, flush=True)
    if waiting or failed:
        # What worked is written (checked.json, gtfs/, unanswered.json) for the job to keep; the list
        # isn't.
        for m in failed + waiting:
            print(m, file=sys.stderr)
        sys.exit(3)
    write_json(out / "feeds.json", {"feeds": feeds})


if __name__ == "__main__":
    main()
