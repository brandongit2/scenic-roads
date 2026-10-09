#!/usr/bin/env python3
"""Provincial and municipal historic places for New Brunswick, Prince Edward Island,
Newfoundland and Labrador, the western provinces and the territories from the Canadian Register of
Historic Places (historicplaces.ca, Parks Canada with the provinces). These publish no open
dataset with coordinates (Quebec, Ontario and Nova Scotia do: heritage.py reads those).

1. A map-bounds search per province (all results on one page) lists every place with its
   coordinates.
2. Each place page gives the recognition: jurisdiction, authority (province or municipality),
   statute, type and date. Pages are cached in <dir>/crhp/, so reruns are quick.

Terms (historicplaces.ca "Important notices"): non-commercial reproduction with credit to the
source; not presented as an official version. The register is no longer actively maintained.

Output: <dir>/crhp.json (GeoJSON points), level 4 provincial / 5 municipal, which heritage.py
reads; --dir is a registers' snapshot folder (docs/plan.md "Hand-made inputs").

usage: crhp.py --dir <registers folder>
"""
from __future__ import annotations

import html
import json
import re
import sys
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from tqdm import tqdm

H = Path()
CACHE = Path()
BASE = "https://www.historicplaces.ca/en/"
UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
PROVINCES = {
    "New Brunswick": ("CA-NB", (-69.1, 44.5, -63.7, 48.1)),
    "Prince Edward Island": ("CA-PE", (-64.5, 45.9, -61.9, 47.1)),
    "Newfoundland and Labrador": ("CA-NL", (-67.9, 46.5, -52.5, 60.5)),
    "Manitoba": ("CA-MB", (-102.1, 48.9, -88.9, 60.1)),
    "Saskatchewan": ("CA-SK", (-110.0, 49.0, -101.3, 60.0)),
    "Alberta": ("CA-AB", (-120.0, 49.0, -110.0, 60.0)),
    "British Columbia": ("CA-BC", (-139.1, 48.2, -114.0, 60.0)),
    "Yukon": ("CA-YT", (-141.0, 60.0, -123.8, 69.7)),
    "Northwest Territories": ("CA-NT", (-136.5, 60.0, -101.9, 78.8)),
    "Nunavut": ("CA-NU", (-120.7, 51.6, -61.0, 83.2)),
}
CREDIT = "Canadian Register of Historic Places (historicplaces.ca)"


def fetch(url: str, data: dict | None = None) -> str:
    body = urllib.parse.urlencode(data).encode() if data else None
    for attempt in range(6):
        try:
            req = urllib.request.Request(url, data=body, headers=UA)
            with urllib.request.urlopen(req, timeout=120) as r:
                return r.read().decode("utf-8", "ignore")
        except Exception as e:  # noqa: BLE001
            if attempt == 5:
                raise
            print(f"  retry {url[:80]} ({e})", file=sys.stderr)
            time.sleep(2 ** attempt)
    raise RuntimeError


def hidden_fields(t: str) -> dict:
    return {m.group(1): html.unescape(m.group(2)) for m in
            re.finditer(r'<input type="hidden" name="([^"]+)" id="[^"]*" value="([^"]*)"', t)}


def list_places(bbox) -> list[dict]:
    w, s, e, n = bbox
    url = f"{BASE}results-resultats.aspx?m=3&neLat={n}&neLng={e}&swLat={s}&swLng={w}"
    t = fetch(url)
    total = int(re.search(r'lblTotalRecordsFound">(\d+)<', t).group(1))
    # "Results per page: All" is an ASP.NET auto-postback on the dropdown.
    sel = re.search(r'<select name="([^"]*ddlResultsPerPage)"', t).group(1)
    form = hidden_fields(t)
    form.update({"__EVENTTARGET": sel, "__EVENTARGUMENT": "", sel: "65535"})
    t = fetch(url, form)
    raw = re.search(r"placeLayer = (\[.*?\]);", t, re.S).group(1)
    # Some names hold stray backslashes and raw control characters (not valid JSON).
    raw = re.sub(r'\\(["\\/bfnrt]|u[0-9a-fA-F]{4})|\\', lambda m: m.group(0) if m.group(1) else "\\\\", raw)
    places = json.loads(raw, strict=False)
    if len(places) < total:
        print(f"  warning: {len(places)} of {total} listed", file=sys.stderr)
    return places


FIELDS = ["Jurisdiction", "Recognition Authority", "Recognition Statute", "Recognition Type", "Recognition Date"]


def place(pid: str) -> dict:
    path = CACHE / f"{pid}.json"
    if path.exists():
        return json.loads(path.read_text())
    t = fetch(f"{BASE}rep-reg/place-lieu.aspx?id={pid}")
    txt = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", "\n", t))
    rec: dict = {"id": pid}
    i = txt.find(" Recognition ")
    seg = txt[i:i + 3000] if i >= 0 else ""
    keys = "|".join(FIELDS + ["Historical Information", "Significant Date", "Other Name", "Location of Supporting"])
    for f in FIELDS:
        m = re.search(re.escape(f) + r"\s+(.*?)\s+(?=" + keys + r"|$)", seg)
        if m:
            rec[f] = html.unescape(m.group(1).strip())
    for k in ("PlaceLatitude", "PlaceLongitude"):
        m = re.search(rf'<meta name="{k}" content="([-\d.]+)"', t)
        if m:
            rec[k] = float(m.group(1))
    CACHE.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(rec, ensure_ascii=False))
    time.sleep(0.3)
    return rec


def main():
    global H, CACHE
    if sys.argv[1:2] != ["--dir"] or len(sys.argv) != 3:
        sys.exit("usage: crhp.py --dir <registers folder>")
    H = Path(sys.argv[2])
    CACHE = H / "crhp"
    feats = []
    for prov, (iso, bbox) in PROVINCES.items():
        lst = [p for p in list_places(bbox) if p.get("location", "").endswith(prov)]
        print(f"{prov}: {len(lst)} places listed")
        with ThreadPoolExecutor(2) as ex:
            recs = list(tqdm(ex.map(lambda p: place(p["id"]), lst), total=len(lst), desc=f"CRHP {iso}", unit="place"))
        for p, r in zip(lst, recs):
            jur = r.get("Jurisdiction", "")
            if jur == "Federal" or not jur:
                continue  # federal designations come from the Parks Canada directory
            auth = r.get("Recognition Authority", "")
            municipal = bool(re.search(r"\b(city|town|village|municipal\w*|local government\w*|rural community|community of|ville|regional)\b", auth, re.I)) \
                and not re.search(r"province|government of|minister|lieutenant", auth, re.I)
            lat, lon = r.get("PlaceLatitude", p["posn"][0]), r.get("PlaceLongitude", p["posn"][1])
            feats.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(lon, 6), round(lat, 6)]},
                          "properties": {
                              "name": html.unescape(p["name"]), "level": 5 if municipal else 4,
                              "designation": r.get("Recognition Type") or ("Municipal heritage property" if municipal else "Provincial heritage property"),
                              "authority": auth, "statute": r.get("Recognition Statute", ""), "date": r.get("Recognition Date", ""),
                              "prov": iso, "url": f"{BASE}rep-reg/place-lieu.aspx?id={p['id']}", "source": CREDIT}})
    out = H / "crhp.json"
    out.with_suffix(".tmp").write_text(json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False))
    out.with_suffix(".tmp").replace(out)
    by = {}
    for f in feats:
        k = (f["properties"]["prov"], f["properties"]["level"])
        by[k] = by.get(k, 0) + 1
    print("crhp.json:", len(feats), dict(sorted(by.items())))


if __name__ == "__main__":
    main()
