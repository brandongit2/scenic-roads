#!/usr/bin/env python3
"""BlueTopo's datum offsets from NOAA's VDatum (its web API), denser than the tide stations: a CSV in
stations.py noaa's shape, one row a VDatum point (the centre of each `--cell`° cell holding a
BlueTopo tile's centre), with `msl_minus_navd88_m` (NAVD88 0 → LMSL, negated) and `msl_minus_mllw_m`
(LMSL 0 → MLLW). Regions: Puerto Rico and the Virgin Islands (`prvi`), Southeast Alaska (`seak`,
everything north of 50°), the rest `contiguous`. Points VDatum can't convert (land, outside its tidal
models, Alaska beyond its southeast) are left out; the datum surface (datums.py) spans them.

usage: vdatum.py --scheme <gpkg> --out <csv> [--cell 0.25]
"""
from __future__ import annotations

import argparse
import csv
import json
import sqlite3
import time
import urllib.parse

from grid import log, robots_allow, urlopen

API = "https://vdatum.noaa.gov/vdatumweb/api/convert"


def region(lon, lat):
    if lon > -68.5 and lat < 19.6:
        return {"region": "prvi"}
    if lat > 50:
        return {"region": "seak", "t_h_frame": "IGS08", "s_v_geoid": "geoid12b", "t_v_geoid": "geoid12b"}
    return {"region": "contiguous"}


def convert(lon, lat, src, dst):
    q = {"s_x": f"{lon:.5f}", "s_y": f"{lat:.5f}", "s_z": "0", "s_h_frame": "NAD83_2011", "t_h_frame": "NAD83_2011", "s_v_frame": src, "t_v_frame": dst}
    q.update(region(lon, lat))
    if q["region"] == "seak" and src != "NAVD88":
        q["s_h_frame"] = "IGS08"  # (VDatum: tidal datums in Alaska are on IGS08; NAVD88 on NAD 83)
    url = API + "?" + urllib.parse.urlencode(q)
    if not robots_allow(url):
        raise SystemExit(f"robots.txt disallows {url}")
    for attempt in range(3):
        try:
            with urlopen(url, 60) as r:
                d = json.loads(r.read())
            break
        except (OSError, ValueError) as e:
            log(f"{lon},{lat}: {e}")
            time.sleep(5)
    else:
        return None
    time.sleep(1.0)
    try:
        z = float(d.get("t_z"))
    except (TypeError, ValueError):
        return None
    return None if abs(z) > 100 else z


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scheme", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cell", type=float, default=0.25)
    a = ap.parse_args()
    con = sqlite3.connect(f"file:{a.scheme}?mode=ro", uri=True)
    (t,) = con.execute("select table_name from gpkg_contents where data_type = 'features'").fetchone()
    cells = sorted({(int((x0 + x1) / 2 // a.cell), int((y0 + y1) / 2 // a.cell)) for x0, x1, y0, y1 in con.execute(
        f'select r.minx, r.maxx, r.miny, r.maxy from "{t}" t join "rtree_{t}_geom" r on r.id = t.fid where t."GeoTIFF_Link" is not null')})
    log(f"{len(cells)} points")
    with open(a.out, "w", newline="") as f:
        w = csv.DictWriter(f, ["id", "name", "lon", "lat", "msl_minus_navd88_m", "msl_minus_mllw_m"])
        w.writeheader()
        for i, (cx, cy) in enumerate(cells):
            lon, lat = (cx + 0.5) * a.cell, (cy + 0.5) * a.cell
            navd = convert(lon, lat, "NAVD88", "LMSL")
            mllw = convert(lon, lat, "LMSL", "MLLW")
            if navd is None and mllw is None:
                continue
            w.writerow({"id": f"vdatum-{i}", "name": "VDatum", "lon": round(lon, 5), "lat": round(lat, 5),
                        "msl_minus_navd88_m": "" if navd is None else round(-navd, 3), "msl_minus_mllw_m": "" if mllw is None else round(mllw, 3)})
            f.flush()
            if i % 100 == 0:
                log(f"{i}/{len(cells)}")


if __name__ == "__main__":
    main()
