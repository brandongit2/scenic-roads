"""dem/timings.py's phases, as a step program writes them for the scenic-build phase that runs it
(SCENIC_PHASES_TO): run by crates/pipeline/tests/timings_python.rs, which reads them back; offline,
nothing read or written but that file."""
from __future__ import annotations

import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent / "dem"))
from timings import count, phase, sub, wait  # noqa: E402

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
    def held(kind, began, done):
        with wait(kind):
            began.set()
            done.wait()
    began, done = threading.Event(), threading.Event()
    t = threading.Thread(target=held, args=("nas", began, done))
    t.start()
    began.wait()
    with wait("net"):
        time.sleep(0.03)
    done.set()
    t.join()
    began, done = threading.Event(), threading.Event()
    t = threading.Thread(target=held, args=("net", began, done))
    t.start()
    began.wait()
    spin = time.process_time()
    while time.process_time() - spin < 0.04:
        pass
    done.set()
    t.join()
with phase("main work"):
    t = threading.Thread(target=lambda: phase("beside").start().end())
    t.start()
    t.join()
