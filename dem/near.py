"""Whether a box comes near the coverage, asked of the coverage's own polygons through an STR tree,
never of the coverage grown or merged (task #493): bldfetch's Near (round: within its margin's
distance)."""
import math

from shapely import STRtree, make_valid
from shapely.geometry import box


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

