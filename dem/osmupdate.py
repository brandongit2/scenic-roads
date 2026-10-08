#!/usr/bin/env python3
"""OSM data for the regions in regions.json: data/osm/merged.osm.pbf, updated incrementally.

merged.regions lists the regions merged.osm.pbf holds. A region not in it is downloaded (its
Geofabrik extract, a Geofabrik extract cut to its boundary relation, or its Overpass area) and merged into merged.osm.pbf (each object once, at
its newest version). With --refresh every region is downloaded again (Geofabrik
only sends newer extracts) and merged.osm.pbf is rebuilt from them.

Extracts are deleted once merged unless the basemap still needs them for a part (a region added
after base.pmtiles, see the Makefile's basemap-parts) or --keep is given: the merged file has
everything, and the extracts of all regions are ~12 GB.

usage: osmupdate.py [--refresh] [--keep]
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OSM = ROOT / "data" / "osm"
BUILD = ROOT / "data" / "build"
MERGED = OSM / "merged.osm.pbf"
LIST = OSM / "merged.regions"
UA = "scenic-roads/0.1 (personal offline map)"


def run(*cmd: str) -> None:
    print("+", " ".join(cmd), file=sys.stderr, flush=True)
    subprocess.run(cmd, check=True)


def merge(files: list[Path]) -> None:
    """Merge sorted OSM files into merged.osm.pbf. An object in several of them at different
    versions (extracts of different dates, e.g. a region from Overpass next to older Geofabrik
    ones) is kept once, at its newest version (time-filter at the present); osmium merge alone
    would keep both, and later steps need each ID once."""
    tmp = MERGED.with_suffix(".tmp.pbf")
    print("+ osmium merge", *map(str, files), "| osmium time-filter", file=sys.stderr, flush=True)
    m = subprocess.Popen(["osmium", "merge", *map(str, files), "-f", "pbf", "-o", "-"], stdout=subprocess.PIPE)
    t = subprocess.run(["osmium", "time-filter", "-F", "pbf", "-", "-o", str(tmp), "--overwrite"], stdin=m.stdout)
    m.stdout.close()
    if m.wait() or t.returncode:
        raise SystemExit("osmium merge failed")
    tmp.rename(MERGED)


def fetch(r: dict) -> Path:
    """Download a region's extract (Geofabrik only sends a newer one than ours)."""
    out = OSM / f"{r['id']}.osm.pbf"
    if "clip_relation" in r:
        # A country Geofabrik only has with its neighbours (Singapore in malaysia-singapore-brunei;
        # too big for Overpass): the neighbours' extract, cut to the country's boundary relation.
        parent = OSM / f"{r['id']}.parent.osm.pbf"
        run("curl", "-sSL", "--fail", "-A", UA, "-o", str(parent), f"https://download.geofabrik.de/{r['geofabrik']}-latest.osm.pbf")
        # The boundary in full from the OSM API: its sea edges lie outside the neighbours' extract.
        rel = OSM / f"{r['id']}.boundary.osm"
        run("curl", "-sSL", "--fail", "-A", UA, "-o", str(rel), f"https://api.openstreetmap.org/api/0.6/relation/{r['clip_relation']}/full")
        run("osmium", "extract", "-p", str(rel), str(parent), "-o", str(out), "--overwrite")
        parent.unlink()
        rel.unlink()
        print(f"{r['id']}: cut from {r['geofabrik']}", file=sys.stderr)
    elif "geofabrik" in r:
        part = out.with_suffix(".part")
        z = ["-z", str(out)] if out.exists() else []
        run("curl", "-sSL", "--fail", "-A", UA, *z, "-o", str(part), f"https://download.geofabrik.de/{r['geofabrik']}-latest.osm.pbf")
        if part.exists() and part.stat().st_size > 0:
            part.rename(out)
            print(f"{r['id']}: downloaded", file=sys.stderr)
        else:
            part.unlink(missing_ok=True)
            print(f"{r['id']}: up to date", file=sys.stderr)
    else:
        raw = out.with_suffix(".osm")
        q = f"[out:xml][timeout:300];{r['overpass']}->.a;(node(area.a);way(area.a);rel(area.a););(._;>;);out meta;"
        run("curl", "-sS", "--fail", "-H", "Accept: */*", "-A", UA, "-o", str(raw), "--data-urlencode", f"data={q}", "https://overpass-api.de/api/interpreter")
        run("osmium", "sort", str(raw), "-o", str(out), "--overwrite")
        raw.unlink()
    return out


def main():
    refresh, keep = "--refresh" in sys.argv, "--keep" in sys.argv
    OSM.mkdir(parents=True, exist_ok=True)
    regions = json.loads((ROOT / "regions.json").read_text())["regions"]
    have = set(LIST.read_text().split()) if LIST.exists() and MERGED.exists() else set()
    base = set((BUILD / "base.regions").read_text().split()) if (BUILD / "base.regions").exists() else None
    ids = [r["id"] for r in regions]
    if refresh or not have:
        merge([fetch(r) for r in regions])
    else:
        new = [r for r in regions if r["id"] not in have]
        if not new:
            print("merged.osm.pbf: up to date", file=sys.stderr)
        else:
            merge([MERGED, *(fetch(r) for r in new)])
    LIST.write_text("\n".join(ids) + "\n")
    if not keep:
        for r in regions:
            f = OSM / f"{r['id']}.osm.pbf"
            # The basemap needs a region's own extract until base.pmtiles includes it.
            if f.exists() and base is not None and r["id"] in base:
                f.unlink()
                print(f"{r['id']}: extract removed (merged)", file=sys.stderr)


if __name__ == "__main__":
    main()
