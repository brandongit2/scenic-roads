"""tools/dev/inputsnas.py: the staged declarations against a stand-in NAS project folder (--root),
in a toy checkout with its own origin. Run by check.sh when inputsnas.py or this changes:
python3 -m unittest discover -s tools/dev/test -p 'test_inputsnas.py'."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), "inputsnas.py")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def read(path):
    with open(path) as f:
        return f.read()


def git(cwd, *a):
    subprocess.run(["git", "-C", cwd, *a], check=True, capture_output=True)


class Drift(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.repo, self.nas = os.path.join(self.tmp, "repo"), os.path.join(self.tmp, "nas")
        r = self.repo
        os.makedirs(os.path.join(r, "tools/dev"))
        shutil.copy(SCRIPT, os.path.join(r, "tools/dev/inputsnas.py"))
        write(f"{r}/producers/pyproject.toml", "[project]\n")
        write(f"{r}/producers/extents/terrain/a.toml", "name = 'a'\n")
        write(f"{r}/producers/extents/terrain/a.geojson", "{}\n")
        write(f"{r}/producers/extents/terrain/score.toml", "w = 1\n")
        write(f"{r}/producers/sources/x.toml", 'name = "x"\n[data]\nindex = "x-tiles.geojson"\n')
        write(f"{r}/producers/sources/x-tiles.geojson", "{}\n")
        write(f"{r}/producers/sources/x.py", "# a producer\n")
        write(f"{r}/producers/depth/declarations/d.toml", 'name = "d"\n[data]\nindex = "d-grids.geojson"\n')
        write(f"{r}/producers/depth/declarations/w.toml", 'name = "w"\n[data]\nindex = "w-grids.geojson"\n')
        write(f"{r}/producers/elevation/e.toml", "name = 'e'\n")
        write(f"{r}/producers/elevation/squares.geojson", "{}\n")  # (a producer's input: no declaration names it)
        write(f"{r}/producers/elevation/1m/e1.toml", "name = 'e1'\n")
        write(f"{r}/producers/elevation/held/later.toml", "name = 'later'\n")
        write(f"{r}/producers/how/README.md", "the staging's own\n")
        write(f"{r}/producers/how/inputs-README.md", "inputs\n")
        write(f"{r}/producers/how/sources/README.md", "how sources\n")
        write(f"{r}/producers/how/timetables/feeds/README.md", "how feeds\n")
        write(f"{r}/producers/how/translations/README.md", "how translations\n")
        write(f"{r}/producers/how/translations/briefs/b.md", "a brief\n")
        git(self.tmp, "init", "-q", "--bare", "origin.git")
        git(r, "init", "-q", "-b", "main")
        git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "0")
        git(r, "add", "-A")
        git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "toy")
        git(r, "remote", "add", "origin", os.path.join(self.tmp, "origin.git"))
        git(r, "push", "-q", "origin", "main")
        git(r, "fetch", "-q", "origin")
        # The NAS as placed: every staged file, plus the NAS's own.
        n = self.nas
        for src, dst in [
            ("extents/terrain/a.toml", "inputs/extents/terrain/a.toml"),
            ("extents/terrain/a.geojson", "inputs/extents/terrain/a.geojson"),
            ("extents/terrain/score.toml", "inputs/extents/terrain/score.toml"),
            ("sources/x.toml", "inputs/sources/x.toml"),
            ("sources/x-tiles.geojson", "inputs/sources/x-tiles.geojson"),
            ("depth/declarations/d.toml", "inputs/sources/d.toml"),
            ("elevation/e.toml", "inputs/elevation/e.toml"),
            ("elevation/1m/e1.toml", "inputs/elevation/e1.toml"),
            ("how/inputs-README.md", "inputs/README.md"),
            ("how/sources/README.md", "inputs/sources/how/README.md"),
            ("how/timetables/feeds/README.md", "inputs/timetables/feeds/how/README.md"),
            ("how/translations/README.md", "inputs/translations/how/README.md"),
            ("how/translations/briefs/b.md", "inputs/translations/how/briefs/b.md"),
        ]:
            os.makedirs(os.path.dirname(f"{n}/{dst}"), exist_ok=True)
            shutil.copy(f"{r}/producers/{src}", f"{n}/{dst}")
        write(f"{n}/inputs/sources/d-grids.geojson", "{}\n")  # (made by its producer on the NAS)
        write(f"{n}/inputs/elevation/gsi.toml", "the owner's own\n")
        write(f"{n}/inputs/elevation/@eaDir/x", "Synology's\n")
        write(f"{n}/inputs/translations/how/analysis.txt", "the NAS's own notes\n")

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_(self, *a, repo=None):
        p = subprocess.run(
            [sys.executable, os.path.join(repo or self.repo, "tools/dev/inputsnas.py"), *a, "--root", self.nas],
            capture_output=True, text=True,
        )
        return p.returncode, p.stdout, p.stderr

    def test_as_staged_is_one_line(self):
        code, out, err = self.run_("drift")
        self.assertEqual(code, 0, out + err)
        lines = out.splitlines()
        self.assertEqual(len(lines), 1, out)
        self.assertIn("as staged (13 files in 7 folders", lines[0])
        self.assertIn("the NAS's own 2", lines[0])  # (d-grids.geojson, gsi.toml; not @eaDir, nor the notes' own)
        self.assertIn("waiting for their index 1: w.toml", lines[0])
        self.assertIn("held 1: producers/elevation/held/later.toml", lines[0])

    def test_each_kind_of_drift_is_named(self):
        n, r = self.nas, self.repo
        write(f"{n}/inputs/extents/terrain/a.toml", "name = 'changed'\n")
        write(f"{n}/inputs/extents/terrain/gone.toml", "removed from the repository\n")
        os.remove(f"{n}/inputs/sources/x-tiles.geojson")
        write(f"{n}/inputs/translations/how/briefs/b.md", "edited on the NAS\n")
        write(f"{n}/inputs/elevation/later.toml", "placed while held\n")
        write(f"{r}/producers/newthing/decl.toml", "name = 'new'\n")
        code, out, _ = self.run_("drift")
        self.assertEqual(code, 1, out)
        self.assertIn("DRIFT: 2 differ, 1 not placed, 1 only on the NAS, 1 held but on the NAS, 1 unmapped", out)
        for line in [
            "  differs: inputs/extents/terrain/a.toml (producers/extents/terrain/a.toml)",
            "  differs: inputs/translations/how/briefs/b.md (producers/how/translations/briefs/b.md)",
            "  not placed: inputs/sources/x-tiles.geojson (producers/sources/x-tiles.geojson)",
            "  only on the NAS: inputs/extents/terrain/gone.toml",
            "  held but on the NAS: inputs/elevation/later.toml (producers/elevation/held/later.toml)",
            "  unmapped: producers/newthing/decl.toml",
        ]:
            self.assertIn(line, out.splitlines() if not line.startswith("  unmapped") else out)

    def test_a_folder_not_on_the_nas_is_not_placed_not_drift(self):
        shutil.rmtree(f"{self.nas}/inputs/elevation")
        code, out, _ = self.run_("drift")
        self.assertEqual(code, 0, out)
        self.assertIn("not placed, no NAS folder: producers/elevation, producers/elevation/1m → inputs/elevation", out)

    def test_not_read_is_exit_2(self):
        p = subprocess.run(
            [sys.executable, f"{self.repo}/tools/dev/inputsnas.py", "drift", "--root", f"{self.tmp}/none"],
            capture_output=True, text=True,
        )
        self.assertEqual(p.returncode, 2, p.stdout)
        self.assertIn("NOT READ", p.stdout)

    def test_place_refuses_a_worktree_and_a_checkout_not_at_origin_main(self):
        wt = os.path.join(self.tmp, "wt")
        git(self.repo, "worktree", "add", "-q", "-b", "side", wt)
        code, _, err = self.run_("place", repo=wt)
        self.assertEqual(code, 2)
        self.assertIn("a worktree, not the main checkout", err)
        git(self.repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "ahead")
        code, _, err = self.run_("place")
        self.assertEqual(code, 2)
        self.assertIn("isn't origin/main", err)
        git(self.repo, "reset", "-q", "--hard", "origin/main")
        write(f"{self.repo}/producers/sources/x.toml", "uncommitted\n")
        code, _, err = self.run_("place")
        self.assertEqual(code, 2)
        self.assertIn("producers/ has uncommitted changes", err)

    def test_place_stages_verifies_swaps_and_keeps(self):
        n = self.nas
        write(f"{n}/inputs/extents/terrain/a.toml", "name = 'old'\n")
        write(f"{n}/inputs/extents/terrain/gone.toml", "removed from the repository\n")
        os.remove(f"{n}/inputs/sources/x-tiles.geojson")
        write(f"{n}/inputs/README.md", "old inputs readme\n")
        code, out, err = self.run_("place", "--dry-run")
        self.assertEqual(code, 0, out + err)
        self.assertIn("dry run: nothing written", out)
        self.assertFalse(os.path.exists(f"{n}/inputs/.place"))
        code, out, err = self.run_("place")
        self.assertEqual(code, 0, out + err)
        self.assertIn("placing 3: inputs/README.md, inputs/extents/terrain, inputs/sources", out)
        self.assertIn("no input check asked; on the NAS: scenic inputs check extents", out)
        self.assertIn("scenic inputs check sources", out)
        self.assertIn("delete the kept folder", out)
        code, out2, _ = self.run_("drift")
        self.assertEqual(code, 0, out2)
        (stamp,) = os.listdir(f"{n}/inputs/.place")
        kept = f"{n}/inputs/.place/{stamp}/kept/inputs"
        # The old, whole, kept; nothing deleted; the NAS's own carried; the waiting declaration not placed.
        self.assertEqual(read(f"{kept}/extents/terrain/a.toml"), "name = 'old'\n")
        self.assertTrue(os.path.exists(f"{kept}/extents/terrain/gone.toml"))
        self.assertFalse(os.path.exists(f"{n}/inputs/extents/terrain/gone.toml"))
        self.assertEqual(read(f"{kept}/README.md"), "old inputs readme\n")
        self.assertEqual(read(f"{n}/inputs/sources/d-grids.geojson"), "{}\n")
        self.assertTrue(os.path.exists(f"{n}/inputs/sources/how/README.md"))
        self.assertFalse(os.path.exists(f"{n}/inputs/sources/w.toml"))
        left = [f for _, _, fs in os.walk(f"{n}/inputs/.place/{stamp}/new") for f in fs]
        self.assertEqual(left, [])  # (each staged folder swapped in)
        # Nothing left to place.
        code, out, _ = self.run_("place")
        self.assertEqual(code, 0)
        self.assertIn("nothing to place", out)


if __name__ == "__main__":
    unittest.main()
