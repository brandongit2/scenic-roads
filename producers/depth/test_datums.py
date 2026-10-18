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

    def test_a_station_just_off_the_edge_counts(self):
        s = self.solve([(9.6, 49.0, 2.0)])
        self.assertAlmostEqual(float(np.nanmin(s.a)), 2.0, places=5)

    def test_no_station_near_the_water_stops_saying_so(self):
        # the Pacific coast chunk of BlueTopo: the table's nearest rows were thousands of km away
        with self.assertRaises(SystemExit) as r:
            self.solve([(-80.0, 30.0, 0.1), (-137.9, 58.4, 1.4)])
        self.assertIn("needs a station within", str(r.exception))
        self.assertIn("km from its centre", str(r.exception))

    def test_no_water_or_no_stations_still_stops(self):
        with self.assertRaises(SystemExit):
            datums.Surface.harmonic([(11.0, 49.0, 1.0)], np.zeros((5, 5), bool), 10.0, 50.0, 0.1)
        with self.assertRaises(SystemExit):
            self.solve([])


if __name__ == "__main__":
    unittest.main()
