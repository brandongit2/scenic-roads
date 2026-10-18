"""uv run --project .. python -m unittest test_vdatum"""
import unittest

from vdatum import parse, query


class VDatumTest(unittest.TestCase):
    def test_westcoast_query(self):
        q = query(-122.625, 37.625, "NAVD88", "LMSL")
        self.assertEqual((q["region"], q["s_h_frame"], q["t_h_frame"]), ("westcoast", "NAD83_2011", "IGS14"))
        q = query(-122.625, 37.625, "LMSL", "MLLW")
        self.assertEqual((q["s_h_frame"], q["t_h_frame"]), ("IGS14", "IGS14"))

    def test_other_regions(self):
        self.assertEqual(query(-80.0, 30.0, "NAVD88", "LMSL")["region"], "contiguous")
        self.assertEqual(query(-66.0, 18.0, "NAVD88", "LMSL")["region"], "prvi")
        q = query(-134.0, 57.0, "LMSL", "MLLW")
        self.assertEqual((q["region"], q["s_h_frame"]), ("seak", "IGS08"))

    def test_parse(self):
        self.assertEqual(parse({"t_z": "-0.915"}), (-0.915, None))
        z, why = parse({"errorCode": 412, "message": "Uncaught error"})
        self.assertIsNone(z)
        self.assertIn("412", why)
        self.assertIsNone(parse({"t_z": "-999999"})[0])
        self.assertIsNone(parse({})[0])


if __name__ == "__main__":
    unittest.main()
