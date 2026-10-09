"""A step program's timings: the Python twin of pipeline::timings (docs/plan.md §8, Timings).

Its run in named phases, each with its wall time, CPU time (user + system, this process's and its
finished children's), class and the bytes and files it moved:

    from timings import phase, count
    with phase("months read", "disk"):
        ...
        count(len(b), 1)

- A phase opened inside another is its sub-phase; one inside a sub-phase is folded into it (one
  level only). A phase opened again under the same name adds to it (`n` counts its spans): a loop's
  stage is one phase, with totals, never one per item.
- Classes: nas-read, nas-write, disk, net, compute, wait, mixed.
- Run by a step (scenic-build, within one of its phases), the phases go where `SCENIC_PHASES_TO`
  says as the program exits, and come in as that phase's sub-phases. Run by hand: nothing kept.
- A phase on a thread but the main one is marked `background`; a phase while another thread's is
  open, `overlapped` (its CPU, the process's, is approximate).
"""

from __future__ import annotations

import atexit
import json
import os
import resource
import sys
import threading
import time

CLASSES = ("nas-read", "nas-write", "disk", "net", "compute", "wait", "mixed")

_lock = threading.Lock()
_main = threading.main_thread().ident
_t0 = time.monotonic()
_start = int(time.time())
# (name, parent name or None) -> totals, in the order first opened.
_accs: dict = {}
_open: list = []  # open spans, every thread's
_local = threading.local()


def _cpu() -> float:
    s = resource.getrusage(resource.RUSAGE_SELF)
    c = resource.getrusage(resource.RUSAGE_CHILDREN)
    return s.ru_utime + s.ru_stime + c.ru_utime + c.ru_stime


def _stack() -> list:
    st = getattr(_local, "stack", None)
    if st is None:
        st = _local.stack = []
    return st


class phase:
    """A phase, as a context manager (or `start()`/`end()`)."""

    def __init__(self, name: str, cls: str = "compute"):
        assert cls in CLASSES, cls
        self.name, self.cls, self.key = name, cls, None

    def start(self) -> "phase":
        st = _stack()
        parent = st[-1] if st else None
        if parent is not None and (parent.key is None or parent.key[1] is not None):
            # In a sub-phase (or a folded one): folded into it.
            self.key = None
            st.append(self)
            return self
        self.key = (self.name, parent.key[0] if parent is not None else None)
        self.bytes = self.files = 0
        self.overlapped = False
        me = threading.get_ident()
        with _lock:
            a = _accs.setdefault(self.key, {"name": self.name, "class": self.cls, "wall_s": 0.0, "cpu_s": 0.0, "n": 0, "bytes": 0, "files": 0, "overlapped": False, "background": me != _main})
            for o in _open:
                if o.thread != me:
                    o.overlapped = True
                    self.overlapped = True
            self.thread = me
            _open.append(self)
        st.append(self)
        self.t, self.c = time.monotonic(), _cpu()
        return self

    def count(self, nbytes: int = 0, files: int = 0) -> None:
        if self.key is not None:
            self.bytes += nbytes
            self.files += files
        else:
            count(nbytes, files)

    def end(self) -> None:
        st = _stack()
        if self in st:
            st.remove(self)
        if self.key is None:
            return
        wall, cpu = time.monotonic() - self.t, _cpu() - self.c
        with _lock:
            if self in _open:
                _open.remove(self)
            a = _accs[self.key]
            a["wall_s"] += wall
            a["cpu_s"] += cpu
            a["n"] += 1
            a["bytes"] += self.bytes
            a["files"] += self.files
            a["overlapped"] |= self.overlapped

    def __enter__(self) -> "phase":
        return self.start()

    def __exit__(self, *exc) -> None:
        self.end()


sub = phase


def count(nbytes: int = 0, files: int = 0) -> None:
    """Adds bytes and files moved to the innermost phase open on this thread (none: nothing)."""
    st = _stack()
    for p in reversed(st):
        if p.key is not None:
            p.bytes += nbytes
            p.files += files
            return


def record(ok: bool = True) -> dict:
    """The run so far, as pipeline::timings's record has it."""
    with _lock:
        accs = dict(_accs)
    def rec(key, a):
        r = {k: v for k, v in a.items() if v or k in ("name", "class", "wall_s", "n")}
        r["cpu_s"] = a["cpu_s"]
        subs = [rec(k, s) for k, s in accs.items() if k[1] == key[0] and key[1] is None]
        if subs:
            r["sub"] = subs
        return r
    phases = [rec(k, a) for k, a in accs.items() if k[1] is None and a["n"] > 0]
    wall = time.monotonic() - _t0
    timed = sum(p["wall_s"] for p in phases if not p.get("background"))
    return {"v": 1, "kind": os.path.basename(sys.argv[0]) if sys.argv else "", "id": "", "start": _start, "wall_s": wall, "cpu_s": _cpu(), "ok": ok, "untimed_s": max(0.0, wall - timed), "overhead_s": 0.0, "phases": phases}


def _write() -> None:
    to = os.environ.get("SCENIC_PHASES_TO")
    if not to:
        return
    try:
        tmp = f"{to}.{os.getpid()}.tmp"
        with open(tmp, "w") as f:
            json.dump(record(), f)
        os.replace(tmp, to)
    except OSError:
        pass


atexit.register(_write)
