#!/usr/bin/env python3
"""BlueTopo's datum offsets from NOAA's VDatum (its web API), denser than the tide stations: a CSV in
stations.py noaa's shape, one row a VDatum point (the centre of each `--cell`° cell holding a
BlueTopo tile's centre), with `msl_minus_navd88_m` (NAVD88 0 → LMSL, negated) and `msl_minus_mllw_m`
(LMSL 0 → MLLW). Regions: Puerto Rico and the Virgin Islands (`prvi`), Southeast Alaska (`seak`,
everything north of 50°), the Pacific coast (`westcoast`, west of 116° W), the rest `contiguous`.
VDatum answers its errors with HTTP 200 and `{"errorCode": 412, "message": …}`. `westcoast` wants the
target horizontal frame IGS14 for tidal datums (asked as `contiguous`, a Pacific point answers 412
"Uncaught error"). Points VDatum can't convert (land, outside its tidal models, Alaska beyond its
southeast) are left out, each logged with VDatum's reason; the datum surface (datums.py) spans them.

usage: vdatum.py --scheme <gpkg> --out <csv> [--cell 0.25] [--bbox <w,s,e,n>]
  --bbox: only the cells centred inside (to fill one region into a table of its own); their rows'
  ids name the cell (`vdatum-<cx>-<cy>`), so they can't clash with a whole run's `vdatum-<i>`
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
    if lon < -116:
        return {"region": "westcoast", "t_h_frame": "IGS14"}
    return {"region": "contiguous"}


def query(lon, lat, src, dst):
    q = {"s_x": f"{lon:.5f}", "s_y": f"{lat:.5f}", "s_z": "0", "s_h_frame": "NAD83_2011", "t_h_frame": "NAD83_2011", "s_v_frame": src, "t_v_frame": dst}
    q.update(region(lon, lat))
    if src != "NAVD88":  # (VDatum: tidal datums are on IGS08 in Alaska, IGS14 on the West Coast; NAVD88 on NAD 83)
        q["s_h_frame"] = {"seak": "IGS08", "westcoast": "IGS14"}.get(q["region"], q["s_h_frame"])
    return q


def parse(d):
    """VDatum's answer → (z, None) or (None, why)."""
    if not isinstance(d, dict):
        return None, "not a JSON object"
    if "errorCode" in d:
        return None, f"error {d['errorCode']}: {d.get('message', '')}"
    try:
        z = float(d.get("t_z"))
    except (TypeError, ValueError):
        return None, f"no t_z ({d.get('t_z')!r})"
    return (None, f"t_z {z} (no datum here)") if abs(z) > 100 else (z, None)


def convert(lon, lat, src, dst):
    """(z, None) or (None, why)."""
    url = API + "?" + urllib.parse.urlencode(query(lon, lat, src, dst))
    if not robots_allow(url):
        raise SystemExit(f"robots.txt disallows {url}")
    why = None
    for attempt in range(3):
        try:
            with urlopen(url, 60) as r:
                d = json.loads(r.read())
            break
        except (OSError, ValueError) as e:
            log(f"{lon},{lat}: {e}")
            why = str(e)
            time.sleep(5)
    else:
        return None, why
    time.sleep(1.0)
    return parse(d)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scheme", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cell", type=float, default=0.25)
    ap.add_argument("--bbox", help="w,s,e,n: only the cells centred inside")
    a = ap.parse_args()
    box = [float(v) for v in a.bbox.split(",")] if a.bbox else None
    con = sqlite3.connect(f"file:{a.scheme}?mode=ro", uri=True)
    (t,) = con.execute("select table_name from gpkg_contents where data_type = 'features'").fetchone()
    cells = sorted({(int((x0 + x1) / 2 // a.cell), int((y0 + y1) / 2 // a.cell)) for x0, x1, y0, y1 in con.execute(
        f'select r.minx, r.maxx, r.miny, r.maxy from "{t}" t join "rtree_{t}_geom" r on r.id = t.fid where t."GeoTIFF_Link" is not null')})
    if box:
        cells = [(cx, cy) for cx, cy in cells if box[0] <= (cx + 0.5) * a.cell <= box[2] and box[1] <= (cy + 0.5) * a.cell <= box[3]]
    log(f"{len(cells)} points")
    dropped = 0
    with open(a.out, "w", newline="") as f:
        w = csv.DictWriter(f, ["id", "name", "lon", "lat", "msl_minus_navd88_m", "msl_minus_mllw_m"])
        w.writeheader()
        for i, (cx, cy) in enumerate(cells):
            lon, lat = (cx + 0.5) * a.cell, (cy + 0.5) * a.cell
            navd, why_navd = convert(lon, lat, "NAVD88", "LMSL")
            mllw, why_mllw = convert(lon, lat, "LMSL", "MLLW")
            if navd is None or mllw is None:
                whys = "; ".join(f"{k} {v}" for k, v in (("NAVD88→LMSL", why_navd), ("LMSL→MLLW", why_mllw)) if v)
                log(f"{lon},{lat} ({region(lon, lat)['region']}) {'dropped' if navd is None and mllw is None else 'half'}: {whys}")
            if navd is None and mllw is None:
                dropped += 1
                continue
            w.writerow({"id": f"vdatum-{cx}-{cy}" if box else f"vdatum-{i}", "name": "VDatum", "lon": round(lon, 5), "lat": round(lat, 5),
                        "msl_minus_navd88_m": "" if navd is None else round(-navd, 3), "msl_minus_mllw_m": "" if mllw is None else round(mllw, 3)})
            f.flush()
            if i % 100 == 0:
                log(f"{i}/{len(cells)}")
    log(f"{len(cells) - dropped} rows written, {dropped} points dropped")


if __name__ == "__main__":
    main()
