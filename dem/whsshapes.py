#!/usr/bin/env python3
"""World Heritage Sites as their lines and areas, from OpenStreetMap, and one dot per site.

Outlines: a canal, a wall, a site boundary drawn through the view, where UNESCO's data
(heritage_eu.unesco) has only one point per component (the Rideau Canal's dots sit at its locks and
forts; the canal between them is not marked). A site's OSM objects are those tagged with the
Wikidata item of the site or of one of its components (P757, the World Heritage id, "430" or
"430-001"), or of an item that is part of one (P361: Hadrian's Wall, part of the Frontiers of the
Roman Empire), or with its World Heritage reference (ref:whc) or heritage:operator=whc; route,
waterway and site relations bring their member ways (a canal's waterway relation). Only lines and
areas (the points are the dots already), a closed way as an area unless it is a line by its tags
(a wall, a canal). Not the boundaries of settlements, administrative areas, natural regions or
regional and landscape parks that share an item (Wikidata puts some components on their town:
Sahagún on the Camino de Santiago), nor, matched by item alone, an area over three times the
site's inscribed area (the whole island of Ibiza for its Dalt Vila and seagrass meadows): these
aren't the site's extent, unless tagged as World Heritage themselves (heritage=1, ref:whc).

One dot per site: a site UNESCO lists in several components here (Hadrian's Wall in 193, the
Rideau Canal in 6) gets one dot at its centre, on the site: of the site's points (on its lines,
inside its areas clear of their edges, and its components with no outline near), the one with the
least total distance to all of the site (the cells it touches at 1/256 of its extent: lines, areas
and components alike). So a winding canal's dot is on the canal, and a site in scattered pieces
has it on the piece nearest the middle. The components are drawn small from zoom 10 (layers.py).
A site with one component keeps UNESCO's point.

Input: data/build/heritage.json (the sites in our regions: components, names), merged.osm.pbf,
UNESCO's list (heritage_eu, cached), Wikidata (cached).
Writes data/build/whs-shapes.json: lines and polygons with id (World Heritage id), n (the site's
name as the dots show it), c (Cultural, Natural or Mixed), a (1 for areas), simplified to ~15 m;
and data/build/whs-sites.json, per site id: q, its Wikidata items (its own and those of its
components here: its fame is the best known of them, interest.py), and for a site with several
components here, n (their number), dot [lon, lat] and lead [lon, lat] (the component nearest the
dot, whose record the dot shows).

usage: whsshapes.py
"""
from __future__ import annotations

import json
import math
import re
import subprocess
from collections import defaultdict
from pathlib import Path

import numpy as np
import shapely
from shapely.geometry import mapping, shape
from shapely.ops import polylabel

import heritage_eu
from timings import phase

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "whs"
OSM = ROOT / "data" / "osm" / "merged.osm.pbf"
SITES = B / "whs-sites.json"
# Closed ways that are lines, not areas, by their tags (a city wall around the old town).
LINEAR_KEYS = ("barrier", "highway", "railway", "power", "aerialway")
LINEAR_MAN_MADE = {"embankment", "dyke", "breakwater", "groyne", "cutting", "pipeline"}
AREA_WATERWAYS = {"riverbank", "dock", "boatyard"}
# Kinds of place that are settlements or administrative areas (not a square or an island).
SETTLEMENT = {"city", "town", "village", "hamlet", "isolated_dwelling", "farm", "municipality", "borough", "suburb", "quarter",
              "neighbourhood", "city_block", "civil_parish", "county", "region", "state", "province", "district", "country"}
TOO_LARGE = 3.0  # an area matched by item alone, over this many times the site's inscribed area
CELLS = 256  # the footprint's resolution for a site's centre: cells across its extent
MASS_MAX = 4000  # cells to measure distances to (a sample, for large areas)


def run(*cmd: str, ok: tuple[int, ...] = (0,)) -> None:
    print("+", " ".join(cmd)[:160], flush=True)
    r = subprocess.run(cmd)
    if r.returncode not in ok:
        raise SystemExit(f"{cmd[0]} {cmd[1]} failed ({r.returncode})")


def whs_id(p: dict) -> str | None:
    """The World Heritage id of a heritage.json site (UNESCO's), else None."""
    if p.get("level") != 1:
        return None
    m = re.search(r"whc\.unesco\.org/en/list/(\d+)", p.get("url") or "")
    return m.group(1) if m else None


def tagged_whs(tags: dict) -> bool:
    """Tagged as World Heritage itself (not only by a shared item)."""
    return tags.get("heritage") == "1" or bool(tags.get("ref:whc")) or tags.get("heritage:operator") == "whc"


def not_extent(tags: dict) -> bool:
    """A settlement, administrative or natural-region boundary, or a regional or landscape park
    (IUCN V–VI), not tagged as World Heritage itself: sharing a site's item, but not its extent."""
    return not tagged_whs(tags) and (tags.get("boundary") in ("administrative", "place", "political", "natural", "census", "statistical")
                                     or tags.get("place") in SETTLEMENT or tags.get("protect_class") in ("5", "6"))


def linear(tags: dict) -> bool:
    """A closed way that is a line (a wall, a canal loop), not an area."""
    if tags.get("area") == "yes":
        return False
    w = tags.get("waterway")
    return (any(k in tags for k in LINEAR_KEYS) or bool(w and w not in AREA_WATERWAYS)
            or tags.get("historic") in ("citywalls", "aqueduct") or tags.get("man_made") in LINEAR_MAN_MADE)


def our_sites() -> tuple[dict[str, str], dict[str, str], dict[str, list[tuple[float, float]]]]:
    """Our World Heritage Sites (heritage.json): id → the name the dots show, category, components."""
    names: dict[str, str] = {}
    cats: dict[str, str] = {}
    comps: dict[str, list[tuple[float, float]]] = defaultdict(list)
    for f in json.loads((B / "heritage.json").read_text())["features"]:
        p = f["properties"]
        sid = whs_id(p)
        if sid:
            names.setdefault(sid, p.get("name") or p.get("name_en") or "")
            cats.setdefault(sid, p.get("category") or "Cultural")
            comps[sid].append((round(f["geometry"]["coordinates"][0], 6), round(f["geometry"]["coordinates"][1], 6)))
    return names, cats, comps


def unesco_list() -> list[dict]:
    """UNESCO's World Heritage List (heritage_eu's cached copy)."""
    return json.loads(heritage_eu.fetch("https://data.unesco.org/api/explore/v2.1/catalog/datasets/whc001/exports/json",
                                        heritage_eu.H / "unesco" / "whc001.json").read_text())


def site_items(names: dict[str, str], comps: dict[str, list[tuple[float, float]]], d: list[dict]) -> dict[str, list[str]]:
    """Per site, its Wikidata items: its own (P757 "1221", "320bis") and its components' here
    (P757 "1221-001", by the UNESCO reference of the component at that point)."""
    plain = lambda ref: re.sub(r"(?<=\d)(bis|ter|quater|quinquies|rev)", "", ref.strip())  # noqa: E731 ("430ter-038" → "430-038")
    ours: dict[str, set[str]] = defaultdict(set)
    for s in d:
        sid = str(s.get("id_no"))
        if sid not in names:
            continue
        here = set(comps[sid])
        for _n, ref, la, lo in heritage_eu.COMPONENT.findall(s.get("components_list") or ""):
            if (round(float(lo), 6), round(float(la), 6)) in here:
                ours[sid].add(plain(ref))
    items: dict[str, list[str]] = defaultdict(list)
    for r in heritage_eu.wd_sparql("SELECT ?item ?id WHERE { ?item wdt:P757 ?id }", W / "wd-p757.json"):
        v = plain(r["id"])
        sid = v.split("-", 1)[0]
        q = r["item"].rsplit("/", 1)[-1]
        if sid in names and ("-" not in v or v in ours[sid]) and q not in items[sid]:
            items[sid].append(q)
    return items


def outlines(names: dict[str, str], km2: dict[str, float]) -> dict[str, list[tuple[shapely.Geometry, bool]]]:
    """Each site's OSM lines and areas (simplified), by site id: [(geometry, is_area)]; km2: the
    sites' inscribed areas, where UNESCO gives them."""
    # Its Wikidata items: the site and its components (P757), and items part of one (P361).
    site_of: dict[str, str] = {}
    with phase("the Wikidata items read", "disk"):
        for r in heritage_eu.wd_sparql("SELECT ?item ?id WHERE { ?item wdt:P757 ?id }", W / "wd-p757.json"):
            base = re.match(r"\d+", r["id"])
            if base and base.group() in names:
                site_of[r["item"].rsplit("/", 1)[-1]] = base.group()
        parts = heritage_eu.wd_sparql("SELECT ?part ?whole WHERE { ?whole wdt:P757 ?id . ?part wdt:P361 ?whole }", W / "wd-parts.json")
        for r in parts:
            whole, part = r["whole"].rsplit("/", 1)[-1], r["part"].rsplit("/", 1)[-1]
            if whole in site_of:
                site_of.setdefault(part, site_of[whole])
    print(f"{len(site_of)} Wikidata items")

    # Their OSM objects: everything tagged wikidata or ref:whc (tags only), picked by value, then
    # those objects with their geometry (a filter on thousands of values at once is slow).
    tagged = W / "tagged.osm.pbf"
    with phase("the tagged objects filtered by osmium", "compute"):
        run("osmium", "tags-filter", str(OSM), "nwr/wikidata", "nwr/ref:whc", "nwr/heritage:operator=whc", "-R", "-o", str(tagged), "--overwrite")
    with phase("the sites' objects picked", "compute"):
        opl = subprocess.Popen(["osmium", "cat", str(tagged), "-f", "opl", "-o", "-"], stdout=subprocess.PIPE, text=True, encoding="utf-8")
        want = []
        for line in opl.stdout:
            m = re.search(r"(?:^| T)(?:.*,)?wikidata=(Q\d+)", line)
            if (m and m.group(1) in site_of) or "ref:whc=" in line or "heritage:operator=whc" in line:
                want.append(line.split(" ", 1)[0])
        opl.wait()
        ids = W / "ids.txt"
        ids.write_text("\n".join(want) + "\n")
    print(f"{len(want)} OSM objects")
    pbf = W / "whs.osm.pbf"
    # (exit 1: some objects weren't found, e.g. a relation member outside our extracts)
    with phase("the sites' objects extracted by osmium", "compute"):
        run("osmium", "getid", "-r", str(OSM), "-i", str(ids), "-o", str(pbf), "--overwrite", ok=(0, 1))
    with phase("the sites' objects exported by osmium", "compute"):
        run("osmium", "export", str(pbf), "-f", "geojsonseq", "-a", "type,id", "--geometry-types=linestring,polygon", "-o", str(W / "whs.geojsonseq"), "--overwrite")
    with phase("the relations listed by osmium", "compute"):
        run("osmium", "cat", str(pbf), "-t", "relation", "-f", "opl", "-o", str(W / "relations.opl"), "--overwrite")

    def site(tags: dict) -> str | None:
        ref = re.match(r"\d+", tags.get("ref:whc") or tags.get("whc:ref") or "")
        if ref and ref.group() in names:
            return ref.group()
        return site_of.get(tags.get("wikidata", ""))

    # Route, waterway and site relations of a site pass it on to their member ways (not areas'
    # relations: their ways are the area's edges).
    with phase("the relations' members read", "compute"):
        member_site: dict[str, str] = {}
        for line in (W / "relations.opl").open(encoding="utf-8"):
            tm, mm = re.search(r" T(\S*)", line), re.search(r" M(\S*)", line)
            if not tm or not mm:
                continue
            tags = dict(kv.split("=", 1) for kv in tm.group(1).split(",") if "=" in kv)
            s = site(tags)
            if not s or tags.get("type") in ("multipolygon", "boundary") or not_extent(tags):
                continue
            for m in mm.group(1).split(","):
                k = m.split("@", 1)[0]
                if k.startswith("w"):
                    member_site.setdefault(k, s)

    # A closed way comes out twice, as a line and as an area: one of them, by its tags.
    with phase("the outlines picked and simplified", "compute"):
        found: dict[str, dict[bool, dict]] = defaultdict(dict)
        for line in (W / "whs.geojsonseq").open(encoding="utf-8"):
            f = json.loads(line.lstrip("\x1e"))
            p = f["properties"]
            found[f"{p['@type'][0]}{p['@id']}"][f["geometry"]["type"] in ("Polygon", "MultiPolygon")] = f
        out: dict[str, list[tuple[shapely.Geometry, bool]]] = defaultdict(list)
        skipped: dict[str, int] = defaultdict(int)
        for key, by in found.items():
            f = by.get(False) if False in by and (True not in by or linear(by[False]["properties"])) else by[True]
            p = f["properties"]
            s = site(p) or member_site.get(key)
            if not s:
                continue
            g = shape(f["geometry"]).simplify(0.00015, preserve_topology=True)
            if g.is_empty:
                continue
            area = g.geom_type in ("Polygon", "MultiPolygon")
            if not_extent(p) or (area and not tagged_whs(p) and s in km2
                                 and g.area * 111.32 ** 2 * math.cos(math.radians(g.centroid.y)) > TOO_LARGE * km2[s]):
                skipped[names[s]] += 1
                continue
            out[s].append((g, area))
    if skipped:
        print("not the sites' extent (settlements, administrative areas, regions, regional parks, too large): "
              + ", ".join(f"{n} {k}" for k, n in sorted(skipped.items(), key=lambda kv: -kv[1])[:12]))
    return out


def centre(geoms: list[tuple[shapely.Geometry, bool]], comps: list[tuple[float, float]]) -> tuple[float, float]:
    """A site's dot: of its points (on its lines, inside its areas clear of the edges, its
    components with no outline near), the one with the least total distance to the cells it
    touches at 1/CELLS of its extent (see the module docstring)."""
    lon0 = float(np.mean([c[0] for c in comps]))
    lat0 = float(np.mean([c[1] for c in comps]))
    k = np.array([111.32 * math.cos(math.radians(lat0)), 110.57])
    o = np.array([lon0, lat0])
    lines: list[shapely.Geometry] = []
    polys: list[shapely.Geometry] = []
    for g, _ in geoms:
        g = shapely.transform(g, lambda xy: (xy - o) * k)  # km
        if g.geom_type in ("Polygon", "MultiPolygon"):
            g = shapely.make_valid(g)
        for part in shapely.get_parts(shapely.get_parts(g)):  # multi and collection parts
            if part.is_empty:
                continue
            if part.geom_type == "Polygon" and part.area > 0:
                polys.append(part)
            elif part.geom_type in ("LineString", "LinearRing") and part.length > 0:
                lines.append(shapely.LineString(part.coords))
    pts = (np.array(comps, dtype=float) - o) * k
    allb = np.array([*(g.bounds for g in lines + polys), (*pts.min(0), *pts.max(0))])
    ext = max(allb[:, 2].max() - allb[:, 0].min(), allb[:, 3].max() - allb[:, 1].min())
    h = max(ext / CELLS, 0.01)  # km

    def along(line: shapely.Geometry, closed: bool = False) -> np.ndarray:
        n = max(2, math.ceil(line.length / (h / 2)) + 1)
        d = np.linspace(0, line.length, n, endpoint=not closed)
        return shapely.get_coordinates(shapely.line_interpolate_point(line, d))

    mass, cand = [pts], []
    for ln in lines:
        s = along(ln)
        mass.append(s)
        cand.append(s)
    for pg in polys:
        x0, y0, x1, y1 = pg.bounds
        gx, gy = np.arange(x0 + h / 2, x1, h), np.arange(y0 + h / 2, y1, h)
        X, Y = (a.ravel() for a in np.meshgrid(gx, gy))
        inner = np.column_stack([X, Y])[shapely.contains_xy(pg, X, Y)] if len(X) else np.empty((0, 2))
        mass += [inner, along(pg.exterior, closed=True)]
        # Inside, clear of the edges: half its inradius, at most a cell; the pole of
        # inaccessibility always.
        try:
            pole = polylabel(pg, tolerance=max(h / 20, 1e-4))
        except Exception:  # noqa: BLE001 (a degenerate ring)
            pole = pg.representative_point()
        margin = min(h, 0.5 * pg.boundary.distance(pole))
        if len(inner):
            core = pg.buffer(-margin)
            if not core.is_empty:
                cand.append(inner[shapely.contains_xy(core, inner[:, 0], inner[:, 1])])
        cand.append(np.array([[pole.x, pole.y]]))
    if lines or polys:
        # Components with no outline near: points of the site as they are.
        idx, dist = shapely.STRtree(lines + polys).query_nearest(shapely.points(pts), return_distance=True, all_matches=False)
        near = np.full(len(pts), np.inf)
        np.minimum.at(near, idx[0], dist)
        cand.append(pts[near > 2 * h])
    else:
        cand.append(pts)
    cells = np.unique(np.floor(np.vstack(mass) / h), axis=0)
    m = ((cells + 0.5) * h).astype(np.float32)
    if len(m) > MASS_MAX:
        m = m[np.random.default_rng(0).choice(len(m), MASS_MAX, replace=False)]
    c = np.vstack([x for x in cand if len(x)])
    _, first = np.unique(np.floor(c / h), axis=0, return_index=True)
    c = c[np.sort(first)].astype(np.float32)
    total = np.concatenate([np.sqrt(((c[i:i + 512, None, :] - m[None, :, :]) ** 2).sum(-1)).sum(1) for i in range(0, len(c), 512)])
    best = c[int(np.argmin(total))].astype(float) / k + o
    return round(float(best[0]), 6), round(float(best[1]), 6)


def main() -> None:
    W.mkdir(parents=True, exist_ok=True)
    with phase("the sites read", "disk"):
        names, cats, comps = our_sites()
    print(f"{len(names)} World Heritage Sites in our regions")
    with phase("UNESCO's list read", "disk"):
        unesco = unesco_list()
        km2 = {str(x["id_no"]): x["area_hectares"] / 100 for x in unesco if x.get("area_hectares")}
    shapes = outlines(names, km2)

    with phase("the outlines' features made", "compute"):
        feats, per = [], defaultdict(int)
        for s, gs in shapes.items():
            for g, area in gs:
                gj = json.loads(json.dumps(mapping(g)), parse_float=lambda v: round(float(v), 5))
                feats.append({"type": "Feature", "geometry": gj, "properties": {"id": int(s), "n": names[s], "c": cats[s], **({"a": 1} if area else {})}})
                per[s] += 1
    with phase("the outlines written", "disk"):
        (B / "whs-shapes.json").write_text(json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False, separators=(",", ":")))
    print(f"{len(feats)} lines and areas for {len(per)} of {len(names)} sites → {B / 'whs-shapes.json'}")
    for s, n in sorted(per.items(), key=lambda kv: -kv[1])[:8]:
        print(f"  {names[s]}: {n}")

    with phase("the sites' items found", "compute"):
        items = site_items(names, comps, unesco)
    with phase("the sites' dots placed", "compute"):
        sites: dict[str, dict] = {}
        for s in names:
            rec: dict = {"q": items.get(s, [])}
            cs = sorted(set(comps[s]))
            if len(cs) > 1:
                dot = centre(shapes.get(s, []), cs)
                kx = math.cos(math.radians(dot[1]))
                lead = min(cs, key=lambda c: ((c[0] - dot[0]) * kx) ** 2 + (c[1] - dot[1]) ** 2)
                rec.update(n=len(comps[s]), dot=list(dot), lead=list(lead))
                km = math.hypot((lead[0] - dot[0]) * kx, lead[1] - dot[1]) * 111.2
                print(f"  one dot: {names[s][:52]:52s} {len(comps[s]):4d} components, {len(shapes.get(s, []))} outlines; "
                      f"dot {dot[1]:.4f}, {dot[0]:.4f} ({km:.1f} km from the nearest component)")
            sites[s] = rec
    with phase("the sites written", "disk"):
        SITES.write_text(json.dumps(sites, ensure_ascii=False, separators=(",", ":")))
    print(f"{sum('dot' in r for r in sites.values())} sites in several components get one dot → {SITES}")


def load_sites() -> dict[str, dict]:
    """whs-sites.json (empty before this step has run)."""
    return json.loads(SITES.read_text()) if SITES.exists() else {}


def roles(feats: list[dict], sites: dict[str, dict]) -> dict[int, tuple[str, str]]:
    """Per heritage.json feature index, for the sites in several components: ("lead", site id) for
    the component the site's dot takes its record from, ("part", site id) for the others."""
    out: dict[int, tuple[str, str]] = {}
    led: set[str] = set()
    for i, f in enumerate(feats):
        sid = whs_id(f["properties"])
        s = sites.get(sid) if sid else None
        if not s or "dot" not in s:
            continue
        lead = sid not in led and [round(c, 6) for c in f["geometry"]["coordinates"][:2]] == s["lead"]
        if lead:
            led.add(sid)
        out[i] = ("lead" if lead else "part", sid)
    return out


if __name__ == "__main__":
    main()
