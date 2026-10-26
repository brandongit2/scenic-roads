#!/usr/bin/env python3
"""NCEI's grids the owner downloaded by hand (docs/inputs.md §6.10), kept in the store as they came:

- `crm`: the Coastal Relief Models (1″, sources/ncei-crm/crm_vol*.nc), heights up. Volumes 1–5, 7, 8
  are on EGM2008 and 9, 10 on local MSL (each file's `z#vert_crs_name`): an EGM2008 volume is brought
  to MSL by `--offset` (`egm2008`'s table, MSL above EGM2008, a datum surface over the volume's sea).
  Land (≥ 0) left out.
- `egm2008`: that table, from one of tide stations' MSL − NAVD88 (`stations.py noaa`, or `vdatum.py`'s):
  each row's NAVD88 → EGM2008 separation added by `scenic-build geoid-offsets` (the declared geoid
  grids, GEOID18 and EGM2008, docs/inputs.md's heights brought to EGM2008), as `msl_minus_egm2008_m`.
- `greatlakes`: the five lakes' bathymetry (3″, sources/ncei-greatlakes/<lake>_lld.geotiff.tar.gz,
  metres relative to each lake's low water datum, IGLD 85), brought to the lake's long-term mean
  level (`lake_levels`); cells above the low water datum (land, and the shallowest rim) left out.

The lakes' levels are fetched, never typed in (into `--cache`, kept there):
- the low water datum, from NOAA CO-OPS' station datums (`GL_LWD`, m IGLD 85) of a gauge on each lake;
- the long-term mean, the average of NOAA GLERL's coordinated lake-wide monthly means (m IGLD 85,
  1918 on) over every complete year up to `--through` (default: the last complete year there).

usage: ncei.py crm --src <file> [--src …] --offset <csv> --out <dir> --index <file> [--piece-zoom 7]
       ncei.py egm2008 --table <csv> --scenic-build <bin> --geoid <declarations dir> --store <sources/fetched> --out <csv>
       ncei.py greatlakes --src <tif> [--src …] --cache <dir> [--through <year>] --out <dir> --index <file>
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import subprocess
from pathlib import Path

from datums import Surface, from_table
from grid import Writer, fetch, log
from raster import Spec, produce

# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): NCEI's and GLERL's published files and CO-OPS's datum API.
FETCHES = "api, data"

URL = {"crm": "https://www.ncei.noaa.gov/products/coastal-relief-model",
       "greatlakes": "https://www.ncei.noaa.gov/products/great-lakes-bathymetry"}
# Each lake (its grid file's first word): GLERL's lake-wide series, and a CO-OPS gauge on it.
LAKES = {"superior": ("superior", "9099018"), "michigan": ("miHuron", "9075014"), "huron": ("miHuron", "9075014"),
         "erie": ("erie", "9063053"), "ontario": ("ontario", "9052030")}
GLERL = "https://www.glerl.noaa.gov/data/wlevels/dashboard/data/lakewide/{}.txt"
COOPS = "https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi/stations/{}/datums.json?units=metric"


def low_water_datum(d: dict, mean: float) -> float:
    """The gauge's low water datum in m IGLD 85, from its CO-OPS datums (`d`). The metadata API gives a
    Great Lakes gauge's GL_LWD as an IGLD 85 elevation, its only datum (no station datum, STND):
    with an `IGLD` entry beside it, both are taken as above STND and IGLD is subtracted; any other
    shape fails. The gauge's observed extremes (`min`, `max`, the same reference) must hold the
    lake's long-term mean (`mean`, m IGLD 85), so a wrong reference fails loudly."""
    if d.get("units") != "meters":
        raise SystemExit(f"CO-OPS datums: units {d.get('units')!r}, not meters")
    by = {x["name"]: float(x["value"]) for x in d.get("datums") or []}
    if "GL_LWD" not in by or set(by) - {"GL_LWD", "IGLD"}:
        raise SystemExit(f"CO-OPS datums: {sorted(by)}, not GL_LWD (and IGLD) alone")
    lwd = by["GL_LWD"] - by.get("IGLD", 0.0)
    lo, hi = d.get("min"), d.get("max")
    if lo is None or hi is None or not (float(lo) - by.get("IGLD", 0.0) <= mean <= float(hi) - by.get("IGLD", 0.0)) or not 0 < mean - lwd < 2:
        raise SystemExit(f"CO-OPS datums: low water datum {lwd} m and extremes {lo}–{hi} m don't fit the mean level {mean:.3f} m IGLD 85")
    return lwd


def mean_level(path: Path, through: int | None) -> tuple[float, int, int]:
    """The mean of the complete years' monthly means (m), and the years it spans."""
    vals, years = [], []
    for line in path.read_text().splitlines():
        p = line.split()
        if len(p) != 13 or not p[0].isdecimal() or "NA" in p or (through is not None and int(p[0]) > through):
            continue
        vals += [float(x) for x in p[1:]]
        years.append(int(p[0]))
    if not years or years != list(range(years[0], years[-1] + 1)):
        raise SystemExit(f"{path}: its complete years aren't one unbroken run")
    return sum(vals) / len(vals), years[0], years[-1]


def lake_levels(cache: Path, through: int | None) -> dict[str, float]:
    """Each lake's long-term mean level above its low water datum (m)."""
    out = {}
    for lake, (series, gauge) in LAKES.items():
        mean, y0, y1 = mean_level(fetch(GLERL.format(series), cache / f"glerl-{series}.txt"), through)
        lwd = low_water_datum(json.loads(fetch(COOPS.format(gauge), cache / f"coops-{gauge}-datums.json").read_text()), mean)
        out[lake] = mean - lwd
        log(f"{lake}: mean {mean:.3f} m ({y0}–{y1}), low water datum {lwd:.3f} m (gauge {gauge}): {out[lake]:+.3f} m")
    return out


# A volume's vertical reference (its `z#vert_crs_name`): EGM2008 brought to MSL, MSL as it is.
VERT = {"EGM2008": True, "Mean Sea Level": False}
COLUMN = "msl_minus_egm2008_m"
# The surface's cells (°): the offset changes by decimetres over tens of km; a volume's sea at 0.005° is millions of cells.
OFFSET_RES = 0.01


def on_egm2008(src: Path) -> bool:
    """Whether a volume's heights are above EGM2008 (else MSL); any other reference fails."""
    import rasterio

    with rasterio.open(src) as d:
        v = d.tags().get("z#vert_crs_name")
    if v not in VERT:
        raise SystemExit(f"{src}: vertical reference {v!r}, not one of {sorted(VERT)}")
    return VERT[v]


def separations(out: str) -> dict[tuple[float, float], float]:
    """`scenic-build geoid-offsets`' lines → {(lon, lat): NAVD88 → EGM2008 (m)}; a point it can't convert has no line of that shape."""
    got = {}
    for line in out.splitlines():
        m = re.match(r"\s*(-?[\d.]+),(-?[\d.]+)\s+NAVD88→EGM2008 ([-+]?[\d.]+) m", line)
        if m:
            got[(round(float(m[1]), 4), round(float(m[2]), 4))] = float(m[3])
    return got


def egm2008_rows(rows: list[dict], sep: dict[tuple[float, float], float]) -> list[dict]:
    """The rows with MSL − NAVD88 and a separation, as MSL above EGM2008 (MSL's NAVD88 height plus NAVD88's EGM2008 height)."""
    out = []
    for r in rows:
        k = (round(float(r["lon"]), 4), round(float(r["lat"]), 4))
        if (r.get("msl_minus_navd88_m") or "").strip() and k in sep:
            out.append({"id": r["id"], "name": r["name"], "lon": r["lon"], "lat": r["lat"], COLUMN: round(float(r["msl_minus_navd88_m"]) + sep[k], 3)})
    return out


def egm2008_table(a) -> None:
    with open(a.table, newline="") as f:
        rows = [r for r in csv.DictReader(f) if (r.get("msl_minus_navd88_m") or "").strip()]
    at = ";".join(f"{float(r['lon']):.4f},{float(r['lat']):.4f}" for r in rows)
    run = subprocess.run([a.scenic_build, "geoid-offsets", "--sources", a.geoid, "--from", "NAVD88", "--at", at, "--store", a.store],
                         capture_output=True, text=True, check=True)
    out = egm2008_rows(rows, separations(run.stdout))
    log(f"{len(out)} of {len(rows)} rows converted (the others outside the geoid grids)")
    with open(a.out, "w", newline="") as f:
        w = csv.DictWriter(f, ["id", "name", "lon", "lat", COLUMN])
        w.writeheader()
        w.writerows(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("which", choices=["crm", "egm2008", "greatlakes"])
    ap.add_argument("--src", action="append", type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", type=Path)
    ap.add_argument("--offset", help="crm: egm2008's table")
    ap.add_argument("--table", help="egm2008: a table with msl_minus_navd88_m")
    ap.add_argument("--scenic-build")
    ap.add_argument("--geoid", help="egm2008: the geoid declarations' folder (producers/geoid/declarations)")
    ap.add_argument("--store", help="egm2008: the store's fetched folder (sources/fetched)")
    ap.add_argument("--piece-zoom", type=int, default=7)
    ap.add_argument("--cache", type=Path)
    ap.add_argument("--through", type=int)
    a = ap.parse_args()
    if a.which == "egm2008":
        if not (a.table and a.scenic_build and a.geoid and a.store):
            raise SystemExit("egm2008 needs --table, --scenic-build, --geoid and --store")
        return egm2008_table(a)
    if not a.src or a.index is None:
        raise SystemExit(f"{a.which} needs --src and --index")
    if a.which == "greatlakes":
        if a.cache is None:
            raise SystemExit("greatlakes needs --cache (the lakes' levels are fetched there)")
        above = lake_levels(a.cache, a.through)
    w = Writer(a.out)
    for s in a.src:
        if a.which == "crm":
            offset = None
            if on_egm2008(s):
                if a.offset is None:
                    raise SystemExit(f"{s}: on EGM2008; crm needs --offset (egm2008's table)")
                # (Per volume, over its sea: its values under 0, land a barrier.)
                offset = from_table((a.offset, COLUMN, frozenset(), "id"), [s], OFFSET_RES, below=0.0)
            spec = Spec(valid=(-12000.0, -0.001), piece_zoom=a.piece_zoom, offset=offset)
            name = s.stem.split("_20")[0].replace("_", "-")
        else:
            lake = s.stem.split("_")[0]
            # (Heights above the low water datum, negative below; minus the mean's height above it:
            # below the mean level.)
            spec = Spec(valid=(-1000.0, 0.0), piece_zoom=a.piece_zoom, offset=Surface.constant(above[lake]))
            name = lake
        produce(s, spec, w, URL[a.which], name)
    w.write_index(a.index)


if __name__ == "__main__":
    main()
