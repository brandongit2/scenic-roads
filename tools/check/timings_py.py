"""dem/timings.py's phases, as a step program writes them for the scenic-build phase that runs it
(SCENIC_PHASES_TO): run by crates/pipeline/tests/timings_python.rs, which reads them back; offline,
nothing read or written but that file."""
from __future__ import annotations

import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent / "dem"))
from timings import count, phase, record, sub, wait  # noqa: E402

for _ in range(3):
    with phase("loop stage") as p:
        p.count(10, 1)
        time.sleep(0.01)
with phase("parent"):
    with sub("inner"):
        count(100, 2)
        with sub("folded"):
            count(1, 0)
# Waits by wall clock: two threads on the NAS for the same 50 ms are 50 ms; the network beside a
# NAS call is the NAS's; alone, its own.
with phase("waiting"):
    def nas():
        with wait("nas"):
            time.sleep(0.05)
    ts = [threading.Thread(target=nas) for _ in range(2)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    with wait("net"):
        time.sleep(0.03)
    # (Task #891: the network beside the NAS, then computing beside the network: each pair's.)
    # Proven by events, not by timing (task #921): the NAS call is in progress before the network's
    # begins and ends only after it; the network's call is in progress while this thread spins
    # until the process has 40 ms more CPU time, however long a loaded Mac takes to give it.
    # Every event's wait bounded (WATCHDOG, as tools/dev/testing.md's): a stuck thread fails the
    # run, never hangs it.
    WATCHDOG = 300

    def held(kind, began, done):
        with wait(kind):
            began.set()
            assert done.wait(WATCHDOG), "never released"

    def begun(began):
        assert began.wait(WATCHDOG), "the helper thread never began its wait"
    began, done = threading.Event(), threading.Event()
    t = threading.Thread(target=held, args=("nas", began, done))
    t.start()
    begun(began)
    with wait("net"):
        time.sleep(0.03)
    done.set()
    t.join()
    began, done = threading.Event(), threading.Event()
    t = threading.Thread(target=held, args=("net", began, done))
    t.start()
    begun(began)
    spin = time.process_time()
    while time.process_time() - spin < 0.04:
        pass
    done.set()
    t.join()
with phase("main work"):
    t = threading.Thread(target=lambda: phase("beside").start().end())
    t.start()
    t.join()


# Task #1116: phases at once on two threads, each its own CPU (60 ms of its own thread's), not the
# other's; the phases' CPU and the CPU in no phase add up to the run's.
def spin(name):
    with phase(name):
        t0 = time.thread_time()
        while time.thread_time() - t0 < 0.06:
            pass


with phase("at once"):
    ts = [threading.Thread(target=spin, args=(n,)) for n in ("one thread", "another")]
    [t.start() for t in ts]
    [t.join() for t in ts]
r = record()
mine = {p["name"]: p["cpu_s"] for p in r["phases"]}
assert all(0.055 <= mine[n] <= 0.1 for n in ("one thread", "another")), mine
total = sum(p["cpu_s"] for p in r["phases"]) + r["untimed_cpu_s"]
assert abs(total - r["cpu_s"]) <= 0.005 + 0.01 * r["cpu_s"], (total, r["cpu_s"])
