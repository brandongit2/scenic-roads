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
with phase("main work"):
    t = threading.Thread(target=lambda: phase("beside").start().end())
    t.start()
    t.join()
