"""datums.py's checks. Run from producers/depth: python -m unittest test_datums"""
from __future__ import annotations

import unittest

import numpy as np

import datums


class Harmonic(unittest.TestCase):
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
