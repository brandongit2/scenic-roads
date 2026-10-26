"""chunked.py's and ncei.py's checks, with fetching stubbed. Run from producers/depth:
uv run --project .. python -m unittest test_chunked"""
from __future__ import annotations

import io
import shutil
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

    def tiff(self, body=b"II*\x00rest"):
        d = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, d, True)
        p = d / "b.tif"
        p.write_bytes(body)
        return p

    def test_a_transient_failure_is_tried_again(self):
        ok = self.tiff()
        answers = [http(503), http(429), ConnectionResetError(), ok]

        def fetch(url, dest, pause):
            a = answers.pop(0)
            if isinstance(a, BaseException):
                raise a
            return a

        with mock.patch.object(chunked, "fetch", fetch):
            self.assertEqual(chunked.fetch_retry("u", Path("d"), 0), ok)
        self.assertEqual(self.sleep.call_count, 3)

    def test_an_exception_report_in_place_of_a_tiff_is_tried_again(self):
        bad = self.tiff(b'<?xml version="1.0"?><ows:ExceptionReport>Connection reset</ows:ExceptionReport>')
        ok = self.tiff()
        answers = [bad, ok]
        with mock.patch.object(chunked, "fetch", lambda url, dest, pause: answers.pop(0)):
            self.assertEqual(chunked.fetch_retry("u", Path("d"), 0), ok)
        self.assertFalse(bad.exists())
        self.assertEqual(self.sleep.call_count, 1)

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


def tile(path, col0, row0, cols, rows, res=None, nodata=None, shift=(0.0, 0.0), value=None):
    """A float32 tif of `cols` × `rows` pixels of the service's grid starting at column `col0`, row `row0`
    (each cell holding its global column index, or `value`); `res`, `nodata`, `shift` (pixels) spoil it."""
    import numpy as np
    import rasterio
    from rasterio.transform import from_origin

    rx, ry = res or (chunked.GRID_RX, chunked.GRID_RY)
    x0 = chunked.GRID_X0 + (col0 + shift[0]) * chunked.GRID_RX
    y1 = chunked.GRID_Y1 - (row0 + shift[1]) * chunked.GRID_RY
    data = np.tile(np.arange(col0, col0 + cols, dtype="float32"), (1, rows, 1)) if value is None else np.full((1, rows, cols), value, "float32")
    with rasterio.open(path, "w", driver="GTiff", height=rows, width=cols, count=1, dtype="float32",
                       nodata=chunked.NODATA if nodata is None else nodata, transform=from_origin(x0, y1, rx, ry), crs="EPSG:3857") as o:
        o.write(data)
    return Path(path)


class SplitOn504(unittest.TestCase):
    """fetch_split: a block the server can't render splits in 4 and the quarters merge."""
    COLS, ROWS = 200, 100
    BOX = (chunked.GRID_X0, chunked.GRID_Y1 - 100 * chunked.GRID_RY, chunked.GRID_X0 + 200 * chunked.GRID_RX, chunked.GRID_Y1)

    def setUp(self):
        mock.patch.object(chunked.time, "sleep").start()
        mock.patch.object(chunked, "MIN_SPLIT_M", 100.0).start()
        self.addCleanup(mock.patch.stopall)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.dest = self.dir / "b.tif"
        self.urls = []

    def stub(self, fails, code=504):
        """fetch for fetch_retry: like grid.fetch keeps a file that is there, else writes the subset in the
        URL (on the service's grid), unless `fails(x0, y0, x1, y1)` says it fails with HTTP `code`."""
        import re

        def fetch(url, dest, pause):
            if dest.exists():
                return dest
            x0, x1 = (float(v) for v in re.search(r"subset=X\(([-\d.]+),([-\d.]+)\)", url).groups())
            y0, y1 = (float(v) for v in re.search(r"subset=Y\(([-\d.]+),([-\d.]+)\)", url).groups())
            self.urls.append((x0, y0, x1, y1))
            if fails(x0, y0, x1, y1):
                raise http(code)
            c0, r0 = round((x0 - chunked.GRID_X0) / chunked.GRID_RX), round((chunked.GRID_Y1 - y1) / chunked.GRID_RY)
            return tile(dest, c0, r0, round((x1 - x0) / chunked.GRID_RX), round((y1 - y0) / chunked.GRID_RY))

        return mock.patch.object(chunked, "fetch", fetch)

    def test_a_504_block_splits_and_merges(self):
        import rasterio

        with self.stub(lambda x0, y0, x1, y1: x1 - x0 > 2500):
            chunked.fetch_split("cov", self.BOX, self.dest, 0)
        with rasterio.open(self.dest) as d:
            self.assertEqual((d.width, d.height), (self.COLS, self.ROWS))
            self.assertLess(max(abs(a - b) for a, b in zip(d.bounds, self.BOX)), 1e-3)
            self.assertEqual(d.read(1)[0].tolist(), list(map(float, range(self.COLS))))
            self.assertEqual(d.nodata, chunked.NODATA)
        self.assertEqual(sorted(p.name for p in self.dir.iterdir()), ["b.tif"])

    def test_two_504s_in_a_row_are_the_trigger(self):
        with self.stub(lambda x0, y0, x1, y1: x1 - x0 > 2500):
            chunked.fetch_split("cov", self.BOX, self.dest, 0)
        self.assertEqual(len(self.urls), 2 + 4)  # the block twice (not five times), then the 4 quarters

    def test_504s_split_by_other_failures_are_not(self):
        answers = [http(504), http(502), http(504), http(504)]
        with mock.patch.object(chunked, "fetch", side_effect=answers) as f, self.assertRaises(chunked.Unrenderable):
            chunked.fetch_retry("u", Path("d"), 0)
        self.assertEqual(f.call_count, 4)

    def test_a_quarter_failing_at_the_minimum_size_stops_the_run(self):
        with self.stub(lambda *b: True), mock.patch.object(chunked, "MIN_SPLIT_M", 600.0), \
                self.assertRaises(chunked.Unrenderable):
            chunked.fetch_split("cov", self.BOX, self.dest, 0)
        # (3.8 km × 1.9 km: the block 2 tries, its first quarter 2; that one's half, 478 m, is under 600.)
        self.assertEqual(len(self.urls), 4)
        self.assertFalse(self.dest.exists())

    def test_another_http_error_does_not_split(self):
        with self.stub(lambda *b: True, code=404), self.assertRaises(SystemExit) as r:
            chunked.fetch_split("cov", self.BOX, self.dest, 0)
        self.assertNotIsInstance(r.exception, chunked.Unrenderable)
        self.assertEqual(len(self.urls), 1)

    def test_a_resumed_run_does_not_retry_the_block(self):
        import rasterio

        q0 = self.dir / "b_q0.tif"
        tile(q0, 0, 50, 100, 50, value=7.0)  # (a leftover; kept, not fetched again)
        with self.stub(lambda *b: False):
            chunked.fetch_split("cov", self.BOX, self.dest, 0)
        self.assertEqual(len(self.urls), 3)  # the other quarters only
        self.assertNotIn(self.BOX, self.urls)
        with rasterio.open(self.dest) as d:
            self.assertEqual(d.read(1)[-1, 0], 7.0)
            self.assertEqual(d.read(1)[0, 0], 0.0)

    def test_an_on_grid_box_is_unchanged(self):
        box = (chunked.GRID_X0 + 5 * chunked.GRID_RX, chunked.GRID_Y1 - 40 * chunked.GRID_RY,
               chunked.GRID_X0 + 77 * chunked.GRID_RX, chunked.GRID_Y1 - 3 * chunked.GRID_RY)
        for a, b in zip(chunked.on_grid(box), box):
            self.assertAlmostEqual(a, b, places=6)

    def test_outer_edges_snap_outward(self):
        x0, y0, x1, y1 = chunked.on_grid((chunked.GRID_X0 + 5.3 * chunked.GRID_RX, chunked.GRID_Y1 - 40.6 * chunked.GRID_RY,
                                           chunked.GRID_X0 + 77.2 * chunked.GRID_RX, chunked.GRID_Y1 - 3.4 * chunked.GRID_RY))
        self.assertAlmostEqual((x0 - chunked.GRID_X0) / chunked.GRID_RX, 5, places=6)
        self.assertAlmostEqual((chunked.GRID_Y1 - y0) / chunked.GRID_RY, 41, places=6)
        self.assertAlmostEqual((x1 - chunked.GRID_X0) / chunked.GRID_RX, 78, places=6)
        self.assertAlmostEqual((chunked.GRID_Y1 - y1) / chunked.GRID_RY, 3, places=6)

    def test_the_midpoint_snaps_the_same_way_at_every_level(self):
        """Each level's midpoint is the halfway pixel line, in X and in Y alike."""
        lo, hi = 3, 3 + 1024  # columns (halving never lands between lines)
        while hi - lo > 1:
            m = chunked.mid_on_grid(chunked.GRID_X0 + lo * chunked.GRID_RX, chunked.GRID_X0 + hi * chunked.GRID_RX)
            col = (m - chunked.GRID_X0) / chunked.GRID_RX
            self.assertAlmostEqual(col, round(col), places=6)
            self.assertEqual(round(col), (lo + hi) // 2)
            ym = chunked.mid_on_grid(chunked.GRID_Y1 - lo * chunked.GRID_RY, chunked.GRID_Y1 - hi * chunked.GRID_RY, y=True)
            row = (chunked.GRID_Y1 - ym) / chunked.GRID_RY
            self.assertEqual(round(row), round(col))
            hi = round(col)

    def mosaic_of(self, *tiles):
        subs = []
        for i, t in enumerate(tiles):
            subs.append(tile(self.dir / f"q{i}.tif", *t[:4], **t[4]))
        return chunked.mosaic(subs, self.dest)

    def test_aligned_quarters_merge(self):
        self.mosaic_of((0, 0, 10, 5, {}), (10, 0, 10, 5, {}), (0, 5, 10, 5, {}), (10, 5, 10, 5, {}))
        self.assertTrue(self.dest.exists())

    def test_quarters_off_the_grid_fail_loudly(self):
        with self.assertRaises(SystemExit) as r:
            self.mosaic_of((0, 0, 10, 5, {}), (10, 0, 10, 5, {"shift": (0.5, 0)}))
        self.assertIn("pixel", str(r.exception))

    def test_overlapping_quarters_fail_loudly(self):
        with self.assertRaises(SystemExit) as r:
            self.mosaic_of((0, 0, 10, 5, {}), (9, 0, 10, 5, {}))
        self.assertIn("overlaps", str(r.exception))

    def test_a_gap_fails_loudly(self):
        with self.assertRaises(SystemExit) as r:
            self.mosaic_of((0, 0, 10, 5, {}), (11, 0, 10, 5, {}))
        self.assertIn("gap", str(r.exception))

    def test_a_different_resolution_fails_loudly(self):
        with self.assertRaises(SystemExit) as r:
            self.mosaic_of((0, 0, 10, 5, {}), (10, 0, 10, 5, {"res": (chunked.GRID_RX * 1.01, chunked.GRID_RY)}))
        self.assertIn("resolution", str(r.exception))

    def test_another_nodata_fails_loudly(self):
        with self.assertRaises(SystemExit) as r:
            self.mosaic_of((0, 0, 10, 5, {"nodata": -9999.0}), (10, 0, 10, 5, {}))
        self.assertIn("nodata", str(r.exception))

    def test_the_estimate_leaves_quarters_out(self):
        cache = self.dir
        (cache / "nonna10_a.tif").write_bytes(b"x" * 1000)
        (cache / "nonna10_a_q0.tif").write_bytes(b"x" * 9000)
        self.assertIn("(0.0 MB an item, from 1 in the cache)", chunked.estimate(cache, "nonna10", 1))


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


class Egm2008Table(unittest.TestCase):
    # (scenic-build geoid-offsets' lines, 26 Oct 2026.)
    OUT = ("-74.0000,40.0000  NAVD88→EGM2008 -0.388 m  (NAVD88→ellipsoid-NAD83 -33.080, ellipsoid-NAD83→ellipsoid-WGS84 -1.275, ellipsoid-WGS84→EGM2008 +33.966)\n"
           "-81.0000,25.5000  NAVD88→EGM2008 +0.047 m  (…)\n"
           "timings: geoid-offsets done in 725 ms\n")

    def test_the_separations_are_read(self):
        self.assertEqual(ncei.separations(self.OUT), {(-74.0, 40.0): -0.388, (-81.0, 25.5): 0.047})

    def test_msl_above_egm2008_is_msl_above_navd88_plus_navd88_above_egm2008(self):
        rows = [{"id": "a", "name": "A", "lon": "-74", "lat": "40", "msl_minus_navd88_m": "0.1"},
                {"id": "b", "name": "B", "lon": "-81.0", "lat": "25.5", "msl_minus_navd88_m": "-0.2"},
                {"id": "c", "name": "C", "lon": "-70", "lat": "40", "msl_minus_navd88_m": "0.3"}]
        out = ncei.egm2008_rows(rows, ncei.separations(self.OUT))
        self.assertEqual([(r["id"], r[ncei.COLUMN]) for r in out], [("a", -0.288), ("b", -0.153)])


if __name__ == "__main__":
    unittest.main()
