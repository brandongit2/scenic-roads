"""bldfetch.Near: which boxes come within the margin of the coverage (task #493)."""
import unittest

from shapely.geometry import Polygon, box

from bldfetch import Near

KM = 1 / 111.32  # a km of latitude, in degrees


class NearTest(unittest.TestCase):
    def test_within_and_just_beyond_the_margin(self):
        near = Near([box(10.0, 45.0, 10.1, 45.1)], 20.0)
        # North of it: latitude's degrees are the same everywhere.
        self.assertTrue(near.meets(10.0, 45.1 + 19 * KM, 10.1, 45.2 + 19 * KM))
        self.assertFalse(near.meets(10.0, 45.1 + 30 * KM, 10.1, 45.2 + 30 * KM))
        self.assertFalse(near.meets(12.0, 45.0, 12.1, 45.1))

    def test_high_latitude(self):
        # At 70° a degree of longitude is ~38 km: 15 km east is within 20 km.
        near = Near([box(20.0, 70.0, 20.1, 70.1)], 20.0)
        self.assertTrue(near.meets(20.1 + 0.4, 70.0, 20.6, 70.1))
        self.assertFalse(near.meets(22.0, 70.0, 22.1, 70.1))

    def test_near_the_pole_every_longitude(self):
        # Northern Svalbard's tip past 80.8°: within 20 km of the pole, every longitude is near.
        near = Near([box(20.0, 89.9, 20.1, 89.95)], 20.0)
        self.assertTrue(near.meets(-160.0, 89.9, -159.9, 89.95))
        # 81°: a degree of longitude ~17 km; a box 1° away across the meridians is within 20 km.
        near = Near([box(20.0, 81.0, 20.1, 81.1)], 20.0)
        self.assertTrue(near.meets(21.0, 81.0, 21.1, 81.1))
        # Nothing near the pole: a box there meets nothing.
        near = Near([box(20.0, 70.0, 20.1, 70.1)], 20.0)
        self.assertFalse(near.meets(-160.0, 89.9, -159.9, 89.95))

    def test_across_the_antimeridian(self):
        near = Near([box(179.8, 52.0, 179.95, 52.1)], 20.0)
        self.assertTrue(near.meets(-179.95, 52.0, -179.8, 52.1))
        near = Near([box(-179.95, 52.0, -179.8, 52.1)], 20.0)
        self.assertTrue(near.meets(179.8, 52.0, 179.95, 52.1))
        self.assertFalse(near.meets(170.0, 52.0, 170.1, 52.1))

    def test_an_invalid_polygon(self):
        bowtie = Polygon([(0, 0), (1, 1), (1, 0), (0, 1), (0, 0)])
        self.assertFalse(bowtie.is_valid)
        near = Near([bowtie], 20.0)
        # Both lobes count: west of the left one, and inside the right one.
        self.assertTrue(near.meets(-0.1 - 10 * KM, 0.45, -10 * KM, 0.55))
        self.assertTrue(near.meets(0.9, 0.45, 0.95, 0.55))
        self.assertTrue(near.meets(1.0 + 10 * KM, 0.45, 1.1, 0.55))
        self.assertFalse(near.meets(3.0, 3.0, 3.1, 3.1))

    def test_a_zero_area_outline(self):
        # A ring folded on itself: made valid, a line, which still answers distances.
        flat = Polygon([(0, 0), (1, 0), (2, 0), (0, 0)])
        self.assertFalse(flat.is_valid)
        near = Near([flat], 20.0)
        self.assertTrue(near.meets(0.5, 10 * KM, 0.6, 0.2))
        self.assertFalse(near.meets(0.5, 1.0, 0.6, 1.1))


if __name__ == "__main__":
    unittest.main()
