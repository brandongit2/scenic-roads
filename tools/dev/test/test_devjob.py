"""tools/dev/devjob.py: what it reads from a job's output and cargo's timing reports, the wrapper's
status and spool, and the sampler's send and its end of a gone span. Run by tools/dev/test/devjob.zsh
(check.sh, publish's other tests): python3 -m unittest discover -s tools/dev/test -p 'test_devjob.py'."""
import fcntl
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import devjob  # noqa: E402

T = 1_800_000_000.0


def lines(text, t0=T, step=1.0):
    return [(t0 + i * step, l) for i, l in enumerate(text.strip("\n").split("\n"))]


def flat(spans, depth=0):
    for s in spans:
        yield depth, s
        yield from flat(s["kids"], depth + 1)


class Parse(unittest.TestCase):
    def test_cargo_compiles_and_a_failed_one(self):
        out, _ = devjob.parse(lines("""
    Blocking waiting for file lock on build directory
   Compiling serde v1.0.0
   Compiling pipeline v0.1.0 (/x/crates/pipeline)
    Finished `test` profile [optimized] target(s) in 1m 25s
    Checking store v0.1.0
error: could not compile `store` (lib) due to 1 previous error
"""), {"cargo"}, T, T + 10)
        self.assertEqual([(s["kind"], s["t0"], s["t1"], s["status"]) for s in out],
                         [("wait", T, T + 1, "ok"), ("compile", T + 1, T + 3, "ok"), ("compile", T + 4, T + 5, "fail")])
        self.assertEqual(out[1]["attrs"]["crates"], ["serde", "pipeline"])

    def test_cargo_test_suites_and_their_tests(self):
        out, _ = devjob.parse(lines("""
     Running unittests src/bin/scenic.rs (/x/target/debug/deps/scenic-f92e6657c7d3721a)
running 2 tests
test tests::a ... ok
test tests::b ... FAILED
test tests::c ... ignored
test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.50s
   Doc-tests toy
test src/lib.rs - two (line 1) ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
     Running tests/cli.rs (/x/target/debug/deps/cli-0123456789abcdef)
test cut ... ok
"""), {"tests"}, T, T + 20)
        self.assertEqual([(s["name"], s["status"]) for s in out], [("scenic (unittests src/bin/scenic.rs)", "fail"), ("toy (doctests)", "ok"), ("cli (tests/cli.rs)", "fail")])
        self.assertEqual((out[0]["t0"], out[0]["t1"]), (T + 5 - 2.5, T + 5), "the suite's own time, as libtest says it")
        self.assertEqual([(k["name"], k["status"], k["t0"] == k["t1"]) for k in out[0]["kids"]], [("tests::a", "ok", True), ("tests::b", "fail", True)])
        self.assertEqual((out[2]["t1"], out[2]["attrs"]["why"]), (T + 20, "no result line (cut short, or the program crashed)"), "a suite without its result line ends with the output")

    def test_nextest_tests_timed_by_their_own_durations_and_the_summary_repeats_ignored(self):
        out, _ = devjob.parse(lines("""
    Starting 3 tests across 2 binaries (1 test skipped)
        PASS [   0.500s] store tests::a
        SLOW [> 60.000s] pipeline agent::tests::slow
        FAIL [   2.000s] pipeline agent::tests::b
  TRY 2 PASS [   1.000s] pipeline agent::tests::c
     Summary [   5.000s] 3 tests run: 2 passed, 1 failed, 1 skipped
        FAIL [   2.000s] pipeline agent::tests::b
"""), {"tests"}, T, T + 10)
        self.assertEqual(len(out), 1)
        run = out[0]
        self.assertEqual((run["kind"], run["status"], run["t0"], run["t1"]), ("tests", "fail", T, T + 5))
        self.assertEqual([s["name"] for s in run["kids"]], ["store", "pipeline"])
        b = [k for k in run["kids"][1]["kids"] if k["name"] == "agent::tests::b"]
        self.assertEqual(len(b), 1, "the summary's repeat isn't a second run")
        self.assertEqual((b[0]["t0"], b[0]["t1"], b[0]["status"]), (T + 3 - 2.0, T + 3, "fail"))

    def test_remote_tests_phases_and_the_test_mac_run_holding_its_compile_and_tests(self):
        out, attrs = devjob.parse(lines("""
remote-tests: key k is in use on the test Mac (m1 123 x): waiting
remote-tests: got key k after 120 s
remote-tests: tree copied to the test Mac in 1 s (this Mac's load 6.52)
remote-tests: the test Mac's 3 test slots are taken (a b c ): waiting
remote-tests: got a test-Mac slot after 5 s
remote-tests: on the test Mac Mac-mini-M4 (load 13.09, slot 1, /x)
   Compiling pipeline v0.1.0
    Finished `test` profile [optimized] target(s) in 1s
     Running unittests src/lib.rs (/x/target/debug/deps/store-0123456789abcdef)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.50s
remote-tests: the run took 5.00s on the test Mac, cpu 9.00s user + 1.00s sys
remote-tests: exit 0 on the test Mac (load 12.00)
""", step=1.0), {"cargo", "tests", "remote"}, T, T + 12)
        top = [(s["kind"], s["name"]) for s in out]
        self.assertEqual(top, [("wait", "the test Mac's key"), ("copy", "copy to the test Mac"), ("wait", "a test-Mac slot"), ("run", "run on the test Mac")])
        run = out[-1]
        self.assertEqual((run["t0"], run["t1"]), (T + 10 - 5.0, T + 10))
        self.assertEqual(sorted(k["kind"] for k in run["kids"]), ["compile", "suite"])
        self.assertEqual(run["attrs"]["ran_on"], "Mac-mini-M4")
        self.assertEqual(attrs, {"exit_on_test_mac": "0", "load_after": "12.00"})


class Timings(unittest.TestCase):
    def test_a_reports_compiled_crates_on_the_clock(self):
        d = self.enterContext(tempfile.TemporaryDirectory())
        f = os.path.join(d, "r.html")
        units = [{"i": 0, "name": "serde", "version": "1.0", "mode": "todo", "target": "", "start": 0.5, "duration": 2.0, "rmeta_time": 1.0},
                 {"i": 1, "name": "pipeline", "version": "0.1.0", "mode": "todo", "target": ' scenic "bin"', "start": 3.0, "duration": 10.0},
                 {"i": 2, "name": "fresh", "version": "1", "mode": "todo", "target": "", "start": 0.0, "duration": 0.0}]
        with open(f, "w") as o:
            o.write("<td>Fresh units:</td><td>1</td><td>Dirty units:</td><td>2</td><td>Build start:</td><td>2027-01-15T08:00:00.250000Z</td>\n"
                    f"<script>\nconst UNIT_DATA = {json.dumps(units)};\nconst CONCURRENCY_DATA = [];\n</script>")
        was = devjob.SPOOL, devjob.DIR
        with tempfile.TemporaryDirectory() as spool:
            devjob.SPOOL = os.path.join(spool, "s.ndjson")
            devjob.DIR = spool
            self.addCleanup(lambda: (setattr(devjob, "SPOOL", was[0]), setattr(devjob, "DIR", was[1])))
            self.assertEqual(devjob.timings(f, "build-1", "m"), 2)
            with open(devjob.SPOOL) as o:
                ev = [json.loads(x) for x in o]
        start = __import__("calendar").timegm((2027, 1, 15, 8, 0, 0)) + 0.25
        self.assertEqual(ev[0], {"op": "attr", "id": "build-1", "attrs": {"fresh": "1", "dirty": "2"}})
        self.assertEqual([(e["name"], e["t0"], e["t1"], e["parent"]) for e in ev[1:]], [("serde", round(start + 0.5, 3), round(start + 2.5, 3), "build-1"), ("pipeline (scenic bin)", round(start + 3, 3), round(start + 13, 3), "build-1")])


class Board(http.server.BaseHTTPRequestHandler):
    got = []

    def do_POST(self):
        body = self.rfile.read(int(self.headers["content-length"]))
        Board.got += [json.loads(x) for x in body.decode().splitlines() if x]
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"accepted":1,"refused":[]}')

    def log_message(self, *a):
        pass


class Wrap(unittest.TestCase):
    def test_a_wrapped_command_its_status_output_and_spans_reach_the_app(self):
        srv = http.server.HTTPServer(("127.0.0.1", 0), Board)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        with tempfile.TemporaryDirectory() as d:
            env = dict(os.environ, SCENIC_DEVJOB_DIR=d, SCENIC_BOARD=f"http://127.0.0.1:{srv.server_port}", SCENIC_DEVJOB_PARENT="p-1", SCENIC_DEVJOB_MAC="toy")
            env.pop("SCENIC_DEVJOB", None)
            script = 'echo "   Compiling a v1"; echo "    Finished x"; echo "child $SCENIC_DEVJOB_PARENT"; exit 3'
            r = subprocess.run([sys.executable, devjob.__file__, "wrap", "--kind", "part", "--name", "toy", "--parse", "cargo", "--", "sh", "-c", script], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 3, r.stderr)
            self.assertIn("   Compiling a v1\n", r.stdout, "the output passed on")
            child = [l for l in r.stdout.splitlines() if l.startswith("child ")][0].split()[1]
            # The sampler sends the spool and exits once nothing is live.
            deadline = time.time() + 60
            while time.time() < deadline and (os.listdir(os.path.join(d, "live")) or os.path.exists(os.path.join(d, "spool.ndjson")) or [f for f in os.listdir(d) if f.startswith("sending-")]):
                time.sleep(0.1)
            time.sleep(0.5)
        srv.shutdown()
        srv.server_close()
        ops = [(e["op"], e.get("kind")) for e in Board.got if e["op"] != "sample"]
        self.assertEqual(ops, [("start", "part"), ("span", "compile"), ("end", None)])
        start = Board.got[0]
        self.assertEqual((start["parent"], start["mac"], start["id"]), ("p-1", "toy", child), "its children record under it")
        end = [e for e in Board.got if e["op"] == "end"][0]
        self.assertEqual((end["status"], end["attrs"]["exit"]), ("fail", "3"))
        self.assertTrue(any(e["op"] == "sample" and e["mac"] == "toy" for e in Board.got), "a sample of the Mac")

    def test_a_test_mac_run_is_that_macs_and_the_job_says_where_it_ran_as_it_runs(self):
        with tempfile.TemporaryDirectory() as d:
            env = dict(os.environ, SCENIC_DEVJOB_DIR=d, SCENIC_BOARD="http://127.0.0.1:9", SCENIC_DEVJOB_MAC="here")
            env.pop("SCENIC_DEVJOB", None)
            env.pop("SCENIC_DEVJOB_PARENT", None)
            script = ("echo 'remote-tests: on the test Mac there (load 1.00, slot 1, /x)'; sleep 0.2; "
                      "echo '     Running unittests src/lib.rs (/x/target/debug/deps/store-0123456789abcdef)'; "
                      "echo 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s'; "
                      "echo 'remote-tests: the run took 0.30s on the test Mac, cpu 0.10s user + 0.10s sys'; "
                      "echo 'remote-tests: the test Mac run ended without a result (x): running here'; "
                      "echo '--- remote result discarded, local rerun ---'")
            r = subprocess.run([sys.executable, devjob.__file__, "wrap", "--kind", "tests", "--name", "t", "--parse", "remote,tests", "--", "sh", "-c", script], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            for _ in range(600):
                try:
                    with open(os.path.join(d, "sampler.lock")) as f:
                        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except OSError:
                    time.sleep(0.1)
            ev = []
            for f in sorted(os.listdir(d)):
                if f.startswith(("sending-", "spool")):
                    with open(os.path.join(d, f)) as o:
                        ev += [json.loads(x) for x in o if '"sample"' not in x]
        job = ev[0]["id"]
        self.assertEqual([e["op"] for e in ev], ["start", "attr", "attr", "span", "span", "end"])
        self.assertEqual([e["attrs"] for e in ev[1:3]], [{"ran_on": "there"}, {"rerun_here": "the test Mac's run ended without a result"}], "said as the lines come, before the job ends")
        self.assertEqual({e["op"] == "attr" and e["id"] for e in ev[1:3]}, {job})
        self.assertEqual([(e["kind"], e["mac"]) for e in ev if e["op"] in ("start", "span")], [("tests", "here"), ("run", "there"), ("suite", "there")],
                         "the job on this Mac; the run on the test Mac and its suite on that Mac")

    def test_a_command_that_cant_run_is_127_and_its_span_ended(self):
        with tempfile.TemporaryDirectory() as d:
            env = dict(os.environ, SCENIC_DEVJOB_DIR=d, SCENIC_BOARD="http://127.0.0.1:9", SCENIC_DEVJOB_MAC="toy")
            env.pop("SCENIC_DEVJOB", None)
            r = subprocess.run([sys.executable, devjob.__file__, "wrap", "--kind", "part", "--name", "x", "--", os.path.join(d, "no-such")], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 127, r.stdout + r.stderr)
            self.assertEqual(os.listdir(os.path.join(d, "live")), [])
            # (The sampler it started ends at once: nothing is live.)
            for _ in range(600):
                try:
                    with open(os.path.join(d, "sampler.lock")) as f:
                        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except OSError:
                    time.sleep(0.1)

    def test_a_span_whose_process_is_gone_is_ended_gone(self):
        with tempfile.TemporaryDirectory() as d:
            env = dict(os.environ, SCENIC_DEVJOB_DIR=d, SCENIC_BOARD="http://127.0.0.1:9", SCENIC_DEVJOB_MAC="toy")
            p = subprocess.Popen(["sleep", "30"])
            os.makedirs(os.path.join(d, "live"))
            with open(os.path.join(d, "live", "toy-1"), "w") as o:
                o.write(f"{p.pid} {devjob.lstart(p.pid)}\n")
            p.kill()
            p.wait()
            subprocess.run([sys.executable, devjob.__file__, "sampler"], env=env, timeout=60)
            ev = []
            for f in sorted(os.listdir(d)):
                if f.startswith(("sending-", "spool")):
                    with open(os.path.join(d, f)) as o:
                        ev += [json.loads(x) for x in o]
            self.assertIn(("end", "toy-1", "gone"), [(e["op"], e.get("id"), e.get("status")) for e in ev], "kept to send (the app away)")
            self.assertEqual(os.listdir(os.path.join(d, "live")), [])

    def test_a_spool_that_cant_be_written_runs_the_command_still(self):
        with tempfile.TemporaryDirectory() as d:
            spool = os.path.join(d, "a-file")
            open(spool, "w").close()  # (the spool's folder a file: every write fails)
            env = dict(os.environ, SCENIC_DEVJOB_DIR=spool, SCENIC_BOARD="http://127.0.0.1:9", SCENIC_DEVJOB_MAC="toy")
            env.pop("SCENIC_DEVJOB", None)
            env.pop("SCENIC_DEVJOB_PARENT", None)
            r = subprocess.run([sys.executable, devjob.__file__, "wrap", "--kind", "part", "--name", "x", "--", "sh", "-c", "echo ran; exit 4"], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual((r.returncode, r.stdout), (4, "ran\n"), r.stderr)


class Flush(unittest.TestCase):
    def test_the_app_away_leaves_one_batch_not_a_file_a_tick(self):
        with tempfile.TemporaryDirectory() as d:
            old = devjob.DIR, devjob.SPOOL
            devjob.DIR, devjob.SPOOL = d, os.path.join(d, "spool.ndjson")
            try:
                for i in range(3):
                    devjob.emit({"op": "attr", "id": f"x-{i}", "attrs": {}})
                    self.assertFalse(devjob.flush(lambda m: None, "http://127.0.0.1:9/api/jobs/events"))
            finally:
                devjob.DIR, devjob.SPOOL = old
            self.assertEqual(len([f for f in os.listdir(d) if f.startswith("sending-")]), 1)
            with open(os.path.join(d, "spool.ndjson")) as f:
                self.assertEqual(len(f.readlines()), 2, "what came after waits in the spool")

    def test_a_name_past_the_apps_limit_is_cut_not_refused(self):
        # (#1123: a cargo's name is its command line; check.sh's nextest ran to 4 KB, the app refused
        # it, and its span and every span under it were lost.)
        with tempfile.TemporaryDirectory() as d:
            old = devjob.DIR, devjob.SPOOL
            devjob.DIR, devjob.SPOOL = d, os.path.join(d, "spool.ndjson")
            try:
                devjob.emit({"op": "start", "id": "x-1", "kind": "cargo", "name": "n" * 5000, "attrs": {}})
                devjob.emit({"op": "span", "id": "x-2", "kind": "wait", "name": "short", "attrs": {}})
            finally:
                devjob.DIR, devjob.SPOOL = old
            with open(os.path.join(d, "spool.ndjson")) as f:
                long, short = (json.loads(x)["name"] for x in f)
            self.assertEqual((len(long), long[-1]), (devjob.NAME_MAX, "…"))
            self.assertEqual(short, "short")


if __name__ == "__main__":
    unittest.main()
