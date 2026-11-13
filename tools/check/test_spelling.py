"""The spelling guard's test (task #1374): tools/check/spelling.py finds -ise/-yse spellings, in prose
and inside identifiers, and leaves alone what is not one. Run by tools/app/guards.sh:
  python3 tools/check/test_spelling.py
"""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import spelling as S


def found(text):
    return [(w, s) for _, w, s in S.scan_text(text)]


class Spelling(unittest.TestCase):
    def test_flags_ise_forms_with_the_ize_suggestion(self):
        self.assertEqual(found("Equalise the histogram"), [("Equalise", "equalize")])
        self.assertEqual(found("rasterised, rasterising and rasterisation"),
                         [("rasterised", "rasterized"), ("rasterising", "rasterizing"), ("rasterisation", "rasterization")])
        self.assertEqual(found("an Optimiser, normalisers, finalise"),
                         [("Optimiser", "optimizer"), ("normalisers", "normalizers"), ("finalise", "finalize")])

    def test_flags_yse_forms_but_not_analyses(self):
        self.assertEqual(found("analyse it, paralysed, catalysing"),
                         [("analyse", "analyze"), ("paralysed", "paralyzed"), ("catalysing", "catalyzing")])
        self.assertEqual(found("two analyses, an analysis, the emphasis"), [])

    def test_finds_words_inside_identifiers(self):
        self.assertEqual(found("summariseFeature(x)"), [("summarise", "summarize")])
        self.assertEqual(found("let n = EQUALISE_MAX + measure_quantisation_drift()"),
                         [("EQUALISE", "equalize"), ("quantisation", "quantization")])

    def test_leaves_the_ize_spellings_and_look_alikes(self):
        ok = ("Equalize rasterized organization analyze raise raised otherwise likewise clockwise precise promise "
              "exercise advise revise supervise surprise enterprise expertise noise praise compromise premise "
              "colour centre metre emphasis analysis hypothesis")
        self.assertEqual(found(ok), [])

    def test_urls_and_hashes_are_not_read(self):
        self.assertEqual(found("see https://example.org/equalise/optimise?x=1 for it"), [])
        self.assertEqual(found("sha 0123456789abcdef0123456789abcdef"), [])

    def test_a_marked_line_is_a_deliberate_use(self):
        self.assertEqual(found("The Organisation (its name)  // spelling: kept, a body's name\nOrganise"), [("Organise", "organize")])

    def test_scan_skips_vendored_data_and_binary_and_reports_path_line(self):
        with tempfile.TemporaryDirectory() as d:
            def put(rel, data, mode="w"):
                p = os.path.join(d, rel)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, mode) as fh:
                    fh.write(data)
            put("src/a.rs", "// fine\nfn equalise() {}\n")
            put("web/maplibre/src/b.ts", "equalise\n")
            put("crates/geos-src/source/c.cpp", "optimisation\n")
            put("Cargo.lock", "optimise\n")
            put("data/places.csv", "Organisation\n")
            put("bin.dat", b"\xff\xfeequalise", "wb")
            files = ["src/a.rs", "web/maplibre/src/b.ts", "crates/geos-src/source/c.cpp", "Cargo.lock",
                     "data/places.csv", "bin.dat"]
            bad = S.scan(d, files)
            self.assertEqual(len(bad), 1)
            self.assertTrue(bad[0].startswith("src/a.rs:2: equalise"), bad)


if __name__ == "__main__":
    unittest.main()
