"""extents.py's own rules, with no network: the metrics' readings, a piece's making, the
antimeridian and the poles, a listing's names, the range reader."""
import json
import math
import os
import random
import tomllib
import unittest
import urllib.parse

import shapely
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

    def json(self, url):
        return json.loads(self.get(url))


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
        # Two-digit years of the 2000s, a span's last (PST-A's "aammgg" flights).
        self.assertEqual(extents.metric({"field": "d", "date": "yymmdd"}, {"d": "131018"})[0], 2013)
        self.assertEqual(extents.metric({"field": "d", "date": "yymmdd"}, {"d": "090317 - 100526"})[0], 2010)
        self.assertEqual(extents.metric({"field": "d", "date": "yymmdd"}, {"d": " "})[0], None)
        # A span's two-digit end (the BEV's flight years), not a date's month.
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "2011-12"})[0], 2012)
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "DTM:2019, DSM:2009"})[0], 2019)
        self.assertEqual(extents.metric({"field": "d", "date": "text"}, {"d": "flown 2011-12-05"})[0], 2011)

    def test_a_number_written_with_its_unit(self):
        spec = {"field": "s", "text": "metre"}
        self.assertEqual(extents.metric(spec, {"s": "2 metre"}), (2.0, None))
        self.assertEqual(extents.metric(spec, {"s": "0.5 metres"}), (0.5, None))
        # Another unit is not read as this one: kept as stated, the metric unknown.
        self.assertEqual(extents.metric(spec, {"s": "3 feet"}), (None, "3 feet"))

    def test_a_point_density_is_its_nominal_spacing(self):
        self.assertEqual(extents.metric({"field": "ppsm", "density": True}, {"ppsm": 4})[0], 0.5)
        # Written with its unit, as Quebec's index has it.
        self.assertEqual(extents.metric({"field": "d", "text": "pts", "density": True}, {"d": "4 pts/m2"})[0], 0.5)

    def test_a_remark_after_the_unit_is_left_out(self):
        spec = {"field": "s", "text": "m"}
        self.assertEqual(extents.metric(spec, {"s": "0.35m (spec)"}), (0.35, None))
        self.assertEqual(extents.metric(spec, {"s": "0.28 m "}), (0.28, None))
        self.assertEqual(extents.metric(spec, {"s": "Topo 1 m, Bathy 5 m"}), (None, "Topo 1 m, Bathy 5 m"))

    def test_readings_in_turn_the_first_with_a_value(self):
        spec = [{"field": "id", "date": "text"}, {"field": "datetime", "date": "iso"}]
        self.assertEqual(extents.metric(spec, {"id": "ON-Lake_Huron_2022-1m", "datetime": "2023-06-10T12:00:00Z"})[0], 2022)
        self.assertEqual(extents.metric(spec, {"id": "AAFC-Admiral-1m", "datetime": "2012-04-17T12:00:00Z"})[0], 2012)
        self.assertEqual(extents.metric(spec, {"id": "AAFC-Admiral-1m"}), (None, None))
        # A class kept as stated survives a later reading's lack of a value.
        self.assertEqual(extents.metric([{"field": "q", "map": {"QL 1": 0.1}}, {"field": "x"}], {"q": "Other"}), (None, "Other"))
        self.assertEqual(extents.wanted_fields({"metrics": {"year": spec, "surface": "ground"}}), {"id", "datetime"})


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

    def test_a_readers_geometry_is_built_only_for_the_features_the_filter_keeps(self):
        def never():
            raise AssertionError("a filtered-out feature's geometry was built")
        feats = [(never, {"adm": "A"}), (lambda: sg.box(0, 0, 1, 1), {"adm": "B"})]
        block = {"id": "dtm", "method": "fake", "class": "claimed", "url": "u", "filter": {"adm": ["B"]}, "metrics": {}}
        pieces, [read] = self.run_block(block, feats)
        self.assertEqual(len(pieces), 1)
        self.assertEqual(read["features"], 1)

    def test_each_feature_is_generalised_to_a_quarter_of_the_tolerance_and_its_repair_counted(self):
        # A square with a fine saw along its top (a mask's pixel staircase, teeth of a metre) and a
        # bow tie (invalid: repaired, counted).
        d = 1 / extents.M_PER_DEG
        saw = [(x * d, 1000 * d + (d if x % 2 else 0)) for x in range(1000, -1, -1)]
        feats = [(sg.Polygon([(0, 0), (1000 * d, 0), *saw]), {}), (sg.Polygon([(2000 * d, 0), (3000 * d, 1000 * d), (3000 * d, 0), (2000 * d, 1000 * d)]), {})]
        block = {"id": "dtm", "method": "fake", "class": "index", "url": "u", "metrics": {}, "tolerance_m": 100}
        pieces, [read] = self.run_block(block, feats)
        self.assertEqual(read["repaired"], 1)
        g = sg.shape(pieces[0]["geometry"])
        self.assertLess(shapely.get_num_coordinates(g), 30)
        self.assertAlmostEqual(g.bounds[3], 1000 * d, delta=2 * d)

    def test_a_feature_whose_field_is_excluded_is_left_out_an_empty_one_by_its_empty_string(self):
        feats = [(sg.box(0, 0, 1, 1), {"dtm": "a.tif"}), (sg.box(1, 0, 2, 1), {"dtm": ""}), (sg.box(2, 0, 3, 1), {}),
                 (sg.box(3, 0, 4, 1), {"dtm": "b.tif"})]
        block = {"id": "dtm", "method": "fake", "class": "index", "url": "u", "exclude": {"dtm": ["", "b.tif"]}, "metrics": {}}
        pieces, [read] = self.run_block(block, feats)
        self.assertEqual(read["features"], 1)
        self.assertEqual(sg.shape(pieces[0]["geometry"]).bounds, (0.0, 0.0, 1.0, 1.0))

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

    def test_classes_of_one_value_are_pieces_apart_each_stating_its_own(self):
        feats = [(sg.box(0, 0, 1, 1), {"ql": "QL 1"}), (sg.box(3, 0, 4, 1), {"ql": "QL 2"}), (sg.box(6, 0, 7, 1), {"ql": "Other"})]
        block = {"id": "l", "method": "fake", "class": "index", "url": "u",
                 "metrics": {"vertical_rmse_m": {"field": "ql", "map": {"QL 1": 0.1, "QL 2": 0.1}}}}
        pieces, _ = self.run_block(block, feats)
        got = sorted((p["properties"]["piece"], p["properties"].get("vertical_rmse_m"), p["properties"]["stated"]["vertical_rmse_m"]) for p in pieces)
        self.assertEqual(got, [("l/vertical_rmse_m=Other", None, "Other"), ("l/vertical_rmse_m=QL 1", 0.1, "QL 1"), ("l/vertical_rmse_m=QL 2", 0.1, "QL 2")])

    def test_a_name_is_one_of_the_fields_a_piece_is_told_apart_by(self):
        block = {"id": "p", "method": "fake", "class": "index", "url": "u", "name": "project"}
        with self.assertRaises(SystemExit):
            self.run_block(block, [(sg.box(0, 0, 1, 1), {"project": "a"})])

    def test_only_a_tiles_latest_version_is_read(self):
        # (swissALTI3D's: a tile re-made in 2025 keeps its 2019 item.)
        block = {"id": "x", "method": "fake", "class": "index", "url": "u", "latest": {"field": "id", "pattern": r"\d{4}-\d{4}$", "order": "datetime"},
                 "metrics": {"year": {"field": "datetime", "date": "iso"}}}
        feats = [(sg.box(0, 0, 1, 1), {"id": "t_2019_2600-1199", "datetime": "2019-01-01"}),
                 (sg.box(0, 0, 1, 1), {"id": "t_2025_2600-1199", "datetime": "2025-01-01"}),
                 (sg.box(1, 0, 2, 1), {"id": "t_2019_2601-1199", "datetime": "2019-01-01"}),
                 (sg.box(2, 0, 3, 1), {"id": "readme", "datetime": "2019-01-01"})]
        pieces, reads = self.run_block(block, feats)
        self.assertEqual(sorted((p["properties"]["year"], round(sg.shape(p["geometry"]).bounds[0], 3)) for p in pieces), [(2019, 1.0), (2025, 0.0)])
        self.assertEqual(reads[0]["features"], 3)

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

    def test_lettered_grid_references_give_their_squares(self):
        # Orkney's 10 km HY20, Ayrshire's 5 km NS16NE quadrant, Skye's 1 km NG2004 (the British
        # National Grid); Dublin's 1 km O1234 (the Irish Grid).
        net = FakeNet({"u": b"HY20_1M_DTM.tif NS16NE_50CM_DTM.tif NG2004_50cm_DTM.tif readme"})
        b = {"url": "u", "list": "lines", "pattern": r"^(?P<ref>[A-Z]{2}\d+(?:[NS][EW])?)_", "grid": "osgb", "crs": "EPSG:27700"}
        self.assertEqual([g.bounds for g, _ in extents.read_listing(net, b, 0)],
                         [(320000, 1000000, 330000, 1010000), (215000, 665000, 220000, 670000), (120000, 804000, 121000, 805000)])
        self.assertEqual(b["grain_m"], 10000)
        self.assertEqual(extents.grid_ref("O1234", "irish"), (312000, 234000, 1000))
        with self.assertRaises(SystemExit):
            extents.grid_ref("NS123", "osgb")

    def test_a_tile_listed_twice_is_one_and_its_successor_another(self):
        net = FakeNet({"u": b"dgm1_32_433_5426_1_rp_2022.tif dgm1_32_433_5426_1_rp_2022.tif dgm1_32_433_5426_1_rp_2026.tif"})
        b = {"url": "u", "list": "lines", "pattern": r"_(?P<x>\d{3})_(?P<y>\d{4})_1_rp_(?P<year>\d{4})", "unit": 1000, "size": 1000}
        self.assertEqual([p for _, p in extents.read_listing(net, b, 0)], [{"year": "2022"}, {"year": "2026"}])

    def test_zips_members_from_their_central_directories_alone(self):
        import io
        import zipfile
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w") as z:
            for name in ("SB/dgm1_32_308_5488_1_SL_2025.tif", "SB/dgm1_32_309_5488_1_SL_2025.tif"):
                z.writestr(name, bytes(200_000))
        body = out.getvalue()
        net = FakeNet({"https://h/a.zip": body})
        b = {"urls": ["https://h/a.zip"], "list": "zip", "pattern": r"_(?P<x>\d{3})_(?P<y>\d{4})_1_SL_", "unit": 1000, "size": 1000}
        self.assertEqual([g.bounds[:2] for g, _ in extents.read_listing(net, b, 0)], [(308000.0, 5488000.0), (309000.0, 5488000.0)])
        # (The end of the file read, not its members' data.)
        self.assertLess(net.bytes, len(body) / 2)

    def test_a_spreadsheets_rows_with_their_dates(self):
        import io
        import zipfile
        main = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
        rid = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w") as z:
            z.writestr("xl/workbook.xml", f'<workbook {main} {rid}><sheets><sheet name="notes" r:id="rId2"/><sheet name="tiles" r:id="rId1"/></sheets></workbook>')
            z.writestr("xl/_rels/workbook.xml.rels", '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                       '<Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/></Relationships>')
            z.writestr("xl/sharedStrings.xml", f"<sst {main}><si><t>Kachelname</t></si><si><t>Erbach</t></si></sst>")
            z.writestr("xl/styles.xml", f'<styleSheet {main}><cellXfs><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>')
            z.writestr("xl/worksheets/sheet1.xml", f'<worksheet {main}><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row>'
                       '<row r="2"><c r="A2"><v>4985500</v></c><c r="B2" s="1"><v>43803</v></c><c r="D2" t="s"><v>1</v></c></row></sheetData></worksheet>')
            z.writestr("xl/worksheets/sheet2.xml", f'<worksheet {main}><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a note</t></is></c></row></sheetData></worksheet>')
        net = FakeNet({"u": out.getvalue()})
        self.assertEqual(list(extents.listed_names(net, {"url": "u", "list": "xlsx", "sheet": "tiles"})), ["Kachelname", "4985500;2019-12-04;;Erbach"])
        self.assertEqual(list(extents.listed_names(net, {"url": "u", "list": "xlsx"})), ["a note"])
        b = {"url": "u", "list": "xlsx", "sheet": "tiles", "pattern": r"^(?P<x>\d{3})(?P<y>\d{4});(?P<flown>[\d-]+);", "unit": 1000, "size": 1000}
        self.assertEqual([(g.bounds[:2], p) for g, p in extents.read_listing(net, b, 0)], [((498000.0, 5500000.0), {"flown": "2019-12-04"})])
        with self.assertRaises(SystemExit):
            list(extents.listed_names(net, {"url": "u", "list": "xlsx", "sheet": "other"}))

    def test_a_nested_nomenclatures_sheets(self):
        # Latvia's 1 km sheets (LKS-92 TM), as their files' own headers place them: a 100 km row and
        # column, a 50 km and a 25 km quadrant (1 and 2 the southern row), a 5 km and a 1 km row
        # and column from the south-west.
        net = FakeNet({"u": b"las/2434/2434-15-15.las\r\nlas/5412/5412-11-42.las\r\nlas/3123/3123-41-41.las\r\n"})
        b = {"url": "u", "list": "lines", "pattern": r"/(?P<a>\d)(?P<b>\d)(?P<c>\d)(?P<d>\d)-(?P<e>\d)(?P<f>\d)-(?P<g>\d)(?P<h>\d)\.las$",
             "size": 1000, "origin": [200000, 0],
             "digits": [{"group": "a", "y": 100000}, {"group": "b", "x": 100000, "base": 0},
                        {"group": "c", "x": 50000, "y": 50000, "cols": 2}, {"group": "d", "x": 25000, "y": 25000, "cols": 2},
                        {"group": "e", "y": 5000}, {"group": "f", "x": 5000}, {"group": "g", "y": 1000}, {"group": "h", "x": 1000}]}
        got = [(g.bounds, p) for g, p in extents.read_listing(net, b, 0)]
        self.assertEqual([x for x, _ in got], [(649000.0, 175000.0, 650000.0, 176000.0), (626000.0, 403000.0, 627000.0, 404000.0),
                                               (350000.0, 243000.0, 351000.0, 244000.0)])
        # The digits are the sheet's place, not its properties.
        self.assertEqual(got[0][1], {})

    def test_a_page_s_links_in_either_case(self):
        # An IIS directory listing writes its links' attribute in capitals.
        net = FakeNet({"u": b'<A HREF="/EL/ETRS89_1325_3750.tif">a</A> <a href="/EL/ETRS89_1350_3750.tif">b</a> <A HREF="/EL/x.tfw">c</A>'})
        b = {"url": "u", "list": "links", "pattern": r"ETRS89_(?P<x>\d+)_(?P<y>\d+)\.tif$", "unit": 100, "size": 2500}
        self.assertEqual(sorted(g.bounds for g, _ in extents.read_listing(net, b, 0)),
                         [(132500.0, 375000.0, 135000.0, 377500.0), (135000.0, 375000.0, 137500.0, 377500.0)])


class ArcGIS(unittest.TestCase):
    LAYER = "https://h/rest/services/X/ImageServer"

    def query(self, where, fmt):
        q = {"where": where, "outFields": "Name,OBJECTID", "outSR": "4326", "f": fmt, "returnGeometry": "true",
             "geometryPrecision": "6", "maxAllowableOffset": f"{50 / extents.M_PER_DEG / 4:.7f}",
             "orderByFields": "OBJECTID", "resultRecordCount": "2"}
        return self.LAYER + "/query?" + extents.urllib.parse.urlencode(q)

    def count(self, where, n):
        q = {"where": where, "returnCountOnly": "true", "f": "json"}
        return {self.LAYER + "/query?" + extents.urllib.parse.urlencode(q): json.dumps({"count": n}).encode()}

    ESRI = {"fields": [{"name": "OBJECTID", "type": "esriFieldTypeOID"}], "capabilities": "Image,Catalog"}

    def pages(self, *pages, count):
        """A catalogue answering `pages` (each its features' ids) two to a page, counting `count`."""
        ring = [[0, 0], [0, 1], [1, 1], [1, 0], [0, 0]]
        answers = {self.LAYER + "?f=json": json.dumps(self.ESRI).encode(), **self.count("1=1", count)}
        last = None
        for k, ids in enumerate(pages):
            where = "1=1" if last is None else f"(1=1) AND OBJECTID > {last}"
            feats = [{"attributes": {"OBJECTID": i, "Name": str(i)}, "geometry": {"rings": [ring]}} for i in ids]
            answers[self.query(where, "json")] = json.dumps({"features": feats, "exceededTransferLimit": k < len(pages) - 1}).encode()
            last = max(ids) if ids else last
        return FakeNet(answers)

    def test_an_image_servers_catalogue_reads_as_esri_json_paged_by_object_id(self):
        ring = lambda x: [[x, 0], [x, 1], [x + 1, 1], [x + 1, 0], [x, 0]]
        # The second item has a hole (Esri JSON: exterior clockwise, the hole counter-clockwise).
        hole = [[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75], [0.25, 0.25]]
        page1 = {"features": [{"attributes": {"OBJECTID": 3, "Name": "a"}, "geometry": {"rings": [ring(0)]}},
                              {"attributes": {"OBJECTID": 7, "Name": "b"}, "geometry": {"rings": [ring(2), [[2 + x, y] for x, y in hole]]}}],
                 "exceededTransferLimit": True}
        page2 = {"features": [{"attributes": {"OBJECTID": 9, "Name": "c"}, "geometry": None}]}
        net = FakeNet({
            self.LAYER + "?f=json": b'{"fields": [{"name": "OBJECTID", "type": "esriFieldTypeOID"}], "capabilities": "Image,Catalog"}',
            **self.count("Category = 1", 3),
            self.query("Category = 1", "json"): json.dumps(page1).encode(),
            self.query("(Category = 1) AND OBJECTID > 7", "json"): json.dumps(page2).encode(),
        })
        b = {"url": self.LAYER, "where": "Category = 1", "page": 2, "name": "Name", "by": ["Name"]}
        got = list(extents.read_arcgis(net, b, 50 / extents.M_PER_DEG))
        self.assertEqual([p["Name"] for _, p in got], ["a", "b"])
        self.assertAlmostEqual(got[0][0].area, 1.0)
        self.assertAlmostEqual(got[1][0].area, 0.75)
        self.assertFalse(got[1][0].contains(sg.Point(2.5, 0.5)))

    def test_a_layer_offering_geojson_is_read_as_geojson(self):
        net = FakeNet({
            self.LAYER + "?f=json": b'{"objectIdField": "OBJECTID", "supportedQueryFormats": "JSON, geoJSON"}',
            **self.count("1=1", 1),
            self.query("1=1", "geojson"): b'{"features": [{"id": 1, "properties": {"OBJECTID": 1, "Name": "a"}, "geometry": {"type": "Polygon", "coordinates": [[[0,0],[1,0],[1,1],[0,0]]]}}]}',
        })
        got = list(extents.read_arcgis(net, {"url": self.LAYER, "page": 2, "name": "Name", "by": ["Name"]}, 50 / extents.M_PER_DEG))
        self.assertEqual([p["Name"] for _, p in got], ["a"])

    def test_a_geojson_answers_z_and_null_m_are_dropped(self):
        # (Emilia-Romagna's tile indexes: each position x, y, 0, null.)
        ring = [[0, 0, 0, None], [1, 0, 0, None], [1, 1, 0, None], [0, 0, 0, None]]
        net = FakeNet({
            self.LAYER + "?f=json": b'{"objectIdField": "OBJECTID", "supportedQueryFormats": "JSON, geoJSON"}',
            **self.count("1=1", 1),
            self.query("1=1", "geojson"): json.dumps({"features": [{"id": 1, "properties": {"OBJECTID": 1, "Name": "a"}, "geometry": {"type": "Polygon", "coordinates": [ring]}}]}).encode(),
        })
        (g, _), = extents.read_arcgis(net, {"url": self.LAYER, "page": 2, "name": "Name", "by": ["Name"]}, 50 / extents.M_PER_DEG)
        self.assertFalse(g.has_z)
        self.assertAlmostEqual(g.area, 0.5)


    def test_pages_by_object_id_come_to_the_services_count(self):
        net = self.pages([3, 7], [9, 12], [], count=4)
        got = list(extents.read_arcgis(net, {"url": self.LAYER, "page": 2, "name": "Name", "by": ["Name"]}, 50 / extents.M_PER_DEG))
        self.assertEqual([p["Name"] for _, p in got], ["3", "7", "9", "12"])

    def test_rows_sharing_an_id_are_refused_within_a_page_and_across_its_edge(self):
        b = {"url": self.LAYER, "page": 2, "name": "Name", "by": ["Name"]}
        # Two rows of id 7 on one page (a join's).
        with self.assertRaises(SystemExit) as e:
            list(extents.read_arcgis(self.pages([7, 7], count=2), b, 50 / extents.M_PER_DEG))
        self.assertIn("isn't one each", str(e.exception))
        # The second row of id 7 the next page's first, which "> 7" skips: the count tells.
        with self.assertRaises(SystemExit) as e:
            list(extents.read_arcgis(self.pages([3, 7], [9], count=4), b, 50 / extents.M_PER_DEG))
        self.assertIn("3 features read in pages, where the service counts 4", str(e.exception))

    def test_a_real_footprint_with_holes_reads_as_its_exterior_less_its_holes(self):
        # Yukon's catalogue item 456: an exterior wound clockwise and three holes counter-clockwise
        # (ArcGIS's rule), the even-odd join the same as by their windings.
        with open(os.path.join(os.path.dirname(__file__), "test_esri_holes.json")) as f:
            rings = json.load(f)["geometry"]["rings"]
        g = extents.esri_shape({"rings": rings})
        windings = [sg.LinearRing(r).is_ccw for r in rings]
        self.assertEqual(windings, [False, True, True, True])
        by_winding = sg.Polygon(rings[0], rings[1:])
        self.assertTrue(by_winding.is_valid)
        [part] = getattr(g, "geoms", [g])
        self.assertEqual(len(part.interiors), 3)
        self.assertAlmostEqual(g.symmetric_difference(by_winding).area / by_winding.area, 0, places=9)
        for hole in rings[1:]:
            self.assertFalse(g.contains(sg.Polygon(hole).representative_point()))


class Atom(unittest.TestCase):
    TOP = b"""<feed xmlns="http://www.w3.org/2005/Atom" xmlns:georss="http://www.georss.org/georss">
      <entry><title>DTM 0.5 m</title><id>d</id><updated>2023-04-12T00:00:00Z</updated>
        <link href="https://h/sub.xml" rel="alternate" type="application/atom+xml"/>
        <link href="https://h/a.tif" rel="section" type="image/tiff" length="10" title="sheet a" bbox="5.5 53.4 5.6 53.5"/>
        <georss:polygon>50 3 50 7 54 7 54 3 50 3</georss:polygon>
      </entry>
      <entry><title>no files</title><updated>2021-01-01T00:00:00Z</updated>
        <georss:polygon>50 3 50 4 51 4 51 3 50 3</georss:polygon>
      </entry>
    </feed>"""
    SUB = b"""<feed xmlns="http://www.w3.org/2005/Atom" xmlns:georss="http://www.georss.org/georss">
      <entry><title>zone</title><updated>2024-05-01T00:00:00Z</updated><georss:box>45 -73 46 -72</georss:box>
        <link href="https://h/top.xml" rel="up" type="application/atom+xml"/>
        <link href="https://h/other.xml" rel="up" type="application/atom+xml"/></entry>
    </feed>"""

    def test_files_by_their_bbox_entries_by_their_polygon_and_feeds_followed_once(self):
        net = FakeNet({"https://h/top.xml": self.TOP, "https://h/sub.xml": self.SUB})
        got = list(extents.read_atom(net, {"url": "https://h/top.xml", "follow": True}, 0))
        self.assertEqual([g.bounds for g, _ in got], [(5.5, 53.4, 5.6, 53.5), (3.0, 50.0, 4.0, 51.0), (-73.0, 45.0, -72.0, 46.0)])
        self.assertEqual(got[0][1]["href"], "https://h/a.tif")
        self.assertEqual(got[0][1]["updated"], "2023-04-12T00:00:00Z")
        self.assertEqual(got[2][1]["title"], "zone")
        self.assertEqual(net.requests, 2)
        # Not followed unless the declaration says.
        self.assertEqual(len(list(extents.read_atom(FakeNet({"https://h/top.xml": self.TOP}), {"url": "https://h/top.xml"}, 0))), 2)

    def test_a_feed_writing_its_bboxes_latitude_first(self):
        feed = b"""<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>DTM</title>
          <link rel="section" href="https://h/t.zip" type="image/tiff" bbox="50.8134 4.2268 50.8224 4.2410"/></entry></feed>"""
        [(g, _)] = list(extents.read_atom(FakeNet({"u": feed}), {"url": "u", "latfirst": True}, 0))
        self.assertEqual(g.bounds, (4.2268, 50.8134, 4.241, 50.8224))


def geopackage(rows, srs=25832):
    """A GeoPackage of one features table `tiles` (name, year, geom), as bytes."""
    import sqlite3
    import struct
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.gpkg")
        db = sqlite3.connect(path)
        db.executescript(f"""
            CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT, srs_id INTEGER PRIMARY KEY, organization TEXT, organization_coordsys_id INTEGER, definition TEXT);
            INSERT INTO gpkg_spatial_ref_sys VALUES ('etrs89', {srs}, 'EPSG', {srs}, 'undefined');
            CREATE TABLE gpkg_contents (table_name TEXT, data_type TEXT);
            INSERT INTO gpkg_contents VALUES ('tiles', 'features');
            CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT, srs_id INTEGER);
            INSERT INTO gpkg_geometry_columns VALUES ('tiles', 'geom', {srs});
            CREATE TABLE tiles (fid INTEGER PRIMARY KEY, name TEXT, year INTEGER, geom BLOB);""")
        for k, (name, year, g) in enumerate(rows):
            # (The header with an envelope of four doubles, flags 0b0011: little-endian, xy envelope.)
            blob = None if g is None else b"GP\x00\x03" + struct.pack("<i", srs) + struct.pack("<4d", *g.bounds) + shapely.to_wkb(g)
            db.execute("INSERT INTO tiles VALUES (?, ?, ?, ?)", (k, name, year, blob))
        db.commit()
        db.close()
        with open(path, "rb") as f:
            return f.read()


def zipped(name, body):
    import io
    import zipfile
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as z:
        z.writestr("readme.txt", "x")
        z.writestr(name, body)
    return out.getvalue()


GML_ANSWER = b"""<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:dgm="https://h/dgm">
  <wfs:member><dgm:kachel gml:id="k1"><dgm:name>32_480_5500</dgm:name><dgm:jahr>2021</dgm:jahr>
    <dgm:geom><gml:Polygon srsName="urn:ogc:def:crs:EPSG::25832"><gml:exterior><gml:LinearRing>
      <gml:posList srsDimension="2">480000 5500000 481000 5500000 481000 5501000 480000 5501000 480000 5500000</gml:posList>
    </gml:LinearRing></gml:exterior><gml:interior><gml:LinearRing>
      <gml:posList>480100 5500100 480100 5500200 480200 5500200 480200 5500100 480100 5500100</gml:posList>
    </gml:LinearRing></gml:interior></gml:Polygon></dgm:geom></dgm:kachel></wfs:member>
  <wfs:member><dgm:kachel gml:id="k2"><dgm:name>32_481_5500</dgm:name><dgm:jahr>2023</dgm:jahr>
    <dgm:geom><gml:MultiSurface srsName="urn:ogc:def:crs:EPSG::25832"><gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>
      <gml:coordinates>481000,5500000 482000,5500000 482000,5501000 481000,5501000 481000,5500000</gml:coordinates>
    </gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember></gml:MultiSurface></dgm:geom></dgm:kachel></wfs:member>
  <wfs:member><dgm:kachel gml:id="k3"><dgm:name>no geometry</dgm:name></dgm:kachel></wfs:member>
</wfs:FeatureCollection>"""


class IndexFiles(unittest.TestCase):
    def test_a_geopackage_gives_its_rows_in_its_crs(self):
        body = geopackage([("a", 2020, sg.box(480000, 5500000, 481000, 5501000)), ("b", 2022, None)])
        b = {"url": "u"}
        got = [(g(), p) for g, p in extents.read_file(FakeNet({"u": body}), b, 0)]
        self.assertEqual(b["crs"], "EPSG:25832")
        self.assertEqual(got[0][0].bounds, (480000.0, 5500000.0, 481000.0, 5501000.0))
        self.assertEqual(got[0][1], {"fid": 0, "name": "a", "year": 2020})
        # A row with no geometry is a feature with none.
        self.assertEqual((got[1][0], got[1][1]["name"]), (None, "b"))

    def test_a_zip_gives_the_index_file_it_holds(self):
        body = zipped("index/kacheln.gpkg", geopackage([("a", 2020, sg.box(0, 0, 1, 1))], srs=4326))
        got = [g().bounds for g, _ in extents.read_file(FakeNet({"u": body}), {"url": "u"}, 0)]
        self.assertEqual(got, [(0.0, 0.0, 1.0, 1.0)])
        gj = b'{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"y":1},"geometry":{"type":"Polygon","coordinates":[[[0,0],[2,0],[2,2],[0,0]]]}}]}'
        got = [(g().bounds, p) for g, p in extents.read_file(FakeNet({"u": zipped("a/tiles.geojson", gj)}), {"url": "u"}, 0)]
        self.assertEqual(got, [((0.0, 0.0, 2.0, 2.0), {"y": 1})])
        with self.assertRaises(SystemExit):
            list(extents.read_file(FakeNet({"u": zipped("a/tiles.geojson", gj)}), {"url": "u", "member": "other"}, 0))

    def test_a_shapefiles_text_in_its_encoding(self):
        import io
        import zipfile

        import shapefile
        shp, shx, dbf = io.BytesIO(), io.BytesIO(), io.BytesIO()
        with shapefile.Writer(shp=shp, shx=shx, dbf=dbf, shapeType=shapefile.POLYGON, encoding="cp1250") as w:
            w.field("LOT_nazov", "C", 40)
            w.poly([[[0, 0], [0, 1], [1, 1], [1, 0], [0, 0]]])
            w.record("Banská Štiavnica")

        def body(cpg=None):
            out = io.BytesIO()
            with zipfile.ZipFile(out, "w") as z:
                for ext, part in (("shp", shp), ("shx", shx), ("dbf", dbf)):
                    z.writestr("lots." + ext, part.getvalue())
                if cpg:
                    z.writestr("lots.cpg", cpg)
            return out.getvalue()
        # (No .cpg: the declaration says it, as Slovakia's lots need; Esri's "1250" a code page.)
        for b, zip_body in (({"url": "u", "encoding": "cp1250"}, body()), ({"url": "u"}, body("1250"))):
            got = [p for _, p in extents.read_file(FakeNet({"u": zip_body}), b, 0)]
            self.assertEqual(got, [{"LOT_nazov": "Banská Štiavnica"}])

    def test_gml_gives_polygons_surfaces_and_their_properties(self):
        b = {"url": "u"}
        got = [(g(), p) for g, p in extents.read_file(FakeNet({"u": GML_ANSWER}), b, 0)]
        self.assertEqual(b["crs"], "urn:ogc:def:crs:EPSG::25832")
        self.assertEqual([p for _, p in got], [{"name": "32_480_5500", "jahr": "2021"}, {"name": "32_481_5500", "jahr": "2023"}, {"name": "no geometry"}])
        self.assertEqual(got[0][0].area, 1000 * 1000 - 100 * 100)
        self.assertEqual(got[1][0].bounds, (481000.0, 5500000.0, 482000.0, 5501000.0))
        self.assertIsNone(got[2][0])

    def test_a_wfs_answering_gml_is_paged_by_every_feature(self):
        # (Three features a page, one without geometry: the page is full, so a second is asked for.)
        def url(off):
            return "https://h/wfs?" + urllib.parse.urlencode({"service": "WFS", "version": "2.0.0", "request": "GetFeature", "typeNames": "dgm:kachel",
                                                              "outputFormat": "text/xml; subtype=gml/3.2", "count": "3", "startIndex": str(off)})
        net = FakeNet({url(0): GML_ANSWER, url(3): b'<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0"/>'})
        b = {"url": "https://h/wfs", "type": "dgm:kachel", "format": "text/xml; subtype=gml/3.2", "page": 3}
        got = list(extents.read_wfs(net, b, 0))
        self.assertEqual(len(got), 3)
        self.assertEqual(net.requests, 2)


class StacAssets(unittest.TestCase):
    def test_an_items_footprint_from_its_asset(self):
        items = {"features": [
            {"id": "p-1m", "properties": {}, "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [9, 0], [9, 9], [0, 0]]]},
             "assets": {"extent": {"href": "https://h/p-extent.geojson"}}},
            {"id": "q-1m", "properties": {}, "geometry": {"type": "Polygon", "coordinates": [[[20, 0], [21, 0], [21, 1], [20, 0]]]}, "assets": {}}],
            "links": []}
        net = FakeNet({"https://h/c/items?limit=500": json.dumps(items).encode(),
                       "https://h/p-extent.geojson": b'{"type": "Feature", "geometry": {"type": "Polygon", "coordinates": [[[0,0],[1,0],[1,1],[0,1],[0,0]]]}}'})
        got = list(extents.read_stac(net, {"url": "https://h/c", "geometry": "extent"}, 0))
        self.assertEqual([(p["id"], g.bounds) for g, p in got], [("p-1m", (0.0, 0.0, 1.0, 1.0)), ("q-1m", (20.0, 0.0, 21.0, 1.0))])

    def test_a_next_link_dropping_the_page_size_gets_it_back(self):
        # (Luzern's: its next link is ?page=2, whose default size answers nothing, or everything.)
        def page(i, nxt):
            return json.dumps({"features": [{"id": f"t{i}", "properties": {}, "geometry": {"type": "Point", "coordinates": [i, 0]}}],
                               "links": [{"rel": "next", "href": nxt}] if nxt else []}).encode()
        net = FakeNet({"https://h/c/items?limit=1": page(1, "https://h/c/items?page=2"),
                       "https://h/c/items?page=2&limit=1": page(2, "https://h/c/items?page=3&limit=1"),
                       "https://h/c/items?page=3&limit=1": page(3, None)})
        got = list(extents.read_stac(net, {"url": "https://h/c", "page": 1}, 0))
        self.assertEqual([p["id"] for _, p in got], ["t1", "t2", "t3"])

    def test_paged_to_the_first_page_with_no_items_whatever_it_links(self):
        # (Ellipsis Drive's OGC API: an empty page links `next` back to the first.)
        item = {"id": "s", "properties": {}, "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]]}}
        first = "https://h/c/items?limit=500"
        net = FakeNet({first: json.dumps({"features": [item], "links": [{"rel": "next", "href": "https://h/c/items?offset=a&limit=500"}]}).encode(),
                       "https://h/c/items?offset=a&limit=500": json.dumps({"features": [], "links": [{"rel": "next", "href": first}]}).encode()})
        self.assertEqual(len(list(extents.read_stac(net, {"url": "https://h/c"}, 0))), 1)
        self.assertEqual(net.requests, 2)
        # A `next` back to a page already read, after items, stops the run (they'd be read twice).
        net = FakeNet({first: json.dumps({"features": [item], "links": [{"rel": "next", "href": first}]}).encode()})
        with self.assertRaises(SystemExit):
            list(extents.read_stac(net, {"url": "https://h/c"}, 0))


class RasterFiles(unittest.TestCase):
    def test_a_buckets_files_by_their_names(self):
        listing = b"""<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>raster</Name>
          <Contents><Key>Haiti_16/Haiti_16_be.vrt</Key></Contents>
          <Contents><Key>Haiti_16/Haiti_16_be/Haiti_651800_2063900.tif</Key></Contents>
          <Contents><Key>Haiti_16/Haiti_16_be/Haiti_651800_2073900.tif</Key></Contents></ListBucketResult>"""
        net = FakeNet({"https://h/raster/?list-type=2&prefix=Haiti_16%2FHaiti_16_be%2F": listing})
        b = {"url": "https://h/raster", "list": "s3", "prefix": "Haiti_16/Haiti_16_be/", "pattern": r"\.tif$"}
        self.assertEqual(extents.raster_files(net, b), ["https://h/raster/Haiti_16/Haiti_16_be/Haiti_651800_2063900.tif",
                                                        "https://h/raster/Haiti_16/Haiti_16_be/Haiti_651800_2073900.tif"])


    def tif(self, overviews):
        import numpy as np
        import rasterio
        from rasterio.io import MemoryFile
        from rasterio.transform import from_origin
        data = np.zeros((256, 256), np.float32)
        data[:, :128] = -9999
        with MemoryFile() as mem:
            with mem.open(driver="GTiff", width=256, height=256, count=1, dtype="float32", crs="EPSG:32618",
                          transform=from_origin(650000, 2070000, 2, 2), nodata=-9999, tiled=True) as ds:
                ds.write(data, 1)
                if overviews:
                    ds.build_overviews([2, 4, 8], rasterio.enums.Resampling.nearest)
            return mem.read()

    def test_b_a_file_without_overviews_is_refused_not_read_whole(self):
        net = FakeNet({"https://h/a.tif": self.tif(False)})
        with self.assertRaises(SystemExit) as e:
            list(extents.read_raster(net, {"urls": ["https://h/a.tif"], "mask_m": 20}, 0))
        self.assertIn("no overviews", str(e.exception))

    def test_c_a_file_s_mask_is_its_valid_half(self):
        net = FakeNet({"https://h/a.tif": self.tif(True)})
        b = {"urls": ["https://h/a.tif"], "mask_m": 20}
        [(geom, props)] = list(extents.read_raster(net, b, 0))
        self.assertEqual(props, {"res_m": 2.0})
        self.assertEqual(b["grain_m"], 16.0)
        valid = extents.to_wgs84(sg.box(650256, 2069488, 650512, 2070000), "EPSG:32618", 1)
        self.assertAlmostEqual(geom.symmetric_difference(valid).area / valid.area, 0, places=2)


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
