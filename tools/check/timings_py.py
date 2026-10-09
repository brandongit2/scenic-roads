"""dem/timings.py's phases, as a step program writes them for the scenic-build phase that runs it
(SCENIC_PHASES_TO): run by crates/pipeline/tests/timings_python.rs, which reads them back; offline,
nothing read or written but that file."""
from __future__ import annotations

import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent / "dem"))
from timings import count, phase, sub  # noqa: E402

for _ in range(3):
    with phase("loop stage", "compute") as p:
        p.count(10, 1)
        time.sleep(0.01)
with phase("parent", "mixed"):
    with sub("inner", "disk"):
        count(100, 2)
        with sub("folded", "net"):
            count(1, 0)
with phase("main work", "compute"):
    t = threading.Thread(target=lambda: phase("beside", "nas-read").start().end())
    t.start()
    t.join()
