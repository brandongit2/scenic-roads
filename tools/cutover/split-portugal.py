#!/usr/bin/env python3
# Portugal's Geofabrik outline (one ring around the mainland, Madeira and the Azores) in three: an
# outline around each archipelago, strictly inside the ring, and the mainland as the ring itself
# with those two as holes. The ring's own edges stay as they were, so the coverage, and every
# unit's fingerprint but the islands', stays (docs/plan.md §5: a unit is keyed on the edges inside
# its box).
#
#   uv run --with shapely tools/cutover/split-portugal.py <NAS>/inputs/outlines/geofabrik/europe-portugal.poly
#
# writes portugal-mainland.poly, azores.poly and madeira.poly here, for inputs/outlines/ (the recipes
# portugal, azores and madeira name them as poly:<file>).
import sys
from shapely.geometry import Point, Polygon

src = sys.argv[1]
text = open(src).read()
lines = [l for l in text.split("\n")]
ring = []
for l in lines[2:]:
    s = l.split()
    if len(s) == 2:
        ring.append((float(s[0]), float(s[1])))
whole = Polygon(ring)
assert whole.is_valid
azores = [(-31.5, 38.9), (-31.5, 40.0), (-24.4, 40.0), (-24.4, 36.4)]
madeira = [(-17.6, 30.5), (-17.6, 33.4), (-15.6, 33.4), (-15.6, 29.9), (-16.4, 29.9)]
islands = {
    "azores": [("Flores", -31.21, 39.43), ("Corvo", -31.11, 39.70), ("Faial", -28.70, 38.58), ("Pico", -28.33, 38.46),
               ("Sao Jorge", -28.03, 38.65), ("Graciosa", -28.00, 39.05), ("Terceira", -27.22, 38.72), ("Sao Miguel", -25.50, 37.78),
               ("Santa Maria", -25.10, 36.95), ("Formigas", -24.78, 37.27), ("Santa Maria S", -25.15, 36.93)],
    "madeira": [("Madeira W", -17.27, 32.80), ("Madeira E", -16.66, 32.75), ("Porto Santo", -16.33, 33.08), ("Desertas", -16.50, 32.45),
                ("Selvagem Grande", -15.87, 30.14), ("Selvagem Pequena", -16.03, 30.04)],
}
for name, pts in (("azores", azores), ("madeira", madeira)):
    p = Polygon(pts)
    assert p.is_valid
    # Strictly inside the ring, clear of its edges.
    assert whole.contains(p), name
    print(f"{name}: {p.exterior.distance(whole.exterior) * 111:.0f} km clear of the ring at the closest")
    for n, x, y in islands[name]:
        assert p.contains(Point(x, y)), (name, n)
        print(f"  {n:16s} {p.exterior.distance(Point(x, y)) * 111:5.0f} km inside")

def ring_lines(i, pts, hole=False):
    out = [("!" if hole else "") + str(i)]
    closed = pts + [pts[0]]
    out += [f"   {x:.6E}   {y:.6E}" for x, y in closed]
    out.append("END")
    return out

# The mainland: the original file's ring, verbatim, then the two holes.
body = lines[1:]
end = max(i for i, l in enumerate(body) if l.strip() == "END")
assert body[0].strip() == "1"
mainland = ["portugal-mainland"] + body[:end] + ring_lines(2, azores, True) + ring_lines(3, madeira, True) + ["END"]
open("portugal-mainland.poly", "w").write("\n".join(mainland) + "\n")
for name, pts in (("azores", azores), ("madeira", madeira)):
    open(f"{name}.poly", "w").write("\n".join([name] + ring_lines(1, pts) + ["END"]) + "\n")
print("written")
