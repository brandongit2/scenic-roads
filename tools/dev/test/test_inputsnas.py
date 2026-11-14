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

    def commit(self, msg):
        git(self.repo, "add", "-A")
        git(self.repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", msg)
        git(self.repo, "push", "-q", "origin", "main")
        git(self.repo, "fetch", "-q", "origin")

    def test_a_declaration_the_repository_removed_is_drift_and_left_out(self):
        os.remove(f"{self.repo}/producers/sources/x.toml")
        os.remove(f"{self.repo}/producers/sources/x-tiles.geojson")
        self.commit("x removed")
        code, out, _ = self.run_("drift")
        self.assertEqual(code, 1, out)
        self.assertIn("1 removed from the repository", out)
        self.assertIn("  removed from the repository: inputs/sources/x.toml (producers/sources/x.toml)", out)
        code, out, err = self.run_("place", "--dry-run")
        self.assertIn("leave out: inputs/sources/x.toml (the repository removed it", out, out + err)
        code, out, err = self.run_("place")
        self.assertEqual(code, 0, out + err)
        (stamp,) = os.listdir(f"{self.nas}/inputs/.place")
        self.assertFalse(os.path.exists(f"{self.nas}/inputs/sources/x.toml"))
        self.assertTrue(os.path.exists(f"{self.nas}/inputs/.place/{stamp}/kept/inputs/sources/x.toml"))
        # (Its index, no longer staged, is the NAS's own now: carried; the check says it's an orphan.)
        self.assertTrue(os.path.exists(f"{self.nas}/inputs/sources/x-tiles.geojson"))
        self.assertEqual(self.run_("drift")[0], 0)

    def test_the_dry_run_shows_each_change_and_writes_nothing(self):
        n = self.nas
        write(f"{n}/inputs/sources/x.toml", 'name = "x on the NAS"\n[data]\nindex = "x-tiles.geojson"\n')
        os.remove(f"{n}/inputs/sources/x-tiles.geojson")
        write(f"{n}/inputs/extents/terrain/gone.toml", "only here\n")
        before = sorted(os.path.join(d, f) for d, _, fs in os.walk(n) for f in fs)
        code, out, err = self.run_("place", "--dry-run")
        self.assertEqual(code, 0, out + err)
        for line in [
            "    replace: inputs/sources/x.toml (producers/sources/x.toml)",
            '      -name = "x on the NAS"',
            '      +name = "x"',
            "    add: inputs/sources/x-tiles.geojson (producers/sources/x-tiles.geojson)",
            "    leave out: inputs/extents/terrain/gone.toml (only on the NAS;",
        ]:
            self.assertTrue(any(l.startswith(line) for l in out.splitlines()), line + "\n" + out)
        self.assertEqual(sorted(os.path.join(d, f) for d, _, fs in os.walk(n) for f in fs), before)
        # (Anywhere: from a worktree, saying a placement would be refused.)
        wt = os.path.join(self.tmp, "wt")
        git(self.repo, "worktree", "add", "-q", "-b", "side", wt)
        code, out, _ = self.run_("place", "--dry-run", repo=wt)
        self.assertEqual(code, 0)
        self.assertIn("placing from here would be refused", out)

    def test_synology_the_macs_and_links_are_not_carried_drafts_are(self):
        n = self.nas
        write(f"{n}/inputs/elevation/e.toml", "changed on the NAS\n")
        write(f"{n}/inputs/elevation/.DS_Store", "the Mac's\n")
        write(f"{n}/inputs/elevation/_draft.toml", "the owner's draft\n")
        os.symlink(f"{n}/inputs/elevation/gsi.toml", f"{n}/inputs/elevation/link.toml")
        code, out, err = self.run_("drift")
        self.assertEqual(code, 1, out)
        self.assertNotIn("link.toml", out)
        code, out, err = self.run_("place", "inputs/elevation")
        self.assertEqual(code, 0, out + err)
        names = sorted(os.listdir(f"{n}/inputs/elevation"))
        self.assertEqual(names, ["_draft.toml", "e.toml", "e1.toml", "gsi.toml"])  # (no @eaDir, .DS_Store, link)
        self.assertEqual(read(f"{n}/inputs/elevation/e.toml"), "name = 'e'\n")

    def test_a_path_under_another_is_placed_with_it_one_old_folder_kept(self):
        n = self.nas
        write(f"{n}/inputs/sources/x.toml", "old\n")
        write(f"{n}/inputs/sources/how/README.md", "old notes\n")
        write(f"{n}/inputs/sources/how/own.md", "the NAS's own notes\n")
        code, out, err = self.run_("place")
        self.assertEqual(code, 0, out + err)
        self.assertIn("placing 2: inputs/sources, inputs/sources/how", out)
        (stamp,) = os.listdir(f"{n}/inputs/.place")
        kept = f"{n}/inputs/.place/{stamp}/kept/inputs/sources"
        self.assertEqual(read(f"{kept}/how/README.md"), "old notes\n")
        self.assertFalse(os.path.exists(f"{kept}/how/how"))
        self.assertEqual(read(f"{n}/inputs/sources/how/README.md"), "how sources\n")
        self.assertEqual(read(f"{n}/inputs/sources/how/own.md"), "the NAS's own notes\n")
        self.assertEqual(self.run_("drift")[0], 0)

    def fake(self, name, body):
        """A stand-in for a command on the stand-in NAS's PATH (`body`: sh, the real one as $REAL)."""
        b = os.path.join(self.tmp, "bin")
        os.makedirs(b, exist_ok=True)
        real = shutil.which(name)
        write(f"{b}/{name}", f"#!/bin/sh\nREAL={real}\n{body}\n")
        os.chmod(f"{b}/{name}", 0o755)
        return {**os.environ, "PATH": f"{b}:{os.environ['PATH']}"}

    def run_env(self, env, *a):
        p = subprocess.run([sys.executable, f"{self.repo}/tools/dev/inputsnas.py", *a, "--root", self.nas],
                           capture_output=True, text=True, env=env)
        return p.returncode, p.stdout, p.stderr

    def test_a_failed_swap_puts_the_old_folder_back(self):
        n = self.nas
        write(f"{n}/inputs/extents/terrain/a.toml", "name = 'old'\n")
        # (mv refuses to move a staged folder in: the old one must come back.)
        env = self.fake("mv", 'case "$1" in */.place/*/new/*) echo "mv: refused" >&2; exit 1 ;; esac\nexec $REAL "$@"')
        code, out, err = self.run_env(env, "place", "inputs/extents/terrain")
        self.assertEqual(code, 1, out + err)
        self.assertIn("the new folder couldn't be moved in: the old one is back", err)
        self.assertEqual(read(f"{n}/inputs/extents/terrain/a.toml"), "name = 'old'\n")

    def test_a_staging_not_verified_is_not_swapped(self):
        n = self.nas
        write(f"{n}/inputs/extents/terrain/a.toml", "name = 'old'\n")
        env = self.fake("tar", 'cat > /dev/null; exit 0')  # (tar writes nothing)
        code, out, err = self.run_env(env, "place", "inputs/extents/terrain")
        self.assertEqual(code, 1, out + err)
        self.assertIn("not verified (3 of 3 files differ or are missing", err)
        self.assertIn("not swapped", err)
        self.assertEqual(read(f"{n}/inputs/extents/terrain/a.toml"), "name = 'old'\n")

    def test_an_unusual_name_is_refused_before_anything_is_written(self):
        n = self.nas
        write(f"{n}/inputs/sources/x.toml", "old\n")
        write(f"{n}/inputs/sources/a name.toml", "the NAS's own, its name with a space\n")
        code, out, err = self.run_("place")
        self.assertNotEqual(code, 0)
        self.assertIn("refusing an unusual path", out + err)
        self.assertFalse(os.path.exists(f"{n}/inputs/.place"))


if __name__ == "__main__":
    unittest.main()
