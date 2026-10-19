"""smalllakes.py's checks. Run from producers/depth: uv run --project .. python -m unittest test_smalllakes"""
from __future__ import annotations

import collections
import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np

import smalllakes
from grid import HALF, TILE, Writer, lonlat_to_merc, px_size


class Formula(unittest.TestCase):
    def test_the_labs_table(self):
        # (validation.md §3, "What the chosen model gives (m)", to its 0.1 m.)
        for S, L, want in [(0.01, 100, 2.3), (0.01, 1000, 6.6), (0.03, 20, 0.7), (0.03, 200, 5.5), (0.06, 50, 2.4),
                           (0.06, 500, 10.7), (0.12, 10, 0.5), (0.12, 1000, 16.3)]:
            d, by_slope = smalllakes.dmax_estimate(S, L, 1.0)
            self.assertTrue(by_slope)
            self.assertAlmostEqual(d, want, delta=0.05, msg=(S, L))

    def test_l_is_held_at_its_peak(self):
        peak = math.exp(-smalllakes.BETA[2] / (2 * smalllakes.BETA[3]))
        self.assertAlmostEqual(peak, 1790, delta=5)
        a, _ = smalllakes.dmax_estimate(0.05, peak, 1.0)
        b, _ = smalllakes.dmax_estimate(0.05, 5000, 1.0)
        self.assertAlmostEqual(a, b, places=9)

    def test_flat_or_missing_land_takes_the_area_rule(self):
        # (Dmax = 3.04 · A^0.228.)
        for S in (None, float("nan"), 0.002):
            d, by_slope = smalllakes.dmax_estimate(S, 50, 1.0)
            self.assertFalse(by_slope)
            self.assertAlmostEqual(d, 3.04, delta=0.005)
        d, _ = smalllakes.dmax_estimate(None, 50, 5.0)
        self.assertAlmostEqual(d, 3.04 * 5 ** 0.228, delta=0.01)


def merc_square(lon, lat, side_m):
    """A square of `side_m` ground metres, its south-west corner at lon/lat (Mercator)."""
    import shapely

    x, y = (float(v) for v in lonlat_to_merc(lon, lat))
    s = side_m / math.cos(math.radians(lat))
    return shapely.box(x, y, x + s, y + s)


class Slope(unittest.TestCase):
    def test_a_plane_gives_its_slope(self):
        lat = 45.0
        k = math.cos(math.radians(lat))
        r = px_size(smalllakes.TERRAIN_Z)

        def terrain(c0, r0, w, h):
            # (Rising 0.05 m a ground metre eastwards.)
            return (100 + 0.05 * (np.arange(w)[None, :] + c0) * r * k + np.zeros((h, 1))).astype("float32")

        g = merc_square(-73.0, lat, 200)
        S = smalllakes.land_slope(g, [], terrain, k)
        self.assertAlmostEqual(S, 0.05, delta=0.002)
        # Below 0.3 m (the sea) isn't land.
        self.assertIsNone(smalllakes.land_slope(g, [], lambda c0, r0, w, h: np.zeros((h, w), "float32"), k))


class Bowl(unittest.TestCase):
    def test_a_pond_is_a_bowl_to_its_depth(self):
        g = merc_square(-73.0, 45.0, 150)
        span = TILE * 2 ** (smalllakes.ZOOM - smalllakes.PIECE_ZOOM)
        x, y = g.centroid.x, g.centroid.y
        tsz = 2 * HALF / 2 ** smalllakes.PIECE_ZOOM
        tx, ty = int((x + HALF) // tsz), int((HALF - y) // tsz)
        t = smalllakes.paint([(g, 3.0)], tx, ty)
        ok = np.isfinite(t)
        self.assertEqual(t.shape, (span, span))
        self.assertTrue(200 < ok.sum() < 1000)  # (150 m ≈ 21 z14 pixels a side)
        self.assertAlmostEqual(float(np.nanmin(t)), -3.0, places=3)
        self.assertTrue(float(np.nanmax(t)) > -0.6, "shallow at the shore")
        self.assertTrue((t[ok] < 0).all())


class FakeTiles:
    """Basemap tiles from (class, OSM id, intermittent, Mercator polygon)s; a plane for terrain."""

    def __init__(self, lakes, slope=0.05):
        self.lakes, self.slope = lakes, slope

    def base(self, z, x, y):
        import mapbox_vector_tile
        import shapely

        tsz = 2 * HALF / 2 ** z
        box = shapely.box(-HALF + x * tsz, HALF - (y + 1) * tsz, -HALF + (x + 1) * tsz, HALF - y * tsz)
        feats = []
        for cls, oid, it, g in self.lakes:
            c = g.intersection(box)
            if c.is_empty or c.area == 0:
                continue
            c = shapely.affinity.affine_transform(c, [4096 / tsz, 0, 0, -4096 / tsz, -(-HALF + x * tsz) * 4096 / tsz, (HALF - y * tsz) * 4096 / tsz])
            feats.append({"geometry": c.wkt, "properties": {"class": cls, "id": oid, "intermittent": it}, "id": oid * 10 + 2})
        if not feats:
            return b""
        return mapbox_vector_tile.encode([{"name": "water", "features": feats}], default_options={"y_coord_down": True, "quantize_bounds": None})

    def terrain(self, z, x, y):
        r = px_size(z)
        cols = (np.arange(TILE) + x * TILE)[None, :] * r * math.cos(math.radians(45.0))
        return (100 + self.slope * cols + np.zeros((TILE, 1))).astype("float32")


class Run(unittest.TestCase):
    def test_only_the_small_lakes_hydrolakes_lacks_are_drawn(self):
        pond = merc_square(-73.00, 45.0, 150)  # 2.25 ha: drawn
        big = merc_square(-72.99, 45.0, 400)  # 16 ha: GLOBathy's size
        known = merc_square(-72.98, 45.0, 150)  # in HydroLAKES
        dry = merc_square(-72.97, 45.0, 150)  # intermittent
        tiles = FakeTiles([("lake", 1, 0, pond), ("lake", 2, 0, big), ("pond", 3, 0, known), ("lake", 4, 1, dry)])
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            w = Writer(t / "out")
            counts = collections.Counter()
            n = smalllakes.produce(tiles, [known.buffer(10)], w, [-73.5, 44.5, -72.5, 45.5], counts)
            w.write_index(t / "i.geojson")
            self.assertEqual(n, 1)
            self.assertEqual(counts, {"by the land slope": 1, "in HydroLAKES": 1, "intermittent": 1})
            files = [f["properties"] for f in json.loads((t / "i.geojson").read_text())["features"]]
            self.assertEqual(len(files), 1)
            self.assertEqual(files[0]["zoom"], smalllakes.ZOOM)
            import rasterio

            with rasterio.open(t / "out" / files[0]["file"]) as r:
                a = r.read(1)
            # (L ≈ 75 m, S = 0.05: the lab's model.)
            want, _ = smalllakes.dmax_estimate(0.05, 75.0, 2.25)
            self.assertAlmostEqual(float(np.nanmin(a)), -want, delta=0.1 * want)
            self.assertTrue(200 < np.isfinite(a).sum() < 1000, "the pond alone")


if __name__ == "__main__":
    unittest.main()
