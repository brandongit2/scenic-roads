#!/usr/bin/env python3
"""NCEI's grids the owner downloaded by hand (docs/inputs.md §6.10), kept in the store as they came:

- `crm`: the Coastal Relief Models (1″, sources/ncei-crm/crm_vol*.nc), heights up. Volumes 1–5, 7, 8
  are on EGM2008 and 9, 10 on local MSL (each file's `z#vert_crs_name`); EGM2008 is taken as MSL
  (their difference, the mean dynamic topography, isn't corrected). Land (≥ 0) left out.
- `greatlakes`: the five lakes' bathymetry (3″, sources/ncei-greatlakes/<lake>_lld.geotiff.tar.gz,
  metres relative to each lake's low water datum, IGLD 85), brought to the lake's long-term mean
  level (`lake_levels`); cells above the low water datum (land, and the shallowest rim) left out.

The lakes' levels are fetched, never typed in (into `--cache`, kept there):
- the low water datum, from NOAA CO-OPS' station datums (`GL_LWD`, m IGLD 85) of a gauge on each lake;
- the long-term mean, the average of NOAA GLERL's coordinated lake-wide monthly means (m IGLD 85,
  1918 on) over every complete year up to `--through` (default: the last complete year there).

usage: ncei.py crm --src <file> [--src …] --out <dir> --index <file> [--piece-zoom 7]
       ncei.py greatlakes --src <tif> [--src …] --cache <dir> [--through <year>] --out <dir> --index <file>
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from datums import Surface
from grid import Writer, fetch, log
from raster import Spec, produce

URL = {"crm": "https://www.ncei.noaa.gov/products/coastal-relief-model",
       "greatlakes": "https://www.ncei.noaa.gov/products/great-lakes-bathymetry"}
# Each lake (its grid file's first word): GLERL's lake-wide series, and a CO-OPS gauge on it.
LAKES = {"superior": ("superior", "9099018"), "michigan": ("miHuron", "9075014"), "huron": ("miHuron", "9075014"),
         "erie": ("erie", "9063053"), "ontario": ("ontario", "9052030")}
GLERL = "https://www.glerl.noaa.gov/data/wlevels/dashboard/data/lakewide/{}.txt"
COOPS = "https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi/stations/{}/datums.json?units=metric"


def low_water_datum(path: Path) -> float:
    d = json.loads(path.read_text())
    if d.get("units") != "meters":
        raise SystemExit(f"{path}: units {d.get('units')!r}, not meters")
    (v,) = [x["value"] for x in d["datums"] if x["name"] == "GL_LWD"]
    return float(v)


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
        lwd = low_water_datum(fetch(COOPS.format(gauge), cache / f"coops-{gauge}-datums.json"))
        out[lake] = mean - lwd
        log(f"{lake}: mean {mean:.3f} m ({y0}–{y1}), low water datum {lwd:.3f} m (gauge {gauge}): {out[lake]:+.3f} m")
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("which", choices=["crm", "greatlakes"])
    ap.add_argument("--src", action="append", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    ap.add_argument("--piece-zoom", type=int, default=7)
    ap.add_argument("--cache", type=Path)
    ap.add_argument("--through", type=int)
    a = ap.parse_args()
    if a.which == "greatlakes":
        if a.cache is None:
            raise SystemExit("greatlakes needs --cache (the lakes' levels are fetched there)")
        above = lake_levels(a.cache, a.through)
    w = Writer(a.out)
    for s in a.src:
        if a.which == "crm":
            spec = Spec(valid=(-12000.0, -0.001), piece_zoom=a.piece_zoom)
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
