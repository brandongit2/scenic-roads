"""packs.py and repack.py (task #452): `uv run --project .. python -m unittest test_packs`."""
import hashlib
import io
import os
import json
import shutil
import tarfile
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import packs
import repack


def feature(name: str, lon: float, lat: float, data: bytes, low: bool = False) -> dict:
    p = {"url": "https://x.org/a", "zoom": 12, "file": f"{name}.tif", "hash": hashlib.sha256(data).hexdigest()[:16]}
    if low:
        p["low_file"] = f"{name}.low.tif"
    ring = [[lon, lat], [lon + 0.1, lat], [lon + 0.1, lat + 0.1], [lon, lat + 0.1], [lon, lat]]
    return {"type": "Feature", "properties": p, "geometry": {"type": "Polygon", "coordinates": [ring]}}


def grids(folder: Path) -> list[dict]:
    """Five grids in two z6 tiles (four near Halifax, one near Vancouver), two with a low file."""
    folder.mkdir(parents=True, exist_ok=True)
    feats = []
    for i, (lon, lat) in enumerate([(-63.6, 44.6), (-63.5, 44.6), (-63.6, 44.7), (-63.4, 44.5), (-123.1, 49.2)]):
        data = bytes([i]) * (1000 + 37 * i)
        (folder / f"g{i}.tif").write_bytes(data)
        f = feature(f"g{i}", lon, lat, data, low=i % 2 == 0)
        if i % 2 == 0:
            (folder / f"g{i}.low.tif").write_bytes(b"L" * (50 + i))
        feats.append(f)
    return feats


class Packs(unittest.TestCase):
    def test_areas_are_z6_tiles(self):
        self.assertEqual(packs.area(feature("a", -63.6, 44.6, b"")), "6-20-23")
        self.assertEqual(packs.area(feature("a", -123.1, 49.2, b"")), "6-10-21")

    def test_a_folder_packs_by_area_its_ranges_its_bytes_and_the_same_grids_the_same_pack(self):
        with tempfile.TemporaryDirectory() as t:
            a, b = Path(t) / "a", Path(t) / "b"
            fa, fb = grids(a), grids(b)
            want = {n: (a / n).read_bytes() for f in fa for n in packs.names(f)}
            made = packs.pack_folder(a, fa)
            self.assertEqual(sorted(made), sorted(packs.pack_folder(b, fb)), "same grids, same packs")
            self.assertEqual(len(made), 2)
            self.assertEqual(sorted(p.name for p in a.iterdir()), sorted(made), "the loose files gone")
            for f in fa:
                for k in packs.FILE_KEYS:
                    if k in f["properties"]:
                        self.assertEqual(packs.read_member(a, f, k), want[f["properties"][k]])
                # (A plain tar: tar reads it as the files.)
                with tarfile.open(a / f["properties"]["pack"]) as tf:
                    self.assertEqual(tf.extractfile(f["properties"]["file"]).read(), want[f["properties"]["file"]])
            # Packing again packs nothing (all packed).
            self.assertEqual(packs.pack_folder(a, fa), [])

    def test_a_grid_not_its_hash_stops_the_packing(self):
        with tempfile.TemporaryDirectory() as t:
            a = Path(t)
            fa = grids(a)
            (a / "g1.tif").write_bytes(b"changed")
            with self.assertRaises(SystemExit):
                packs.pack_folder(a, fa)


def accept(root: Path, data: bytes, tag: str, beat_ago: float = 0, jobs=(), helper_jobs=()):
    """The build's current accepted version of the sources unit: index `s-grids.geojson` as `data`;
    the lead's beat `beat_ago` s ago, with `jobs` running here and `helper_jobs` on a helper."""
    acc = root / "sources/inputs/sources"
    (acc / f"s-grids.{tag}.geojson").write_bytes(data)
    (acc / f"@index.{tag}.json").write_text(json.dumps({"files": {"s-grids.geojson": {"file": f"sources/inputs/sources/s-grids.{tag}.geojson"}}}))
    (root / "state").mkdir(exist_ok=True)
    (root / "state/status.json").write_text(json.dumps({
        "beat": time.time() - beat_ago,
        "jobs": [{"id": j} for j in jobs],
        "waiting": [],
        "helpers": [{"jobs": [{"id": j} for j in helper_jobs]}],
        "inputs": [{"unit": "sources", "version": f"sources/inputs/sources/@index.{tag}.json"}],
    }))


def backdate(root: Path, work: Path, hours: float):
    """The packed index taken in `hours` ago (its first sighting and its accepted copy's time)."""
    d = json.loads((work / "done.json").read_text())
    d["taken_in_seen"] = time.time() - hours * 3600
    (work / "done.json").write_text(json.dumps(d))
    for p in (root / "sources/inputs/sources").glob("s-grids.packed.geojson"):
        os.utime(p, (time.time() - hours * 3600,) * 2)


class Repack(unittest.TestCase):
    def setup(self, t: Path):
        root, drop, acc = t / "nas", t / "drop", t / "nas/sources/inputs/sources"
        for d in (drop, acc):
            d.mkdir(parents=True)
        src = root / "sources/fetched/s"
        feats = grids(src)
        (drop / "s.toml").write_text('[data]\nrole = "depth"\nindex = "s-grids.geojson"\n')
        (drop / "s-grids.geojson").write_text(json.dumps({"type": "FeatureCollection", "features": feats}))
        args = ["s", "--work", str(t / "work"), "--nas", "local", "--nas-root", str(root), "--drop", str(drop), "--root", str(root), "--batch-gb", "0"]
        return root, drop, acc, src, feats, args

    def run_(self, args):
        with redirect_stdout(io.StringIO()):
            repack.main(args)

    def test_a_repack_packs_on_the_nas_adopts_and_sweeps_only_once_taken_in(self):
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            root, drop, acc, src, feats, args = self.setup(t)
            loose = {n: (src / n).read_bytes() for f in feats for n in packs.names(f)}
            orig = (drop / "s-grids.geojson").read_bytes()
            self.run_(args)
            # The packs on the NAS beside the loose files, the drop box untouched without --adopt.
            self.assertEqual((drop / "s-grids.geojson").read_bytes(), orig)
            packed = json.loads((t / "work/s/s-grids.geojson").read_text())["features"]
            self.assertEqual([f["properties"]["file"] for f in packed], [f["properties"]["file"] for f in feats])
            for f in packed:
                for k in packs.FILE_KEYS:
                    if k in f["properties"]:
                        self.assertEqual(packs.read_member(src, f, k), loose[f["properties"][k]])
                        self.assertEqual(f["properties"]["at"][k][2], hashlib.sha256(loose[f["properties"][k]]).hexdigest()[:16])
            self.assertFalse((src / ".repack-incoming").exists() and any((src / ".repack-incoming").iterdir()))
            self.assertEqual(list((t / "work/s/out").glob("*.tar")), [], "the local packs deleted once on the NAS")
            # Again (resumed): the same, nothing read again.
            before = (t / "work/s/s-grids.geojson").read_bytes()
            shutil.rmtree(t / "work/s/out")
            self.run_(args)
            self.assertEqual((t / "work/s/s-grids.geojson").read_bytes(), before)
            # Adopted; a sweep before the build takes it in deletes nothing.
            self.run_(args + ["--adopt"])
            self.assertEqual((drop / "s-grids.geojson").read_bytes(), before)
            with self.assertRaises(SystemExit):
                self.run_(args + ["--sweep"])
            self.assertTrue(all((src / n).exists() for n in loose))
            # Taken in, then rolled back (the drop box and the accepted version the loose index
            # again, an accepted copy of the packed one still there): nothing deleted.
            accept(root, before, "packed")
            (drop / "s-grids.geojson").write_bytes(orig)
            accept(root, orig, "loose")
            with self.assertRaises(SystemExit):
                self.run_(args + ["--sweep"])
            self.assertTrue(all((src / n).exists() for n in loose))
            # The drop box packed again but the build's version still the loose one: nothing deleted.
            self.run_(args + ["--adopt"])
            with self.assertRaises(SystemExit):
                self.run_(args + ["--sweep"])
            self.assertTrue(all((src / n).exists() for n in loose))
            # Taken in, but only now: nothing deleted (jobs planned before read the loose index).
            accept(root, before, "packed")
            self.refused(args, "only 0.0 h ago")
            self.assertTrue(all((src / n).exists() for n in loose))
            backdate(root, t / "work/s", 25)
            # A day on, but the lead's status stale, a depth job running, one on a helper, or one
            # held to resume: nothing deleted.
            accept(root, before, "packed", beat_ago=3600)
            backdate(root, t / "work/s", 25)
            self.refused(args, "stale")
            accept(root, before, "packed", jobs=["depth 6/20/23"])
            backdate(root, t / "work/s", 25)
            self.refused(args, "depth 6/20/23")
            accept(root, before, "packed", helper_jobs=["depth-lo 3/2/2"])
            backdate(root, t / "work/s", 25)
            self.refused(args, "depth-lo 3/2/2")
            accept(root, before, "packed")
            backdate(root, t / "work/s", 25)
            (root / "state/pool/resume").mkdir(parents=True)
            (root / "state/pool/resume/m-1.json").write_text('{"step": "depth", "target": "6/1/2"}')
            self.refused(args, "resume: m-1.json")
            (root / "state/pool/resume/m-1.json").unlink()
            # A pack gone from the NAS just before: nothing deleted.
            gone = next(src.glob("*.tar"))
            keep = gone.read_bytes()
            gone.unlink()
            self.refused(args, "not on the NAS")
            gone.write_bytes(keep)
            self.assertTrue(all((src / n).exists() for n in loose))
            # Safely taken in: swept, the packs alone left.
            self.run_(args + ["--sweep"])
            self.assertFalse(any((src / n).exists() for n in loose))
            self.assertEqual(sorted(p.name for p in src.glob("*.tar")), sorted({f["properties"]["pack"] for f in packed}))

    def refused(self, args, why):
        with self.assertRaises(SystemExit) as e:
            self.run_(args + ["--sweep"])
        self.assertIn(why, str(e.exception))

    def test_a_trial_packs_a_box_into_its_own_folder_and_never_adopts(self):
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            root, drop, acc, src, feats, args = self.setup(t)
            dest = root / "scratch"
            dest.mkdir()
            self.run_(args + ["--box", "-64,44,-63,45", "--dest", str(dest)])
            packed = json.loads((t / "work/s/s-grids.geojson").read_text())["features"]
            self.assertEqual(len(packed), 4)
            self.assertEqual(len(list((dest / "s").glob("*.tar"))), 1)
            self.assertEqual(len(list(src.glob("*.tar"))), 0)
            with self.assertRaises(SystemExit):
                self.run_(args + ["--box", "-64,44,-63,45", "--dest", str(dest), "--adopt"])
            # Its --work refuses a real run (its done areas are the trial's folder's).
            with self.assertRaises(SystemExit):
                self.run_(args)

    def test_a_changed_drop_box_index_stops_a_run(self):
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            root, drop, acc, src, feats, args = self.setup(t)
            self.run_(args)
            (drop / "s-grids.geojson").write_text(json.dumps({"type": "FeatureCollection", "features": feats[:2]}))
            with self.assertRaises(SystemExit):
                self.run_(args + ["--adopt"])


if __name__ == "__main__":
    unittest.main()
