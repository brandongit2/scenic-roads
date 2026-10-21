"""near.Near: which boxes come within the margin of the coverage (task #493); near.EvenOdd: which
meet railfeeds' coverage (task #498)."""
import unittest

from shapely.geometry import Polygon, box

from near import EvenOdd, Near

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


def square(x, y, d):
    return [[x, y], [x + d, y], [x + d, y + d], [x, y + d], [x, y]]


class EvenOddTest(unittest.TestCase):
    def test_a_ring_inside_another_is_a_hole(self):
        cov = EvenOdd([([square(0, 0, 10), square(4, 4, 2)], 0.0)])
        self.assertTrue(cov.meets(1, 1, 2, 2))
        self.assertFalse(cov.meets(4.5, 4.5, 5.5, 5.5))  # in the hole, no edge met
        self.assertTrue(cov.meets(3, 3, 4.5, 4.5))  # the hole's edge met
        self.assertFalse(cov.meets(11, 11, 12, 12))
        self.assertTrue(cov.meets(-5, -5, 20, 20))  # the box around it all

    def test_overlapping_rings_cancel_but_other_shapes_dont(self):
        # Two overlapping rings of one shape: their overlap outside; another shape's ring there in.
        two = [square(0, 0, 4), square(2, 2, 4)]
        self.assertFalse(EvenOdd([(two, 0.0)]).meets(2.9, 2.9, 3.1, 3.1))
        self.assertTrue(EvenOdd([(two, 0.0), ([square(2.5, 2.5, 1)], 0.0)]).meets(2.9, 2.9, 3.1, 3.1))

    def test_a_ring_of_many_pieces(self):
        # A ring of a thousand points (many pieces): a box deep inside it is in, beyond it out.
        import math
        ring = [[10 * math.cos(2 * math.pi * i / 1000), 10 * math.sin(2 * math.pi * i / 1000)] for i in range(1000)]
        cov = EvenOdd([([ring], 0.0)])
        self.assertTrue(cov.meets(-1, -1, 1, 1))
        self.assertFalse(cov.meets(8, 8, 9, 9))

    def test_the_buffer_in_metres(self):
        cov = EvenOdd([([square(10.0, 45.0, 0.1)], 1000.0)])
        self.assertTrue(cov.meets(10.1 + 0.5 * KM, 45.0, 10.2, 45.05))
        self.assertFalse(cov.meets(10.1 + 2 * KM, 45.0, 10.2, 45.05))
        self.assertFalse(EvenOdd([([square(10.0, 45.0, 0.1)], 0.0)]).meets(10.1 + 0.5 * KM, 45.0, 10.2, 45.05))

    def test_the_buffer_reaches_a_corner_as_rusts_rectangle(self):
        # Rust grows the box by a rectangle: 0.0075° diagonally off the NE corner of a unit square
        # (about 1.06 km at 0°, beyond a round 1000 m) still meets it.
        cov = EvenOdd([([square(0.0, 0.0, 1.0)], 1000.0)])
        self.assertTrue(cov.meets(1.0075, 1.0075, 1.1, 1.1))
        self.assertFalse(cov.meets(1.01, 1.01, 1.1, 1.1))

    def test_a_grown_box_across_the_antimeridian(self):
        cov = EvenOdd([([square(179.9, 10.0, 0.0999)], 1000.0)])
        self.assertTrue(cov.meets(-180.0, 10.0, -179.9, 10.05))

    def test_a_box_past_the_pole_meets_only_what_it_covers(self):
        # (The catalogue has boxes past ±90°: with no margin, not every longitude.)
        cov = EvenOdd([([square(3.0, 49.0, 0.5)], 0.0)])
        self.assertTrue(cov.meets(3.0, 48.9, 4.3, 2457338.0))
        self.assertFalse(cov.meets(120.0, 48.9, 121.0, 2457338.0))


if __name__ == "__main__":
    unittest.main()
