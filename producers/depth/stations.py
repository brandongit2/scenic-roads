#!/usr/bin/env python3
"""Tide stations' datums for the depth producers' datum surfaces (datums.py): a CSV a station a row
with `id`, `name`, `lon`, `lat` and the differences the sources need, from the keyless station
services:

- `noaa` (US): NOAA CO-OPS's metadata API: per station MSL − NAVD88 (`msl_minus_navd88_m`), MSL −
  MLLW (`msl_minus_mllw_m`), MHW − MSL (`mhw_minus_msl_m`);
- `chs` (Canada): CHS's water level service (IWLS): per station the mean water level above chart
  datum (`mwl_above_cd_m`), and the lowest low water, large tide (`llwlt_above_cd_m`).

Japan's (JCG's Z0) and France's (SHOM's RAM) come as tables the owner keeps (their sites are pages
and archives, not services); they're read as they are.

usage: stations.py noaa|chs --box w,s,e,n --out <csv>
"""
from __future__ import annotations

import argparse
import csv
import json
import time

from grid import log, urlopen

NOAA = "https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi"
CHS = "https://api-iwls.dfo-mpo.gc.ca/api/v1"
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): NOAA's and CHS's station APIs.
FETCHES = "api"


def get(url: str):
    with urlopen(url, 120) as r:
        data = json.loads(r.read())
    time.sleep(0.5)
    return data


def inside(box, lon, lat):
    return box[0] <= lon <= box[2] and box[1] <= lat <= box[3]


def noaa(box):
    rows = []
    for s in get(f"{NOAA}/stations.json?type=tidepredictions&units=metric")["stations"]:
        lon, lat = float(s["lng"]), float(s["lat"])
        if not inside(box, lon, lat):
            continue
        try:
            d = {x["name"]: x["value"] for x in get(f"{NOAA}/stations/{s['id']}/datums.json?units=metric").get("datums") or [] if x.get("value") is not None}
        except OSError as e:
            log(f"{s['id']}: {e}")
            continue
        if "MSL" not in d:
            continue
        r = {"id": s["id"], "name": s["name"], "lon": lon, "lat": lat}
        for k, other, sign in (("msl_minus_navd88_m", "NAVD88", 1), ("msl_minus_mllw_m", "MLLW", 1), ("mhw_minus_msl_m", "MHW", -1)):
            if other in d:
                r[k] = round(sign * (d["MSL"] - d[other]), 3)
        rows.append(r)
    return rows


def chs(box):
    rows = []
    types = {t["id"]: t.get("code") for t in get(f"{CHS}/height-types")}
    for s in get(f"{CHS}/stations"):
        lon, lat = float(s["longitude"]), float(s["latitude"])
        if not inside(box, lon, lat):
            continue
        m = get(f"{CHS}/stations/{s['id']}/metadata")
        h = {types.get(x.get("heightTypeId")): x.get("value") for x in m.get("heights") or []}
        if h.get("MWL") is None:
            continue
        rows.append({"id": s.get("code"), "name": s.get("officialName"), "lon": lon, "lat": lat, "mwl_above_cd_m": h["MWL"], "llwlt_above_cd_m": h.get("LLWLT", "")})
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("service", choices=["noaa", "chs"])
    ap.add_argument("--box", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    box = [float(v) for v in a.box.split(",")]
    rows = noaa(box) if a.service == "noaa" else chs(box)
    cols = sorted({k for r in rows for k in r}, key=lambda k: ["id", "name", "lon", "lat"].index(k) if k in ("id", "name", "lon", "lat") else 9)
    with open(a.out, "w", newline="") as f:
        w = csv.DictWriter(f, cols)
        w.writeheader()
        w.writerows(rows)
    log(f"{len(rows)} stations")


if __name__ == "__main__":
    main()
