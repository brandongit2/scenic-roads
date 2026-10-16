#!/usr/bin/env python3
"""Officially designated places and areas, from the heritage registers (the input check's accepted
version of inputs/heritage/, docs/inputs.md §6.5), read the same way whatever the register: the
`heritage-sites` job (docs/phase5.md "Heritage and area flags").

Each register is a pair of files, <register>.geojson (its entries) and <register>.toml (its
description), given in --registers (a folder of them, the accepted copies, laid out by the job);
they're read in the order of their ids. A register's `class` says what its entries are:
- `heritage` and `world-heritage`: a point is a heritage site; a polygon a heritage area (its roads
  flagged HERITAGE), and a site too at its representative point when it has `dot`; a line a site
  at its representative point when it has `dot` (the default for a line, as for a point). Levels 1
  World Heritage · 2 national, highest grade · 3 national, other grades · 4 regional · 5 local.
- `special`: biosphere reserves, geoparks, dark-sky places: a point is matched to the OSM
  boundary named `osm_name` (else its name) nearby, else drawn as a circle of its `area_km2`; a
  polygon is its boundary.
Every site, area and special place carries its register's id (`register`), which the map resolves
to the register's credit from the catalog's credits, and a site its reference there (`ref`).

Two sites are one place (docs/inputs.md §6.5, the merging rule): when the owner's matches say so
(--matches, the accepted inputs/heritage-matches/, `same: true`), or when the lower-graded one is
local (level 5) and lies within 40 m of another register's site of the same or a higher level whose
name shares a word of three letters or more (of two local sites, the one of the register with fewer
entries gives way: a city's list to the provincial register), unless a match says they aren't
(`same: false`). The place kept is the highest-graded one, and lists the registers it was merged
from (`registers`).

Protected areas and Indigenous lands come from OSM (data/areas/areas.geojsonseq).

What is covered is --tiles' tiles (uint32 x, y pairs at zoom --zoom: those within 20 km of the
coverage): sites and special places outside them are left out. The areas' polygons, each with
its flag bit (roadcore::scenic::flag: PARK, HERITAGE, SPECIAL_AREA, INDIGENOUS), go to
area-shapes.geojsonseq for the units to rasterise onto their own grids (the `areaflags` program).
--date stamps heritage-sources.json with the pass's date.

usage: heritage.py <build_dir> --registers <dir> [--matches <dir>] --tiles <tiles.u32> --zoom <z> [--date YYYY-MM-DD]
"""
from __future__ import annotations

import json
import math
import os
import re
import sys
import tomllib
import unicodedata
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
from shapely.geometry import Point, mapping, shape
from shapely.ops import transform as shp_transform
from tqdm import tqdm

import inputs
from timings import phase

PARK, HERITAGE, SPECIAL, INDIGENOUS = 1 << 1, 1 << 4, 1 << 6, 1 << 7
# What an entry says about itself that isn't shown: taken here, not passed on.
OWN = ("id", "ref", "dot", "osm_name", "tier")


def registers(folder: Path) -> list[tuple[str, dict, list[dict]]]:
    """The registers in `folder`, in the order of their ids: (id, description, entries)."""
    out = []
    for t in sorted(folder.glob("*.toml")):
        g = t.with_suffix(".geojson")
        if not g.exists():
            sys.exit(f"{t.name} without its {g.name}")
        out.append((t.stem, tomllib.loads(t.read_text(encoding="utf-8")), json.loads(g.read_text(encoding="utf-8"))["features"]))
    return out


def matches(folder: Path | None) -> tuple[dict[str, str], set[frozenset[str]]]:
    """The owner's matches: each entry (`<register>:<id>`) to its group's first (the `same: true`
    pairs, transitively), and the pairs never merged (`same: false`)."""
    if folder is None:
        return {}, set()
    parent: dict[str, str] = {}

    def find(a: str) -> str:
        while parent.setdefault(a, a) != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a
    never: set[frozenset[str]] = set()
    for f in sorted(folder.glob("**/*.jsonl")):
        for line in f.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            m = json.loads(line)
            if m["same"]:
                a, b = find(m["a"]), find(m["b"])
                if a != b:
                    parent[max(a, b)] = min(a, b)
            else:
                never.add(frozenset((m["a"], m["b"])))
    return {k: find(k) for k in parent}, never


def name_words(s):
    s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode().lower()
    return set(re.findall(r"[a-z0-9]{3,}", s))


def merge(feats: list[dict], keys: list[str], sizes: dict[str, int], groups: dict[str, str], never: set[frozenset[str]], radius_m=40.0) -> list[dict]:
    """The sites made one place by the owner's matches and the merging rule (the module's doc):
    the places kept, each with the registers merged into it. `sizes`: each register's entries."""
    drop: dict[int, int] = {}
    # The owner's matches: each group's highest-graded site (the first read among equals) kept.
    by_group: dict[str, list[int]] = {}
    for i, k in enumerate(keys):
        if k in groups:
            by_group.setdefault(groups[k], []).append(i)
    for members in by_group.values():
        keep = min(members, key=lambda i: (feats[i]["properties"]["level"], i))
        for i in members:
            if i != keep:
                drop[i] = keep
    # The rule: a local site on another register's site of the same or a higher level, sharing a
    # name word.
    grid: dict[tuple, list[int]] = {}
    for i, f in enumerate(feats):
        lon, lat = f["geometry"]["coordinates"]
        grid.setdefault((round(lon, 3), round(lat, 3)), []).append(i)
    for i, f in enumerate(feats):
        p = f["properties"]
        if p["level"] < 5 or i in drop:
            continue
        lon, lat = f["geometry"]["coordinates"]
        for dx in (-0.001, 0, 0.001):
            for dy in (-0.001, 0, 0.001):
                for j in grid.get((round(lon + dx, 3), round(lat + dy, 3)), []):
                    if j == i or j in drop or i in drop:
                        continue
                    q = feats[j]["properties"]
                    if q["register"] == p["register"] or q["level"] > p["level"] or frozenset((keys[i], keys[j])) in never:
                        continue
                    # (Two local sites: the larger register's kept, whichever is read first.)
                    if q["level"] == p["level"] and (sizes[q["register"]], q["register"]) < (sizes[p["register"]], p["register"]):
                        continue
                    lo2, la2 = feats[j]["geometry"]["coordinates"]
                    d = math.hypot((lon - lo2) * 111320 * math.cos(math.radians(lat)), (lat - la2) * 110540)
                    if d <= radius_m and (name_words(p["name"]) & name_words(q["name"])):
                        drop[i] = j
    # Each dropped site's registers to the place it's part of.
    for i, j in drop.items():
        while j in drop:
            j = drop[j]
        q = feats[j]["properties"]
        regs = q.setdefault("registers", [q["register"]])
        if feats[i]["properties"]["register"] not in regs:
            regs.append(feats[i]["properties"]["register"])
    return [f for i, f in enumerate(feats) if i not in drop]


def write_json(path: Path, obj) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, ensure_ascii=False, separators=(",", ":")))
    os.replace(tmp, path)


def option(args: list[str], name: str) -> str | None:
    """Removes `name value` from args and returns the value (None when absent)."""
    if name not in args:
        return None
    i = args.index(name)
    v = args[i + 1]
    del args[i:i + 2]
    return v


PARK_TITLE = re.compile(r"park|forest|reserve|wilderness|wildlife|refuge|sanctuary|preserve|conservation area|natural area|recreation area|seashore|lakeshore", re.I)


def norm_name(s: str) -> str:
    s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode().lower()
    return " ".join(re.findall(r"[a-z0-9]+", s))


def main():
    args = [a for a in sys.argv[1:]]
    tiles_path, zoom, date = option(args, "--tiles"), int(option(args, "--zoom") or 11), option(args, "--date")
    reg_dir, match_dir = option(args, "--registers"), option(args, "--matches")
    if not tiles_path or not reg_dir or not args:
        sys.exit(__doc__.rsplit("usage: ", 1)[1])
    b = Path(args[0])
    areas_path = b.parent / "areas" / "areas.geojsonseq"
    with phase("the covered tiles read", "disk"):
        tiles = {tuple(t) for t in np.fromfile(inputs.arg(tiles_path), dtype=np.uint32).reshape(-1, 2).tolist()}
    n_tiles = 2 ** zoom

    def covered(lon, lat):
        if not (-85 < lat < 85 and -180 <= lon <= 180):
            return False  # a register's bad coordinates
        x = (lon + 180) / 360 * n_tiles
        y = (1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n_tiles
        return (int(x), int(y)) in tiles

    counts = {}
    sites, keys, harea, sp = [], [], [], []
    with phase("the registers read", "disk"):
        regs = registers(inputs.arg(reg_dir))
        groups, never = matches(inputs.arg(match_dir) if match_dir else None)
    described = {}
    for i, (rid, d, feats) in enumerate(regs):
        # (A line the build agent shows as this job's progress, the register under way in brackets.)
        print(f"progress: {i}/{len(regs)} registers ({rid})", file=sys.stderr, flush=True)
        klass = d.get("class", "heritage")
        described[rid] = {"class": klass, **{k: d[k] for k in ("wikidata_property", "osm_ref_tags") if k in d}}
        n_sites = n_areas = 0
        with phase("the registers read", "compute"):
            for f in feats:
                g, p = f["geometry"], dict(f["properties"])
                eid = p["id"]
                ref = p.get("ref", eid)
                dot = p.get("dot", g["type"] in ("Point", "MultiPoint", "LineString", "MultiLineString"))
                osm_name, tier = p.get("osm_name", ...), p.get("tier")
                p = {k: v for k, v in p.items() if k not in OWN}
                if klass == "special":
                    if g["type"] == "Point":
                        lon, lat = g["coordinates"][:2]
                        if covered(lon, lat):
                            sp.append({"lon": lon, "lat": lat, **p, **({"polygon_hint": osm_name} if osm_name is not ... else {}), "register": rid})
                    else:
                        c = shape(g).representative_point()
                        if covered(c.x, c.y):
                            sp.append({"lon": c.x, "lat": c.y, **p, "register": rid, "boundary": g})
                    continue
                if g["type"] in ("Polygon", "MultiPolygon"):
                    harea.append({"type": "Feature", "geometry": g, "properties": {**p, "register": rid, "ref": ref}})
                    n_areas += 1
                if not dot:
                    continue
                if g["type"] == "Point":
                    lon, lat = g["coordinates"][:2]
                else:
                    c = shape(g).representative_point()
                    lon, lat = round(c.x, 6), round(c.y, 6)
                if not covered(lon, lat):
                    continue
                sites.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [lon, lat]},
                              "properties": {**p, **({"t": tier} if tier else {}), "register": rid, "ref": ref}})
                keys.append(f"{rid}:{eid}")
                n_sites += 1
        counts[rid] = n_sites + n_areas
        print(f"heritage: {rid:24s} {n_sites:6d} sites, {n_areas:4d} areas")
    print(f"progress: {len(regs)}/{len(regs)} registers", file=sys.stderr, flush=True)
    with phase("the sites merged", "compute"):
        n0 = len(sites)
        sites = merge(sites, keys, {rid: len(feats) for rid, _, feats in regs}, groups, never)
    print(f"heritage: {n0 - len(sites)} sites merged into others")
    with phase("the sites and heritage areas written", "disk"):
        write_json(b / "heritage.json", {"type": "FeatureCollection", "features": sites})
        write_json(b / "heritage-areas.json", {"type": "FeatureCollection", "features": harea})
    by = {}
    for f in sites:
        by[f["properties"]["level"]] = by.get(f["properties"]["level"], 0) + 1
    print("heritage.json:", len(sites), "sites by level", dict(sorted(by.items())))

    with phase("the special places read", "compute"):
        # Entries by the OSM name to look for; several can share one (e.g. a biosphere reserve named
        # for the national park it surrounds), so each is matched on its own.
        hints: dict[str, list[int]] = {}
        for i, s in enumerate(sp):
            if "boundary" not in s:
                hints.setdefault(norm_name(s.get("polygon_hint") or s["name"]), []).append(i)

    print("protected areas & Indigenous lands (OSM)…")
    with phase("the protected areas and Indigenous lands read", "compute"):
        special_feats, indigenous_feats, matched = [], [], set()
        # The polygons in degrees, with their bits (rasterised by the units).
        shapes_ll: list[tuple] = []
        for line in tqdm(open(areas_path), desc="areas", unit="poly"):
            f = json.loads(line.strip("\x1e"))
            p = f["properties"]
            try:
                g = shape(f["geometry"])
            except Exception:  # noqa: BLE001
                continue
            if g.is_empty:
                continue
            name = p.get("name", "")
            ptitle = p.get("protection_title", "")
            for i in hints.get(norm_name(name), []):
                s = sp[i]
                # Use the mapped boundary only if it plausibly is the designated area: biosphere
                # reserves and geoparks are usually far larger than the park named in the hint.
                if i in matched or (s.get("area_km2") and g.area * 111.32 ** 2 * math.cos(math.radians(s["lat"])) < 0.5 * s["area_km2"]):
                    continue
                if g.distance(Point(s["lon"], s["lat"])) < 0.3:
                    matched.add(i)
                    special_feats.append({"type": "Feature", "geometry": mapping(g.simplify(0.0003)),
                                          "properties": {k: v for k, v in s.items() if k not in ("lon", "lat")}})
                    shapes_ll.append((g.simplify(0.0002, preserve_topology=True), SPECIAL))
            if p.get("boundary") == "aboriginal_lands":
                bit = INDIGENOUS
                indigenous_feats.append({"type": "Feature", "geometry": mapping(g.simplify(0.0003, preserve_topology=True)),
                                         "properties": {"name": name}})
            elif (p.get("boundary") == "national_park" or p.get("leisure") == "nature_reserve"
                  or p.get("protect_class") in ("1", "1a", "1b", "2", "3", "4", "5", "6") or PARK_TITLE.search(ptitle)):
                bit = PARK
            else:
                continue
            shapes_ll.append((g.simplify(0.0002, preserve_topology=True), bit))
    with phase("the remaining areas drawn", "compute"):
        for i, s in enumerate(sp):
            if i in matched:
                continue
            if "boundary" in s:
                g = shape(s["boundary"])
                special_feats.append({"type": "Feature", "geometry": mapping(g),
                                      "properties": {k: v for k, v in s.items() if k not in ("lon", "lat", "boundary")}})
                shapes_ll.append((g, SPECIAL))
                continue
            r_km = math.sqrt(s["area_km2"] / math.pi) if s.get("area_km2") else (15.0 if s["kind"] != "dark_sky" else 8.0)
            r_km = min(max(r_km, 3.0), 60.0)
            circ = Point(s["lon"], s["lat"]).buffer(r_km / 111.0, 48)
            circ = shp_transform(lambda x, y, lo=s["lon"], la=s["lat"]: (lo + (x - lo) / math.cos(math.radians(la)), y), circ)
            special_feats.append({"type": "Feature", "geometry": mapping(circ),
                                  "properties": {**{k: v for k, v in s.items() if k not in ("lon", "lat")}, "approx": True}})
            shapes_ll.append((circ, SPECIAL))
        print(f"special areas: {len(sp)} ({len(matched)} with OSM boundaries)")
        for f in harea:
            shapes_ll.append((shape(f["geometry"]), HERITAGE))
    with phase("the area layers written", "disk"):
        write_json(b / "special.json", {"type": "FeatureCollection", "features": special_feats})
        write_json(b / "indigenous.json", {"type": "FeatureCollection", "features": indigenous_feats})
        write_json(b / "heritage-sources.json", {"counts": counts, "special": len(sp), "registers": described,
                                                 "built": date or datetime.now(timezone.utc).isoformat()[:19]})
    with phase("the area shapes written", "disk"):
        tmp = b / "area-shapes.geojsonseq.tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            for g, bit in shapes_ll:
                f.write(json.dumps({"type": "Feature", "geometry": mapping(g), "properties": {"bit": int(bit)}}, separators=(",", ":")) + "\n")
        os.replace(tmp, b / "area-shapes.geojsonseq")
    print(f"area-shapes.geojsonseq: {len(shapes_ll)} polygons (the units rasterise them)")


if __name__ == "__main__":
    main()
