"""globathy.py's checks. Run from producers/depth: uv run --project .. python -m unittest test_globathy"""
from __future__ import annotations

import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

import numpy as np

import globathy
from grid import Writer


def hydrolakes_zip(path: Path, lakes):
    """A HydroLAKES-shaped zip: (Hylak_id, rings in lon/lat) each."""
    import shapefile

    shp, shx, dbf = io.BytesIO(), io.BytesIO(), io.BytesIO()
    with shapefile.Writer(shp=shp, shx=shx, dbf=dbf, shapeType=shapefile.POLYGON) as w:
        w.field("Hylak_id", "N", 10)
        for hid, rings in lakes:
            w.poly(rings)
            w.record(hid)
    with zipfile.ZipFile(path, "w") as z:
        for ext, b in ((".shp", shp), (".shx", shx), (".dbf", dbf)):
            z.writestr(f"HydroLAKES_polys_v10_shp/HydroLAKES_polys_v10{ext}", b.getvalue())


def params_zip(path: Path, rows):
    lines = ["Hylak_id,Pour_long,Pour_lat,Dmax_use_m"] + [f"{h},{x},{y},{d}" for h, x, y, d in rows]
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("GLOBathy_basic_parameters/GLOBathy_basic_parameters(ALL_LAKES).csv", "\n".join(lines) + "\n")


def square(lon, lat, d):
    return [[lon, lat], [lon, lat + d], [lon + d, lat + d], [lon + d, lat], [lon, lat]]


class Bowls(unittest.TestCase):
    def test_a_lake_is_a_bowl_to_its_maximum_depth(self):
        win, d = globathy.bowl([square(-74.0, 46.0, 0.05)], 20.0)
        ok = np.isfinite(d)
        self.assertTrue(ok.sum() > 100)
        self.assertAlmostEqual(float(np.nanmin(d)), -20.0, places=3)
        self.assertTrue(float(np.nanmax(d)) > -2.0, "shallow at the shore")
        self.assertTrue((d[ok] < 0).all())

    def test_the_run_writes_each_tile_once_with_its_lakes(self):
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            hydrolakes_zip(t / "hl.zip", [(1, [square(-74.0, 46.0, 0.05)]), (2, [square(-73.8, 46.0, 0.03)]), (3, [square(-73.6, 46.0, 0.03)])])
            # (Lake 3 has no maximum depth: left out.)
            params_zip(t / "p.zip", [(1, -73.97, 46.0, 20.0), (2, -73.79, 46.0, 5.0), (3, -73.59, 46.0, "")])
            dmax = globathy.read_dmax(t / "p.zip", [-75, 45, -73, 47])
            self.assertEqual(dmax, {1: 20.0, 2: 5.0})
            # Two boxes side by side: every tile written once.
            w = Writer(t / "out")
            n = 0
            for box in ([-75.0, 44.0, -73.9, 47.0], [-73.9, 44.0, -72.0, 47.0]):
                reach = [box[0], box[1] - 1.5, box[2] + 1.5, box[3]]
                n += globathy.produce(globathy.read_lakes(t / "hl.zip", reach, dmax), w, "u", box)
            w.write_index(t / "i.geojson")
            feats = json.loads((t / "i.geojson").read_text())["features"]
            files = [f["properties"]["file"] for f in feats]
            self.assertEqual(len(files), n)
            self.assertEqual(len(files), len(set(files)))
            import rasterio

            lo = []
            from packs import gdal_path

            for f in feats:
                with rasterio.open(gdal_path(t / "out", f)) as r:
                    a = r.read(1)
                    lo.append(float(np.nanmin(a)))
            self.assertAlmostEqual(min(lo), -20.0, places=3)


if __name__ == "__main__":
    unittest.main()
