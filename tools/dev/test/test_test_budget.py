"""tools/dev/test-budget.py (#1553) against a stand-in dev jobs app: the tests picked from the last
publishes' suites (their medians, at or over the budget, the slowest few; none a measure's own), a
measure not due, and a measure's record (its settings, each test alone, those over the budget) with
the test Mac's run stood in for. Run by check.sh when test-budget.py, its settings or this changes:
python3 -m unittest discover -s tools/dev/test -p 'test_test_budget.py'."""
import importlib.util
import io
import json
import os
import tempfile
import threading
import time
import unittest
import urllib.parse
from contextlib import redirect_stdout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
DEV = os.path.dirname(HERE)


def load():
    spec = importlib.util.spec_from_file_location("test_budget", os.path.join(DEV, "test-budget.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def publish(rid, t0, tests, budget=()):
    """A publish's run: its suites' tests {(binary, test): seconds}; `budget`'s tests under a measure."""
    spans, n = [], 0
    for kind, items in (("suite", tests), ("budget", budget)):
        for (b, t), secs in items:
            n += 1
            parent = f"{rid}-s{n}"
            if kind == "budget":
                spans.append({"id": f"{rid}-g{n}", "parent": rid, "kind": "budget", "name": "test budget", "t0": t0, "t1": t0 + 1, "attrs": {}})
                spans.append({"id": parent, "parent": f"{rid}-g{n}", "kind": "suite", "name": b, "t0": t0, "t1": t0 + 1, "attrs": {}})
            else:
                spans.append({"id": parent, "parent": rid, "kind": "suite", "name": b, "t0": t0, "t1": t0 + 1, "attrs": {}})
            spans.append({"id": f"{rid}-t{n}", "parent": parent, "kind": "test", "name": t, "t0": t0, "t1": t0 + secs, "attrs": {}})
    return {"run": {"id": rid, "kind": "publish", "t0": t0, "t1": t0 + 600}, "spans": spans}


class Jobs:
    """The app's /api/jobs reads: runs?kind=publish, runs/<id>, latest?kind=budget."""

    def __init__(self):
        self.runs, self.latest = {}, None
        jobs = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                u = urllib.parse.urlparse(self.path)
                p = u.path.removeprefix("/api/jobs/")
                if p == "runs":
                    body = {"runs": [r["run"] for r in jobs.runs.values()]}
                elif p.startswith("runs/"):
                    body = jobs.runs[urllib.parse.unquote(p[5:])]
                elif p == "latest":
                    body = {"span": jobs.latest}
                else:
                    self.send_response(404); self.end_headers(); return
                b = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(b)))
                self.end_headers()
                self.wfile.write(b)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]


class Proc:
    """The test Mac's run stood in for: nextest's lines, the exit."""

    def __init__(self, lines, rc=0):
        self.stdout, self.rc = io.StringIO("".join(lines)), rc

    def wait(self):
        return self.rc


class TestBudget(unittest.TestCase):
    def setUp(self):
        self.jobs = Jobs()
        self.tmp = tempfile.TemporaryDirectory()
        self.env = dict(os.environ)
        os.environ.update(SCENIC_BOARD=self.jobs.url, SCENIC_DEVJOB_DIR=self.tmp.name, SCENIC_DEVJOB_MAC="m1", SCENIC_DEVJOB="1")
        os.environ.pop("SCENIC_DEVJOB_PARENT", None)
        self.tb = load()
        self.tb.devjob.DIR, self.tb.devjob.SPOOL, self.tb.devjob.LIVE = self.tmp.name, os.path.join(self.tmp.name, "spool.ndjson"), os.path.join(self.tmp.name, "live")
        self.tb.devjob.ensure_sampler = lambda: None
        self.s = {"budget_s": 10, "every_h": 24, "publishes": 2, "slowest": 2}
        self.tb.settings = lambda: dict(self.s)
        now = time.time()
        # Three publishes, newest last: only the last two read; a measure's own tests never picked.
        self.jobs.runs = {
            "p1": publish("p1", now - 9000, {("terrain", "old::slow"): 90.0}.items()),
            "p2": publish("p2", now - 6000, {("terrain", "a::t"): 30.0, ("pipeline", "b::t"): 9.0, ("inputs", "c::t"): 12.0, ("terrain", "d::t"): 11.0}.items(),
                          budget={("terrain", "a::t"): 3.0}.items()),
            "p3": publish("p3", now - 3000, {("terrain", "a::t"): 20.0, ("pipeline", "b::t"): 13.0, ("inputs", "c::t"): 14.0, ("terrain", "d::t"): 8.0}.items()),
        }

    def tearDown(self):
        self.jobs.server.shutdown()
        os.environ.clear(); os.environ.update(self.env)
        self.tmp.cleanup()

    def events(self):
        p = os.path.join(self.tmp.name, "spool.ndjson")
        if not os.path.exists(p):
            return []
        with open(p) as f:
            return [json.loads(l) for l in f]

    def test_picks_the_slowest_at_or_over_the_budget_from_the_last_publishes(self):
        tests, n_over, read = self.tb.pick(self.s, time.time())
        # (Medians over p2 and p3: a 25, c 13, b 11, d 9.5; p1's never read, the measure's 3 s not counted.)
        self.assertEqual(read, ["p3", "p2"])
        self.assertEqual(n_over, 3)
        self.assertEqual([(b, t, m) for b, t, m in tests], [("terrain", "a::t", 25.0), ("inputs", "c::t", 13.0)])

    def test_not_due_runs_nothing(self):
        self.jobs.latest = {"id": "x", "t0": time.time() - 3600, "attrs": {}}
        ran = []
        self.tb.remote = lambda argv, env: ran.append(argv) or Proc([])
        with redirect_stdout(io.StringIO()) as out:
            self.assertEqual(self.tb.run("/tree"), 0)
        self.assertIn("not due", out.getvalue())
        self.assertEqual((ran, self.events()), ([], []))

    def test_a_measure_records_each_test_alone_and_flags_those_over(self):
        self.jobs.latest = {"id": "x", "t0": time.time() - 25 * 3600, "attrs": {}}
        cmds = []

        def remote(argv, env):
            cmds.append((argv, env["SCENIC_DEVJOB_PARENT"]))
            return Proc(["    Starting 2 tests across 2 binaries\n",
                         "        PASS [  12.500s] (1/2) terrain a::t\n",
                         "        PASS [   4.250s] (2/2) inputs c::t\n",
                         "     Summary [  16.800s] 2 tests run: 2 passed, 9 skipped\n"])
        self.tb.remote = remote
        with redirect_stdout(io.StringIO()) as out:
            self.assertEqual(self.tb.run("/tree"), 0)
        (argv, parent), = cmds
        self.assertEqual(argv[:2], ["publish", "/tree"])
        self.assertIn("--test-threads 1", argv[2])
        self.assertIn("(binary_id(=terrain) & test(=a::t)) | (binary_id(=inputs) & test(=c::t))", argv[2])
        start, end = self.events()
        self.assertEqual((start["op"], start["kind"], start["id"], end["id"]), ("start", "budget", parent, parent))
        self.assertEqual({k: start["attrs"][k] for k in ("budget_s", "every_h", "publishes", "slowest", "chosen", "at_or_over_in_suites")},
                         {"budget_s": 10, "every_h": 24, "publishes": 2, "slowest": 2, "chosen": 2, "at_or_over_in_suites": 3})
        self.assertEqual(end["attrs"]["alone"], [["terrain", "a::t", 12.5, 25.0], ["inputs", "c::t", 4.25, 13.0]])
        self.assertEqual(end["attrs"]["over"], [["terrain", "a::t", 12.5]])
        self.assertEqual(end["status"], "ok")
        self.assertIn("OVER 10 s alone: terrain a::t: 12.5 s", out.getvalue())

    def test_the_app_away_measures_nothing_and_fails_nothing(self):
        os.environ["SCENIC_BOARD"] = "http://127.0.0.1:9"
        with redirect_stdout(io.StringIO()) as out:
            self.assertEqual(self.tb.run("/tree"), 0)
        self.assertIn("NOT MEASURED", out.getvalue())

    def test_the_settings_file_holds_each_setting(self):
        with open(os.path.join(DEV, "test-budget.json")) as f:
            s = json.load(f)
        self.assertEqual(sorted(s), ["budget_s", "every_h", "publishes", "slowest"])


if __name__ == "__main__":
    unittest.main()
