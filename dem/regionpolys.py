#!/usr/bin/env python3
"""Outlines of the regions (regions.json): Geofabrik's .poly for each Geofabrik region, saved to
data/trees/poly/<id>.poly (fetched once). Regions taken from Overpass use their bbox instead
(leaftype.regions()).

usage: regionpolys.py
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POLY = ROOT / "data" / "trees" / "poly"
UA = "road-elevations/0.1 (personal offline map)"


def main():
    POLY.mkdir(parents=True, exist_ok=True)
    for r in json.loads((ROOT / "regions.json").read_text())["regions"]:
        if "geofabrik" not in r:
            continue
        p = POLY / f"{r['id']}.poly"
        if p.exists():
            continue
        tmp = p.with_suffix(".part")
        ok = subprocess.run(["curl", "-sSL", "--fail", "-A", UA, "-o", str(tmp), f"https://download.geofabrik.de/{r['geofabrik']}.poly"]).returncode == 0
        if ok:
            tmp.rename(p)
            print(f"{r['id']}: outline fetched", file=sys.stderr)
        else:
            tmp.unlink(missing_ok=True)
            print(f"{r['id']}: no outline", file=sys.stderr)


if __name__ == "__main__":
    main()
