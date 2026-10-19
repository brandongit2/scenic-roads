"""datums.py's checks. Run from producers/depth: python -m unittest test_datums"""
from __future__ import annotations

import unittest

import numpy as np

import datums


class OpenSea(unittest.TestCase):
    """Open-sea extrapolation (opt-in): open sea beyond the reach takes the nearest value, an
    enclosed lagoon or an inland lake gets nodata."""
    none = datums.OpenSea([])

    def test_a_body_at_the_grid_edge_beyond_reach_takes_the_nearest_solved_value(self):
        water = np.zeros((40, 400), bool)
        water[:, 0:300] = True
        water[:, 330:400] = True  # 19% of the water, 3 degrees off, at the east edge: the sea going on
        s = datums.Surface.harmonic([(10.5, 48.0, 1.0)], water, 10.0, 50.0, 0.1, self.none)
        self.assertAlmostEqual(float(s.a[5, 390]), 1.0, places=4)

    def test_the_edge_rule_alone(self):
        lab = np.zeros((40, 40), np.int32)
        lab[10:20, 10:20] = 1  # 10 cells in
        lab[10:20, 26:35] = 2  # ends at column 34, in the 6-cell margin (columns 34..39)
        lab[25:30, 10:34] = 3  # ends at column 33, just outside it
        self.assertEqual(datums.at_edge(lab, np.array([1, 2, 3])).tolist(), [False, True, False])

    def test_the_area_rule_alone_at_70_north(self):
        res = 0.01  # 1.11 x 0.38 km at 70 N: 0.42 km2 a cell
        lab = np.zeros((100, 100), np.int32)
        lab[10:30, 10:30] = 1  # 400 cells, 170 km2
        lab[50:65, 50:65] = 2  # 225 cells, 95 km2 (at 49 N it would be 164 km2)
        r = datums.over_area(lab, np.array([1, 2]), 70.5, res)
        self.assertEqual(r.tolist(), [True, False])

    def test_a_big_enclosed_body_is_sea_a_small_one_a_lagoon(self):
        res = 0.005  # cells of 0.56 x 0.36 km at 49 N, 0.2 km2
        water = np.zeros((400, 800), bool)
        water[10:390, 10:300] = True
        water[100:300, 500:750] = True  # 50000 cells, about 10000 km2, 1 degree off, not at the edge: open
        water[20:30, 400:410] = True  # 100 cells, 20 km2, 0.5 degree off: a lagoon (under 2% and 5%)
        s = datums.Surface.harmonic([(10.5, 49.0, 0.7)], water, 10.0, 50.0, res, self.none)
        self.assertAlmostEqual(float(s.a[200, 600]), 0.7, places=4)
        self.assertTrue(np.isnan(s.a[25, 405]))

    def test_an_inland_lake_is_left_alone_without_depth_and_does_not_stop(self):
        from shapely.geometry import box

        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:390, 10:300] = True
        water[100:300, 500:750] = True  # as above, but Natural Earth has a lake on it
        lake = box(10 + 500 * res, 50 - 300 * res, 10 + 750 * res, 50 - 100 * res)
        s = datums.Surface.harmonic([(10.5, 49.0, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([lake]))
        self.assertTrue(np.isnan(s.a[100:300, 500:750]).all())
        self.assertAlmostEqual(float(s.a[200, 200]), 0.7, places=4)

    def test_every_body_a_sea_only_chart_extrapolates_tiny_far_bodies_but_not_lakes(self):
        from shapely.geometry import box

        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:60, 10:60] = True  # 2500 cells with the station
        for k in range(30):  # 30 tiny survey-track pieces, 6 cells each, 1-2 degrees off: 6.7% of the water
            water[100 + 8 * k:102 + 8 * k, 400 + 10 * k:403 + 10 * k] = True
        water[200:220, 500:520] = True  # 400 cells: Natural Earth has a lake on it, 13% of the water
        lake = box(10 + 500 * res, 50 - 220 * res, 10 + 520 * res, 50 - 200 * res)
        sea = datums.OpenSea([lake], every_body=True)
        s = datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, sea)
        self.assertTrue(np.isnan(s.a[200:220, 500:520]).all())  # the lake stays nodata, not a stop
        water[200:220, 500:520] = False
        s = datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, sea)
        self.assertAlmostEqual(float(s.a[100, 400]), 0.7, places=4)
        self.assertAlmostEqual(float(s.a[100 + 8 * 29, 400 + 10 * 29]), 0.7, places=4)
        with self.assertRaises(SystemExit):  # without every_body the same pieces are lagoons, over 5%: stops
            datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([]))

    def test_nonna10_chunk_10_lake_superior_far_from_any_station_gets_no_depth_and_does_not_stop(self):
        # NONNA-10's chunk 10 (18 Oct): 47-84 N, Hudson Bay to the Arctic islands and Lake Superior's
        # Thunder Bay; no station within 25 km of the lake's 8705 cells (3% of the water), 97% on
        # Natural Earth's lake, its rim off it. Sea pieces as far take the nearest offset.
        from shapely.geometry import box

        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:60, 10:300] = True  # 14500 cells of sea with the station
        for k in range(20):  # far sea pieces (ice, survey tracks), 6 cells each
            water[100 + 8 * k:102 + 8 * k, 400 + 10 * k:403 + 10 * k] = True
        water[300:320, 500:520] = True  # 400 cells, 80 km2, 2.7% of the water: the lake
        lake = box(10 + 502 * res, 50 - 318 * res, 10 + 518 * res, 50 - 302 * res)  # the rim off the polygon
        s = datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([lake], every_body=True))
        self.assertTrue(np.isnan(s.a[300:320, 500:520]).all())
        self.assertAlmostEqual(float(s.a[100, 400]), 0.7, places=4)
        self.assertAlmostEqual(float(s.a[30, 200]), 0.7, places=4)
        with self.assertRaises(SystemExit) as r:  # without the lakes the same body is a gap in the table
            datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([]))
        self.assertIn("missing area", str(r.exception))

    def test_a_far_sea_body_touching_a_lake_edge_is_still_sea_not_dropped(self):
        # (Bras d'Or, Lake Melville, a river mouth: a lake polygon grazing a sea body)
        from shapely.geometry import box

        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:60, 10:60] = True  # 2500 cells with the station
        water[200:220, 500:520] = True  # 400 cells, 80 km2, far: a sea piece
        lake = box(10 + 510 * res, 50 - 225 * res, 10 + 530 * res, 50 - 205 * res)  # over a quarter of it
        s = datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([lake], every_body=True))
        self.assertAlmostEqual(float(s.a[210, 510]), 0.7, places=4)
        with self.assertRaises(SystemExit) as r:  # not every_body: a small far lagoon, over 2%: stops, not dropped as a lake
            datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([lake]))
        self.assertIn("missing area", str(r.exception))

    def test_a_far_lake_is_subtracted_but_a_far_gap_beside_it_still_stops(self):
        from shapely.geometry import box

        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:60, 10:60] = True  # 2500 cells with the station
        water[200:220, 500:520] = True  # 400 cells: a lake
        water[300:320, 600:620] = True  # 400 cells: no lake, a small lagoon, over 2%
        lake = box(10 + 500 * res, 50 - 220 * res, 10 + 520 * res, 50 - 200 * res)
        with self.assertRaises(SystemExit) as r:
            datums.Surface.harmonic([(10.1, 49.9, 0.7)], water, 10.0, 50.0, res, datums.OpenSea([lake]))
        self.assertIn("400 of", str(r.exception))  # the lake's 400 cells left out of the count

    def test_no_station_within_reach_extrapolates_open_sea_from_the_nearest_station(self):
        water = np.ones((20, 40), bool)
        s = datums.Surface.harmonic([(5.0, 49.0, 0.4), (30.0, 49.0, 2.0)], water, 10.0, 50.0, 0.1, self.none)
        self.assertTrue(np.allclose(s.a, 0.4))

    def test_a_large_lagoon_beyond_reach_still_stops(self):
        res = 0.005
        water = np.zeros((400, 800), bool)
        water[10:60, 10:60] = True  # 2500 cells
        water[100:110, 500:600] = True  # 1000 cells, 200 km2: open sea, extrapolated
        water[200:220, 500:520] = True  # 400 cells, 80 km2: under OPEN_KM2, over 2% of the water: stops
        with self.assertRaises(SystemExit) as r:
            datums.Surface.harmonic([(10.5, 49.0, 0.7)], water, 10.0, 50.0, res, self.none)
        self.assertIn("missing area", str(r.exception))

    def test_natural_earth_reads_the_lakes_table(self):
        import sqlite3
        import tempfile

        from shapely.geometry import box

        with tempfile.TemporaryDirectory() as d:
            p = f"{d}/ne.sqlite"
            with sqlite3.connect(p) as db:
                db.execute("create table ne_10m_lakes (GEOMETRY blob, name text)")
                db.execute("insert into ne_10m_lakes values (?, 'Great Slave Lake')", (box(-117, 61, -109, 63).wkb,))
            sea = datums.OpenSea.natural_earth(p)
            m = sea.mask((40, 40), -118.0, 64.0, 0.25)
        self.assertTrue(m[8, 8] and not m[0, 0])


class Harmonic(unittest.TestCase):
    """The reach and the lagoon rule, without open sea (the default: every caller but NONNA-10's run)."""
    water = np.ones((20, 40), bool)  # 0.1 degree cells, west 10, north 50: 10..14 E, 48..50 N

    def solve(self, pts):
        return datums.Surface.harmonic(pts, self.water, 10.0, 50.0, 0.1)

    def test_a_station_in_the_grid_sets_its_cell(self):
        s = self.solve([(11.0, 49.0, 1.5)])
        self.assertAlmostEqual(float(np.nanmax(s.a)), 1.5, places=5)

    def test_a_station_24_km_off_the_water_counts_and_26_km_stops(self):
        # cell centres are at 10.05 E; a degree of longitude is 73 km at 49 N
        s = self.solve([(10.05 - 24 / 73.0, 49.0, 2.0)])
        self.assertAlmostEqual(float(np.nanmin(s.a)), 2.0, places=5)
        with self.assertRaises(SystemExit):
            self.solve([(10.05 - 26 / 73.0, 49.0, 2.0)])

    def test_a_diagonal_corner_is_measured_in_km_not_as_a_box(self):
        # 20 km west and 20 km north of the corner cell (10.05 E, 49.95 N): inside a 25 km box, 28 km away
        lat = 49.95 + 20 / 111.32
        lon = 10.05 - 20 / (111.32 * np.cos(np.radians(lat)))
        with self.assertRaises(SystemExit):
            self.solve([(lon, lat, 1.0)])
        lat = 49.95 + 16 / 111.32  # 16 and 16: 22.6 km, counts
        lon = 10.05 - 16 / (111.32 * np.cos(np.radians(lat)))
        self.solve([(lon, lat, 1.0)])

    def test_a_land_station_pins_the_basin_nearest_it_not_the_one_across_a_headland(self):
        water = np.ones((20, 40), bool)
        water[:, 18:22] = False  # a land strip between two basins
        water[:, 0:2] = water[:, 38:40] = False
        # on the strip, nearer the east basin (east basin edge at col 22)
        s = datums.Surface.harmonic([(10 + 21.5 * 0.1, 49.0, 3.0), (10 + 5 * 0.1, 48.5, 1.0)], water, 10.0, 50.0, 0.1)
        self.assertAlmostEqual(float(s.a[10, 30]), 3.0, places=4)
        self.assertAlmostEqual(float(s.a[10, 8]), 1.0, places=4)
        # the west basin has no station within 25 km (its edge at col 17 is 47 km off) and that is 50% of the water: stops
        with self.assertRaises(SystemExit) as r:
            datums.Surface.harmonic([(10 + 21.5 * 0.1, 49.0, 3.0)], water, 10.0, 50.0, 0.1)
        self.assertIn("missing area", str(r.exception))

    def test_a_water_body_without_a_station_within_reach_is_logged_and_filled(self):
        water = np.zeros((20, 40), bool)
        water[:, 0:10] = True
        water[:, 12:14] = True  # a small body 2 cells (about 15 km) from the first
        s = datums.Surface.harmonic([(10.5, 49.0, 1.0)], water, 10.0, 50.0, 0.1)
        self.assertAlmostEqual(float(s.a[5, 12]), 1.0, places=4)

    def test_a_far_lagoon_gets_nodata_while_the_rest_converts(self):
        water = np.zeros((40, 400), bool)
        water[:, 0:300] = True
        water[18:20, 380:382] = True  # a small lagoon about 8 degrees east of the sea
        s = datums.Surface.harmonic([(10.5, 48.0, 1.0)], water, 10.0, 50.0, 0.1)
        self.assertTrue(np.isnan(s.a[18, 380]))
        self.assertAlmostEqual(float(s.a[5, 5]), 1.0, places=4)
        self.assertAlmostEqual(float(s.a[5, 350]), 1.0, places=4)  # land between: the nearest value

    def test_a_missing_area_of_the_table_stops_not_nodata(self):
        water = np.zeros((40, 400), bool)
        water[:, 0:300] = True
        water[:, 330:400] = True  # 70 columns of water, far from any station: 19% of the water
        with self.assertRaises(SystemExit) as r:
            datums.Surface.harmonic([(10.5, 48.0, 1.0)], water, 10.0, 50.0, 0.1)
        self.assertIn("missing area", str(r.exception))

    def test_nodata_does_not_bleed_into_the_solved_cells_next_to_it(self):
        from grid import Window
        from rasterio.warp import transform_bounds

        a = np.full((10, 10), 2.0, "float32")
        a[:, 5:] = np.nan  # solved 10.0..10.5 E, nodata 10.5..11.0 E (0.1 degree cells)
        win = Window.around(9, transform_bounds("EPSG:4326", "EPSG:3857", 10.0, 49.0, 11.0, 50.0))
        out = datums.Surface(a, 10.0, 50.0, 0.1).on(win)
        self.assertTrue(np.isnan(out).any())
        self.assertTrue(np.isfinite(out).any())
        self.assertTrue(np.all(out[np.isfinite(out)] == 2.0))  # NaN neighbours would pull a bilinear value off 2

    def test_no_station_near_the_water_stops_saying_so(self):
        # the Pacific coast chunk of BlueTopo: the table's nearest rows were thousands of km away
        with self.assertRaises(SystemExit) as r:
            self.solve([(-80.0, 30.0, 0.1), (-137.9, 58.4, 1.4)])
        self.assertIn("needs a station within", str(r.exception))
        self.assertIn("km from its nearest water cell", str(r.exception))

    def test_no_water_or_no_stations_still_stops(self):
        with self.assertRaises(SystemExit):
            datums.Surface.harmonic([(11.0, 49.0, 1.0)], np.zeros((5, 5), bool), 10.0, 50.0, 0.1)
        with self.assertRaises(SystemExit):
            self.solve([])


if __name__ == "__main__":
    unittest.main()
