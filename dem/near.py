"""Whether a box comes near the coverage, asked of the coverage's own polygons or rings through an
STR tree, never of the coverage grown or merged (tasks #493, #498): bldfetch's Near (round: within
its margin's distance), railfeeds' EvenOdd (as pipeline::coverage answers: the box grown by a
rectangle)."""
import math
from collections.abc import Iterable

import numpy as np
from shapely import STRtree, make_valid
from shapely.geometry import LineString, box


class Near:
    """Whether a box comes within the margin of the coverage: a distance query on the coverage's
    polygons as they are (an STR tree), never the coverage grown. Growing it (a buffer of every
    region, then their union) took GEOS past 60 GB on the regions' 244 MB outlines (126 regions,
    one of 98 k polygons: each polygon's 20 km ring overlapping thousands of others), task #493."""

    def __init__(self, polys: list, margin_km: float):
        # (GEOS leaves its predicates on invalid input undefined; its distances on a bowtie happen
        # to be right today, so an invalid polygon is made valid first, every part kept: the
        # lobes, and the lines and points of a zero-area outline, which still answer distances.
        # buffer(0) would drop a bowtie's lobe and a zero-area outline whole.)
        fixed = [make_valid(g) if not g.is_valid else g for g in polys]
        self.margin_km = margin_km
        self.tree = STRtree([x for g in fixed for x in getattr(g, "geoms", [g]) if not x.is_empty])

    def meets(self, w: float, s: float, e: float, n: float) -> bool:
        # Degrees of longitude shrink with latitude: the margin in degrees at the highest latitude
        # within the margin of the box, so it's at least the margin everywhere near it. Near a pole
        # (that latitude's circle shorter than the margin's reach) every longitude is near.
        dlat = self.margin_km / 111.32
        lat = max(abs(s), abs(n)) + dlat
        c = math.cos(math.radians(min(lat, 90.0)))
        dlon = self.margin_km / (111.32 * c) if c > 1e-9 else math.inf
        if dlon >= 180.0:
            return len(self.tree.query(box(-180.0, s - dlat, 180.0, n + dlat))) > 0
        # (No wrap in the coordinates: the box also a turn east or west when its margin crosses ±180°, so a polygon at
        # 179.9° meets a box at -179.9°.)
        ks = [0.0] + ([360.0] if w - dlon < -180.0 else []) + ([-360.0] if e + dlon > 180.0 else [])
        return any(len(self.tree.query(box(w + k, s, e + k, n), predicate="dwithin", distance=dlon)) > 0 for k in ks)


# Edges in each piece of a ring the trees hold: a box asks about the few pieces near it, never a
# ring of a million points whole.
PIECE = 64


# Metres in a degree of latitude, and the latitude a buffer's longitude reach is capped at, as
# pipeline::coverage's M_PER_E7 and Shape::grown.
M_PER_DEG = 111_320.0
LAT_CAP = 85.0


class EvenOdd:
    """The job's coverage as railfeeds reads it (rail::coverage_geojson: a shape per feature, its
    rings under the even–odd rule, its buffer in metres), answered as pipeline::coverage's
    Shape::meets_rect does: a box meets a shape when one of its rings' edges meets the box grown by
    the buffer (a rectangle: the buffer in E7 units of longitude at the box's latitude furthest from
    the equator, capped at 85°, and of latitude, rounded up; up to √2 × the buffer at its corners),
    else when the box's centre is inside it (an odd count of its rings' edges crossing the ray east
    of it). Unlike Rust, a grown box crossing ±180° also meets edges a turn away.

    Never the rings' polygons merged: each ring's symmetric difference into its shape, then the
    shapes' union, ran GEOS for over 6.5 h on one core on the regions' 244 MB outlines (a shape of
    98 k rings, each difference redoing the whole shape so far), task #498."""

    def __init__(self, shapes: Iterable[tuple[list[list], float]]):
        by_buffer: dict[float, list] = {}
        lines, owner = [], []
        for k, (rings, buffer_m) in enumerate(shapes):
            for ring in rings:
                if len(ring) < 3:
                    continue
                r = np.asarray(ring, dtype=float)[:, :2]
                if not np.array_equal(r[0], r[-1]):
                    r = np.vstack([r, r[:1]])
                for i in range(0, len(r) - 1, PIECE):
                    by_buffer.setdefault(buffer_m, []).append(len(lines))
                    lines.append(LineString(r[i:i + PIECE + 1]))
                    owner.append(k)
        self.lines, self.owner = lines, owner
        self.tree = STRtree(lines)
        # The edges by buffer: a tree of each buffer's pieces.
        self.by_buffer = [(b, STRtree([lines[i] for i in ix])) for b, ix in by_buffer.items()]

    def inside(self, x: float, y: float) -> bool:
        """Whether (x, y) is inside a shape: an odd count of its rings' edges crossing the ray east."""
        odd: dict[int, bool] = {}
        for i in self.tree.query(box(x, y, 180.0, y)):
            p = np.asarray(self.lines[i].coords)
            (x1, y1), (x2, y2) = p[:-1].T, p[1:].T
            cross = (y1 > y) != (y2 > y)
            with np.errstate(divide="ignore", invalid="ignore"):
                at = x1 + (y - y1) * (x2 - x1) / (y2 - y1)
            if np.count_nonzero(cross & (x < at)) % 2:
                odd[self.owner[i]] = not odd.get(self.owner[i], False)
        return any(odd.values())

    def meets(self, w: float, s: float, e: float, n: float) -> bool:
        lat = min(max(abs(s), abs(n)), LAT_CAP)
        for b, tree in self.by_buffer:
            bx = math.ceil(b / (M_PER_DEG * 1e-7 * math.cos(math.radians(lat)))) * 1e-7 if b > 0 else 0.0
            by = math.ceil(b / (M_PER_DEG * 1e-7)) * 1e-7 if b > 0 else 0.0
            gw, gs, ge, gn = w - bx, s - by, e + bx, n + by
            ks = [0.0] + ([360.0] if gw < -180.0 else []) + ([-360.0] if ge > 180.0 else [])
            if any(len(tree.query(box(gw + k, gs, ge + k, gn), predicate="intersects")) > 0 for k in ks):
                return True
        # No edge meets the grown box: the box is all inside or all outside, which its centre tells.
        return self.inside((w + e) / 2, (s + n) / 2)
