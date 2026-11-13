"""extents.py's own rules, with no network: the metrics' readings, a piece's making, the
antimeridian and the poles, a listing's names, the range reader."""
import math
import os
import random
import tomllib
import unittest

import shapely.geometry as sg

import extents
import score


class FakeNet:
    """A fetcher answering from a table."""

    def __init__(self, answers):
        self.answers, self.requests, self.bytes, self.lengths = answers, 0, 0, {}
        self.fetched = None

    def since(self):
        self.fetched = None

    def mark(self):
        return self.requests, self.bytes

    def get(self, url, rng=None):
        body = self.answers[url]
        self.lengths[url] = len(body)
        self.requests += 1
        if rng:
            body = body[rng[0]:rng[1] + 1]
        self.bytes += len(body)
        return body

    def length(self, url):
        return self.lengths[url]


def area_km2(g):
    """A lon/lat geometry's area on a sphere, from its plate-carrée strips."""
    total = 0.0
    for p in getattr(g, "geoms", [g]):
        rings = [(p.exterior, 1)] + [(r, -1) for r in p.interiors]
        for ring, sign in rings:
            xs, ys = ring.coords.xy
            a = sum(math.radians(x1 - x0) * (math.sin(math.radians(y0)) + math.sin(math.radians(y1))) / 2 for x0, y0, x1, y1 in zip(xs, ys, xs[1:], ys[1:]))
            total += sign * abs(a) * 6371.0088 ** 2
    return total


class Metrics(unittest.TestCase):
    def test_a_constant_is_itself_and_a_field_is_read(self):
        self.assertEqual(extents.metric("ground", {}), ("ground", None))
        self.assertEqual(extents.metric({"field": "gsd"}, {"gsd": "0.5"}), (0.5, None))
        self.assertEqual(extents.metric({"field": "ft", "scale": 0.3048}, {"ft": 2}), (0.6096, None))
        self.assertEqual(extents.metric({"field": "t", "index": 1}, {"t": [7, 2.0, 0]}), (2.0, None))

    def test_a_value_not_stated_is_none_never_zero(self):
        for props in ({}, {"gsd": None}, {"gsd": ""}, {"gsd": 0}, {"gsd": "n/a"}, {"gsd": float("nan")}):
            self.assertEqual(extents.metric({"field": "gsd"}, props), (None, None), props)

    def test_a_class_maps_to_its_value_and_is_kept_as_stated(self):
        spec = {"field": "ql", "map": {"QL 1": 0.10}}
        self.assertEqual(extents.metric(spec, {"ql": "QL 1"}), (0.10, "QL 1"))
        self.assertEqual(extents.metric(spec, {"ql": "Other"}), (None, "Other"))

    def test_dates_give_their_year(self):
        self.assertEqual(extents.metric({"field": "d", "date": "epoch-ms"}, {"d": 1577923200000})[0], 2020)
        self.assertEqual(extents.metric({"field": "d", "date": "iso"}, {"d": "2019-06-30T00:00:00Z"})[0], 2019)
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "NB-Fundy_2015_2018-1m"})[0], 2018)
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "AAFC-Admiral-1m"})[0], None)
        # A compact date's year (a file's name: flown 30 Sep 2023 to 15 Mar 2024); a map sheet's
        # number is no year.
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "bc_092b041_xli1m_utm10_20230930_20240315.tif"})[0], 2024)
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "bc_092g028144_xl2m_utm10_20160630_dem.tif"})[0], 2016)

    def test_a_number_written_with_its_unit(self):
        spec = {"field": "s", "text": "metre"}
        self.assertEqual(extents.metric(spec, {"s": "2 metre"}), (2.0, None))
        self.assertEqual(extents.metric(spec, {"s": "0.5 metres"}), (0.5, None))
        # Another unit is not read as this one: kept as stated, the metric unknown.
        self.assertEqual(extents.metric(spec, {"s": "3 feet"}), (None, "3 feet"))

    def test_a_point_density_is_its_nominal_spacing(self):
        self.assertEqual(extents.metric({"field": "ppsm", "density": True}, {"ppsm": 4})[0], 0.5)


class Score(unittest.TestCase):
    with open(os.path.join(os.path.dirname(__file__), "terrain", "score.toml"), "rb") as f:
        DECL = tomllib.load(f)

    def test_an_unknown_metric_leaves_both_sums(self):
        d = self.DECL
        s, vals = score.score(d, {"spacing_m": 1, "surface": "ground"})
        self.assertIsNone(vals["year"])
        lead = vals["spacing_m"]
        # The rest is the surface's value alone (its weight not shrunk by the unknown ones).
        self.assertAlmostEqual(s, (1 - d["share"]) * lead + d["share"] * 1.0)
        # Nothing but the spacing stated: the rest is the spacing's own value.
        self.assertAlmostEqual(score.score(d, {"spacing_m": 1})[0], lead)
        # No spacing, no score.
        self.assertIsNone(score.score(d, {"surface": "ground", "year": 2020})[0])

    def test_the_scales_clamp(self):
        m = self.DECL["metric"]["spacing_m"]
        self.assertEqual(score.value(m, 0.01), 1.0)
        self.assertEqual(score.value(m, 5000), 0.0)
        self.assertAlmostEqual(score.value(m, 10), 0.5)

    def test_a_finer_piece_beyond_the_factor_always_scores_higher(self):
        """The owner's rule (13 Nov), as score.toml states it, over random pieces: finer by more
        than `within` beats coarser whatever the kind, the accuracy and the year, each stated or
        not; within it, the others may decide."""
        d, rng = self.DECL, random.Random(1334)
        m = d["metric"]["spacing_m"]
        lo, hi = math.log(m["best"]), math.log(m["worst"])

        def piece():
            p = {"spacing_m": math.exp(rng.uniform(lo, hi))}
            if rng.random() < 0.7:
                p["surface"] = rng.choice(["ground", "surface"])
            if rng.random() < 0.5:
                p["vertical_rmse_m"] = math.exp(rng.uniform(math.log(0.01), math.log(50)))
            if rng.random() < 0.7:
                p["year"] = rng.randint(1990, 2030)
            return p

        tested = 0
        for _ in range(20000):
            a, b = piece(), piece()
            if a["spacing_m"] * d["within"] < b["spacing_m"]:
                tested += 1
                self.assertGreater(score.score(d, a)[0], score.score(d, b)[0], (a, b))
        self.assertGreater(tested, 5000)
        # At the edge: finer by just over the factor, every other metric at its worst against its best.
        a = {"spacing_m": 1.0, "surface": "surface", "vertical_rmse_m": 10, "year": 2000}
        b = {"spacing_m": 1.0 * d["within"] * 1.0001, "surface": "ground", "vertical_rmse_m": 0.05, "year": 2026}
        self.assertGreater(score.score(d, a)[0], score.score(d, b)[0])
        # Within the factor the rest decides: a 1.2 m surface model below a 1.25 m bare-earth one.
        self.assertLess(score.score(d, {"spacing_m": 1.2, "surface": "surface"})[0],
                        score.score(d, {"spacing_m": 1.25, "surface": "ground"})[0])

    def test_the_servers_expected_scores_are_this_files(self):
        """The map server's scoring is tested against the scores this file makes, checked in beside
        its test (crate::inputs::extents's score_expected.json): made again from score.toml and the
        same pieces, they're the same bytes (else run the command in score.py's usage)."""
        here = os.path.join(os.path.dirname(__file__), "..", "..", "crates", "inputs", "src", "inputs", "extents")
        made = score.expected(self.DECL, os.path.join(here, "score_pieces.json"))
        with open(os.path.join(here, "score_expected.json")) as f:
            self.assertEqual(made, f.read())

    def test_a_share_breaking_the_guarantee_is_refused(self):
        score.check(self.DECL)
        with self.assertRaises(SystemExit):
            score.check({**self.DECL, "share": 0.03})


class Geometry(unittest.TestCase):
    def test_squares_side_by_side_dissolve_to_their_outline(self):
        import shapely
        tiles = [sg.box(x, y, x + 1000, y + 1000) for x in range(500000, 510000, 1000) for y in range(5000000, 5005000, 1000)]
        g = extents.tidy(extents.to_wgs84(shapely.union_all(tiles), "EPSG:32631", 1000), 50 / extents.M_PER_DEG)
        self.assertEqual(len(g.geoms), 1)
        self.assertEqual(len(g.geoms[0].interiors), 0)
        self.assertAlmostEqual(area_km2(g), 50, delta=0.5)

    def test_a_ring_over_the_antimeridian_is_cut_there(self):
        g = extents.ring_region([179.0, -179.0, -179.0, 179.0, 179.0], [10.0, 10.0, 12.0, 12.0, 10.0])
        self.assertEqual(sorted(round(p.bounds[0]) for p in g.geoms), [-180, 179])
        self.assertEqual(sorted(round(p.bounds[2]) for p in g.geoms), [-179, 180])
        self.assertAlmostEqual(g.area, 4.0)

    def test_a_polar_square_holds_its_pole_to_90(self):
        g = extents.to_wgs84(sg.box(-500000, -500000, 500000, 500000), "EPSG:3031", 20000)
        self.assertEqual(g.bounds[:3:2], (-180.0, 180.0))
        self.assertEqual(g.bounds[1], -90.0)
        self.assertTrue(g.contains(sg.Point(37.0, -89.0)) and g.contains(sg.Point(-143.0, -86.5)))
        self.assertFalse(g.contains(sg.Point(0.0, -80.0)))
        # The projection is true to scale at 71° S: at the pole a kilometre of it is 1/0.9728 on
        # the ground, so the square is up to 1/0.9728² larger than its million km².
        self.assertTrue(1.03e6 < area_km2(g) < 1.057e6, area_km2(g))

    def test_a_hole_at_the_pole_is_a_hole(self):
        ring = sg.box(-500000, -500000, 500000, 500000).difference(sg.box(-100000, -100000, 100000, 100000))
        g = extents.to_wgs84(ring, "EPSG:3031", 20000)
        self.assertFalse(g.contains(sg.Point(10.0, -89.9)))
        self.assertTrue(g.contains(sg.Point(10.0, -87.0)))
        self.assertGreater(g.bounds[1], -90.0)

    def test_rings_wind_as_rfc_7946_says(self):
        g = extents.tidy(sg.Polygon([(0, 0), (0, 1), (1, 1), (1, 0)], [[(0.2, 0.2), (0.8, 0.2), (0.8, 0.8), (0.2, 0.8)]]), 50 / extents.M_PER_DEG)
        self.assertTrue(g.geoms[0].exterior.is_ccw)
        self.assertFalse(g.geoms[0].interiors[0].is_ccw)

    def test_an_answer_declared_latitude_first_is_read_east_first(self):
        p = sg.Point(52.0, -3.5)
        self.assertEqual(extents.xy(p, {"latfirst": True}).coords[0], (-3.5, 52.0))
        self.assertEqual(extents.xy(p, {}).coords[0], (52.0, -3.5))

    def test_the_north_pole_too(self):
        g = extents.to_wgs84(sg.box(-300000, -300000, 300000, 300000), "EPSG:3413", 20000)
        self.assertEqual(g.bounds[3], 90.0)
        self.assertTrue(g.contains(sg.Point(100.0, 89.0)))


class Pieces(unittest.TestCase):
    def run_block(self, block, feats, availability="open"):
        extents.READERS["fake"] = lambda net, b, tol: iter(feats)
        extents.METHOD_CLASS["fake"] = None
        try:
            out = list(extents.block_pieces(FakeNet({}), {"id": "src", "availability": availability}, block, "2026-01-01"))
        finally:
            del extents.READERS["fake"], extents.METHOD_CLASS["fake"]
        return [f for f, _ in out if f], [r for _, r in out if r]

    def test_features_of_one_reading_are_one_piece(self):
        feats = [(sg.box(0, 0, 1, 1), {"y": 2020}), (sg.box(1, 0, 2, 1), {"y": 2020}), (sg.box(5, 5, 6, 6), {"y": 2021})]
        block = {"id": "dtm", "method": "fake", "class": "index", "url": "u", "metrics": {"surface": "ground", "year": {"field": "y", "date": "year"}}}
        pieces, reads = self.run_block(block, feats)
        self.assertEqual([p["properties"]["piece"] for p in pieces], ["dtm/year=2020", "dtm/year=2021"])
        self.assertEqual(pieces[0]["properties"], {"piece": "dtm/year=2020", "extent": "index", "availability": "open", "surface": "ground", "year": 2020})
        self.assertEqual(pieces[0]["geometry"]["type"], "MultiPolygon")
        self.assertEqual(len(pieces[0]["geometry"]["coordinates"]), 1)
        self.assertEqual(reads[0]["features"], 3)
        self.assertEqual(reads[0]["tolerance_m"], 50)

    def test_a_name_is_one_of_the_fields_a_piece_is_told_apart_by(self):
        block = {"id": "p", "method": "fake", "class": "index", "url": "u", "name": "project"}
        with self.assertRaises(SystemExit):
            self.run_block(block, [(sg.box(0, 0, 1, 1), {"project": "a"})])

    def test_what_does_not_download_directly_is_claimed(self):
        block = {"id": "x", "method": "fake", "class": "index", "url": "u"}
        pieces, _ = self.run_block(block, [(sg.box(0, 0, 1, 1), {})], availability="login")
        self.assertEqual(pieces[0]["properties"]["extent"], "claimed")

    def test_a_piece_with_no_extent_is_listed_without_geometry(self):
        out = list(extents.block_pieces(FakeNet({}), {"id": "src", "availability": "paid"},
                                        {"id": "x", "method": "none", "why": "no footprint published", "metrics": {"spacing_m": 5}}, "2026-01-01"))
        f, read = out[0]
        self.assertIsNone(f["geometry"])
        self.assertEqual(f["properties"], {"piece": "x", "extent": "unknown", "availability": "paid", "spacing_m": 5})
        self.assertEqual(read["why"], "no footprint published")


class Listing(unittest.TestCase):
    def test_names_give_their_tiles(self):
        net = FakeNet({"u": b"Copernicus_DSM_COG_10_N00_00_E006_00_DEM\nCopernicus_DSM_COG_10_S01_00_W010_00_DEM\nreadme\n"})
        b = {"url": "u", "list": "lines", "pattern": r"_(?P<ns>[NS])(?P<y>\d+)_00_(?P<ew>[EW])(?P<x>\d+)_00_DEM", "size": 1}
        boxes = sorted(g.bounds for g, _ in extents.read_listing(net, b, 0))
        self.assertEqual(boxes, [(-10.0, -1.0, -9.0, 0.0), (6.0, 0.0, 7.0, 1.0)])
        # The extent is good to a tile: its grain, recorded.
        self.assertEqual(b["grain_m"], 111320.0)

    def test_a_north_west_corner_and_a_unit(self):
        net = FakeNet({"u": b"LHD_FXX_0371_6389_MNT.tif"})
        b = {"url": "u", "list": "lines", "pattern": r"_(?P<x>\d{4})_(?P<y>\d{4})_", "unit": 1000, "size": 1000, "corner": "nw"}
        self.assertEqual([g.bounds for g, _ in extents.read_listing(net, b, 0)], [(371000.0, 6388000.0, 372000.0, 6389000.0)])


class Ranges(unittest.TestCase):
    def test_a_remote_file_reads_as_a_file(self):
        body = bytes(range(256)) * 1000
        net = FakeNet({"https://h/f.tif": body})
        f = extents.RangeFile(net, "https:/h/f.tif")
        self.assertEqual(f.seek(0, 2), len(body))
        f.seek(65530)
        self.assertEqual(f.read(20), body[65530:65550])
        f.seek(-5, 2)
        self.assertEqual(f.read(), body[-5:])
        with self.assertRaises(FileNotFoundError):
            extents.RangeFile(net, "test/test")


if __name__ == "__main__":
    unittest.main()
