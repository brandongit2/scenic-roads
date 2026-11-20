"""The testkit guard's test (task #1135): tools/check/testkit_deps.py refuses a testkit feature turned on
outside [dev-dependencies] and the testkit features' own lists, and passes the repository's manifests.
Run by tools/app/guards.sh:
  python3 tools/check/test_testkit_deps.py
"""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import testkit_deps as T

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


class Testkit(unittest.TestCase):
    def test_a_dev_dependency_and_a_testkit_feature_s_own_list_pass(self):
        self.assertEqual(T.scan_manifest("""
[features]
testkit = ["dep:tempfile", "store/testkit", "inputs?/testkit"]
[dependencies]
store = { path = "../store" }
[dev-dependencies]
store = { path = "../store", features = ["testkit"] }
[target.'cfg(unix)'.dev-dependencies]
inputs = { path = "../inputs", features = ["testkit"] }
"""), [])

    def test_a_normal_dependency_turning_it_on_is_refused(self):
        self.assertEqual(T.scan_manifest("""
[dependencies]
store = { path = "../store", features = ["serde", "testkit"] }
"""), [("[dependencies]", "store", "testkit")])

    def test_every_other_place_is_refused(self):
        found = T.scan_manifest("""
[features]
default = ["testkit"]
bench = ["store/testkit"]
[build-dependencies]
codecs = { path = "../codecs", features = ["testkit"] }
[target.'cfg(unix)'.dependencies]
jobrt = { path = "../jobrt", features = ["testkit"] }
[workspace.dependencies]
store = { path = "crates/store", features = ["testkit"] }
""")
        self.assertEqual(sorted(found), sorted([
            ("[features]", "default", "testkit"),
            ("[features]", "bench", "store/testkit"),
            ("[build-dependencies]", "codecs", "testkit"),
            ("[target.cfg(unix).dependencies]", "jobrt", "testkit"),
            ("[workspace.dependencies]", "store", "testkit"),
        ]))

    def test_a_planted_case_in_a_file_fails_the_run_and_names_it(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "Cargo.toml")
            with open(p, "w") as f:
                f.write('[package]\nname = "x"\n[dependencies]\nstore = { path = "s", features = ["testkit"] }\n')
            r = subprocess.run([sys.executable, os.path.join(ROOT, "tools/check/testkit_deps.py"), p],
                               capture_output=True, text=True)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("[dependencies] store turns on 'testkit'", r.stdout)

    def test_the_real_tree_passes(self):
        r = subprocess.run([sys.executable, os.path.join(ROOT, "tools/check/testkit_deps.py")],
                           capture_output=True, text=True, cwd=ROOT)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "", ""))
        # (It read something: pipeline's manifest turns testkits on, in its dev-dependencies.)
        with open(os.path.join(ROOT, "crates/pipeline/Cargo.toml"), "rb") as f:
            self.assertIn(b'features = ["testkit"]', f.read())


if __name__ == "__main__":
    unittest.main()
