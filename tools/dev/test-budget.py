"""The test budget, measured (#1553): the slowest tests of the last publishes' suites, each run alone on
the test Mac, any over the budget flagged on the dev job dashboard (tools/devtools, /jobs/).

  test-budget.py run <tree>   publish.sh's step after its suite passed: when one is due, picks the
                              tests, runs them alone (tools/app/remote-tests.sh, publish's key, so the
                              suite's build: nothing compiles) and records the measure
  test-budget.py pick         the tests a measure would run now, and why each (nothing run)

Its settings are tools/dev/test-budget.json, each shown on the dashboard with the measure it made:
  budget_s    a test's budget alone (s): one over it is flagged
  every_h     a measure is due this long after the last (h)
  publishes   the tests are picked from the suites of the last this many publishes that ran one
  slowest     of the tests whose median there is at or over the budget, the slowest this many run
(A test under the budget beside the suite's others is under it alone: the suite runs every core's
worth of tests at once, so those under it there are left out; the cap keeps a measure to minutes.)

The tests are picked from the dev job records (the app's /api/jobs: each publish's run, its suites'
`test` spans, a suite named by nextest's binary id), not from the measures' own runs. The measure is
a `budget` span under the publish's step (its `tests` job under it, as remote-tests.sh records one),
its attributes the settings, `alone` ([binary, test, seconds alone, median in the suites]) and `over`
(those past the budget); the dashboard shows the latest (`/api/jobs/latest?kind=budget`). It runs
`--test-threads 1`, one test at a time, in publish's slot on the test Mac, beside whatever else runs
there (the pool's jobs, other slots' checks): its run's load is on the dashboard with it.
It never fails a publish: a measure that can't run (the app away, the test Mac refusing) says why.
"""
import json
import os
import shlex
import statistics
import subprocess
import sys
import time
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import devjob  # noqa: E402

SETTINGS = os.path.join(HERE, "test-budget.json")
UA = {"user-agent": "scenic-roads/0.1 (personal offline map)"}


def settings():
    with open(SETTINGS) as f:
        s = json.load(f)
    for k in ("budget_s", "every_h", "publishes", "slowest"):
        if not isinstance(s.get(k), (int, float)) or s[k] <= 0:
            sys.exit(f"test-budget.py: {SETTINGS}: {k} must be a number above 0")
    return s


def get(path):
    with urllib.request.urlopen(urllib.request.Request(devjob.board() + "/api/jobs/" + path, headers=UA), timeout=60) as r:
        return json.load(r)


def last_measure():
    """The latest measure recorded (its span), or None."""
    return get("latest?kind=budget").get("span")


def suite_times(run):
    """{(binary, test): seconds} of a publish's run (`/api/jobs/runs/<id>`), its suites' tests that
    ended, none under a measure of the budget."""
    spans = run["spans"]
    by = {s["id"]: s for s in spans}

    def under_budget(s):
        while s is not None:
            if s["kind"] == "budget":
                return True
            s = by.get(s.get("parent"))
        return False

    out = {}
    for s in spans:
        p = by.get(s.get("parent"))
        if s["kind"] == "test" and p is not None and p["kind"] == "suite" and s.get("t1") is not None and not s["attrs"].get("untimed") and not under_budget(s):
            out[(p["name"], s["name"])] = s["t1"] - s["t0"]
    return out


def pick(s, now):
    """(the tests to run, slowest first: [(binary, test, median)], how many were at or over the
    budget, the publishes read)."""
    runs = get(f"runs?kind=publish&from={now - 14 * 86400:.0f}&to={now:.0f}")["runs"]
    times, read = {}, []
    for r in sorted((r for r in runs if r.get("t1") is not None), key=lambda r: -r["t0"]):
        got = suite_times(get("runs/" + urllib.parse.quote(r["id"], safe="")))
        if not got:
            continue   # (a publish stopped before its suite)
        read.append(r["id"])
        for k, v in got.items():
            times.setdefault(k, []).append(v)
        if len(read) == s["publishes"]:
            break
    meds = sorted(((b, t, statistics.median(v)) for (b, t), v in times.items()), key=lambda x: -x[2])
    over = [m for m in meds if m[2] >= s["budget_s"]]
    return over[: int(s["slowest"])], len(over), read


def remote(argv, env):
    """remote-tests.sh's run (`argv` its arguments), its output read as it comes."""
    return subprocess.Popen(["zsh", os.path.join(HERE, "..", "app", "remote-tests.sh"), *argv], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, env=env)


def filterset(tests):
    return " | ".join(f"(binary_id(={b}) & test(={t}))" for b, t, _ in tests)


def run(tree):
    s, now = settings(), time.time()
    try:
        last = last_measure()
        if last and last.get("t0") and now - last["t0"] < s["every_h"] * 3600:
            print(f"test budget: not due (last measured {time.strftime('%F %T', time.localtime(last['t0']))}, every {s['every_h']} h)")
            return 0
        tests, n_over, read = pick(s, now)
    except (OSError, ValueError, KeyError) as e:
        print(f"test budget: NOT MEASURED: the dev job records unreadable ({e})")
        return 0
    attrs = {"budget_s": s["budget_s"], "every_h": s["every_h"], "publishes": s["publishes"], "slowest": s["slowest"],
             "read": len(read), "at_or_over_in_suites": n_over, "chosen": len(tests)}
    sid = devjob.new_id()
    if devjob.on():
        devjob.emit({"op": "start", "id": sid, "parent": os.environ.get("SCENIC_DEVJOB_PARENT") or None, "kind": "budget", "name": "test budget",
                     "mac": devjob.mac(), "t": time.time(), "attrs": attrs})
        devjob.live(sid, os.getpid())
    if not tests:
        print(f"test budget: none of the last {len(read)} publishes' tests took {s['budget_s']} s or more")
        alone, over, rc = [], [], 0
    else:
        print(f"test budget: {len(tests)} tests alone (the slowest of the {n_over} at or over {s['budget_s']} s in the last {len(read)} publishes' suites)")
        cmd = ("export SCENIC_PUBLISH=1; cargo nextest --version >/dev/null 2>&1 || { mise install -q; eval \"$(mise env -s zsh)\"; }; "
               f"cargo nextest run --no-fail-fast --hide-progress-bar --workspace --test-threads 1 -E {shlex.quote(filterset(tests))}")
        env = dict(os.environ, SCENIC_DEVJOB_PARENT=sid)
        p = remote(["publish", tree, cmd], env)
        took = {}
        for line in p.stdout:
            sys.stdout.write(line)
            if m := devjob.NX_TEST.match(line):
                took[(m.group(3), m.group(4))] = float(m.group(2))
        rc = p.wait()
        med = {(b, t): m for b, t, m in tests}
        alone = [[b, t, round(took[(b, t)], 2), round(med[(b, t)], 2)] for b, t, _ in tests if (b, t) in took]
        over = [a[:3] for a in alone if a[2] > s["budget_s"]]
        if len(alone) < len(tests):
            print(f"test budget: {len(tests) - len(alone)} of the tests didn't run (remote-tests.sh exit {rc})")
        for b, t, secs in over:
            print(f"test budget: OVER {s['budget_s']} s alone: {b} {t}: {secs:.1f} s")
        if alone and not over:
            print(f"test budget: every test measured is under {s['budget_s']} s alone (the slowest {max(a[2] for a in alone):.1f} s)")
    if devjob.on():
        devjob.emit({"op": "end", "id": sid, "t": time.time(), "status": "ok" if rc == 0 else "fail",
                     "attrs": {"alone": alone, "over": over, "exit": rc}})
        devjob.unlive(sid)
    return 0


def main(argv):
    if argv[:1] == ["run"] and len(argv) == 2:
        return run(argv[1])
    if argv == ["pick"]:
        s = settings()
        tests, n_over, read = pick(s, time.time())
        print(f"the slowest {len(tests)} of the {n_over} tests at or over {s['budget_s']} s (median) in the last {len(read)} publishes' suites:")
        for b, t, m in tests:
            print(f"  {m:7.1f} s  {b} {t}")
        return 0
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
