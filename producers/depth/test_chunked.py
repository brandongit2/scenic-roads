"""chunked.py's and ncei.py's checks, with fetching stubbed. Run from producers/depth:
uv run --project .. python -m unittest test_chunked"""
from __future__ import annotations

import io
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

import chunked
import ncei


def http(code):
    return urllib.error.HTTPError("https://x/", code, "x", {}, io.BytesIO())


class FetchRetry(unittest.TestCase):
    def setUp(self):
        self.sleep = mock.patch.object(chunked.time, "sleep").start()
        self.addCleanup(mock.patch.stopall)

    def test_a_transient_failure_is_tried_again(self):
        answers = [http(503), http(429), ConnectionResetError(), Path("/ok")]

        def fetch(url, dest, pause):
            a = answers.pop(0)
            if isinstance(a, BaseException):
                raise a
            return a

        with mock.patch.object(chunked, "fetch", fetch):
            self.assertEqual(chunked.fetch_retry("u", Path("d"), 0), Path("/ok"))
        self.assertEqual(self.sleep.call_count, 3)

    def test_another_http_answer_stops_the_run(self):
        with mock.patch.object(chunked, "fetch", side_effect=http(404)), self.assertRaises(SystemExit):
            chunked.fetch_retry("u", Path("d"), 0)
        self.assertEqual(self.sleep.call_count, 0)

    def test_failing_every_try_stops_the_run(self):
        with mock.patch.object(chunked, "fetch", side_effect=http(502)), self.assertRaises(SystemExit):
            chunked.fetch_retry("u", Path("d"), 0, tries=3)

    def test_a_block_failing_writes_no_part(self):
        """nonna10_chunk raises (no part written, the cache kept) rather than leaving the block out."""
        produced = []
        with mock.patch.object(chunked, "fetch", side_effect=http(503)), \
                mock.patch("raster.produce", lambda *a: produced.append(a)), self.assertRaises(SystemExit):
            chunked.nonna10_chunk([(-60.0, 45.0, -59.5, 45.3)], "t.csv", Path("cache"), None)
        self.assertEqual(produced, [])


class SplitOn504(unittest.TestCase):
    """fetch_split: a block the server can't render splits in 4 and the quarters merge."""
    BOX = (0.0, 0.0, 8000.0, 4000.0)

    def setUp(self):
        mock.patch.object(chunked.time, "sleep").start()
        self.addCleanup(mock.patch.stopall)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dest = Path(self.tmp.name) / "b.tif"
        self.urls = []

    def stub(self, fails, code=504):
        """fetch for fetch_retry: writes a 2x2-pixel tif of the subset in the URL (value = its x0 / 1000),
        unless `fails(x0, y0, x1, y1)` says it fails with HTTP `code`."""
        import re

        import numpy as np
        import rasterio
        from rasterio.transform import from_bounds

        def fetch(url, dest, pause):
            x0, x1 = (float(v) for v in re.search(r"subset=X\(([-\d.]+),([-\d.]+)\)", url).groups())
            y0, y1 = (float(v) for v in re.search(r"subset=Y\(([-\d.]+),([-\d.]+)\)", url).groups())
            self.urls.append((x0, y0, x1, y1))
            if fails(x0, y0, x1, y1):
                raise http(code)
            with rasterio.open(dest, "w", driver="GTiff", height=2, width=2, count=1, dtype="float32", nodata=chunked.NODATA,
                               transform=from_bounds(x0, y0, x1, y1, 2, 2), crs="EPSG:3857") as o:
                o.write(np.full((1, 2, 2), x0 / 1000, "float32"))
            return dest

        return mock.patch.object(chunked, "fetch", fetch)

    def test_a_504_block_splits_and_merges(self):
        import rasterio

        with self.stub(lambda x0, y0, x1, y1: x1 - x0 > 5000):
            chunked.fetch_split("cov", self.BOX, self.dest, 0, tries=2)
        self.assertEqual(len(self.urls), 2 + 4)  # the whole block twice, then 4 quarters
        with rasterio.open(self.dest) as d:
            self.assertEqual((d.width, d.height), (4, 4))
            self.assertEqual(tuple(d.bounds), self.BOX)
            self.assertEqual(d.read(1)[0].tolist(), [0.0, 0.0, 4.0, 4.0])
        self.assertEqual(sorted(p.name for p in self.dest.parent.iterdir()), ["b.tif"])

    def test_a_quarter_failing_at_the_minimum_size_stops_the_run(self):
        with self.stub(lambda *b: True), mock.patch.object(chunked, "MIN_SPLIT_M", 500.0), \
                self.assertRaises(chunked.Unrenderable):
            chunked.fetch_split("cov", (0.0, 0.0, 2000.0, 2000.0), self.dest, 0, tries=1)
        self.assertEqual(len(self.urls), 3)  # the block, its first quarter, that one's first: 250 m is under the minimum
        self.assertFalse(self.dest.exists())

    def test_another_http_error_does_not_split(self):
        with self.stub(lambda *b: True, code=404), self.assertRaises(SystemExit) as r:
            chunked.fetch_split("cov", self.BOX, self.dest, 0, tries=2)
        self.assertNotIsInstance(r.exception, chunked.Unrenderable)
        self.assertEqual(len(self.urls), 1)


class NasCheck(unittest.TestCase):
    def test_each_files_size_is_compared(self):
        listing = ("total 8\n-rw-r--r--  1 u  g  10 Oct 17 20:00 a.tif\n"
                   "-rw-r--r--  1 u  g  25 Oct 17 20:00 b c.tif\ndrwxr-xr-x  2 u  g  0 Oct 17 20:00 sub\n")
        self.assertEqual(chunked.size_mismatches({"a.tif": 10, "b c.tif": 25}, listing), [])
        # (The same total, one file short and one long: caught.)
        self.assertEqual(chunked.size_mismatches({"a.tif": 15, "b c.tif": 20}, listing),
                         ["a.tif: 10 of 15", "b c.tif: 25 of 20"])
        self.assertEqual(chunked.size_mismatches({"d.tif": 1}, listing), ["d.tif: None of 1"])

    def test_the_estimate_reads_the_cache(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertIn("unknown", chunked.estimate(Path(d), "nonna10", 300))
            Path(d, "nonna10_1.tif").write_bytes(b"x" * 2_000_000)
            self.assertIn("0.6 GB", chunked.estimate(Path(d), "nonna10", 300))


class LowWaterDatum(unittest.TestCase):
    # (Marquette's answer, 9099018, 17 Oct 2026.)
    D = {"units": "meters", "datums": [{"name": "GL_LWD", "value": 183.2}], "min": 182.63, "max": 184.119}

    def test_the_igld_elevation(self):
        self.assertEqual(ncei.low_water_datum(self.D, 183.413), 183.2)

    def test_relative_to_a_station_datum_with_igld_beside_it(self):
        d = dict(self.D, datums=[{"name": "GL_LWD", "value": 3.2}, {"name": "IGLD", "value": -180.0}], min=2.63, max=4.119)
        self.assertAlmostEqual(ncei.low_water_datum(d, 183.413), 183.2)

    def test_anything_else_fails(self):
        for d in [dict(self.D, units="feet"),
                  dict(self.D, datums=[{"name": "GL_LWD", "value": 3.2}, {"name": "STND", "value": 0.0}]),
                  dict(self.D, datums=[{"name": "GL_LWD", "value": 3.2}], min=2.63, max=4.119)]:
            with self.assertRaises(SystemExit):
                ncei.low_water_datum(d, 183.413)


if __name__ == "__main__":
    unittest.main()
