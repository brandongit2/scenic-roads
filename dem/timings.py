"""A step program's timings: the Python twin of pipeline::timings (docs/building.md §2, Timings).

Its run in named phases, each with its wall time, CPU time (user + system, this process's and its
finished children's), its wall time split by what it waited on (measured, as store::waits does)
and the bytes and files it moved:

    from timings import phase, count, reading
    with phase("reading the months"):
        with reading(path):
            b = path.read_bytes()
        count(len(b), 1)

- A phase opened inside another is its sub-phase; one inside a sub-phase is folded into it (one
  level only). A phase opened again under the same name adds to it (`n` counts its spans): a loop's
  stage is one phase, with totals, never one per item.
- Waits (task #870): a call that blocks holds `wait("nas" | "net" | "disk" | "other")` (`reading(path)`:
  the NAS's or this Mac's disk, by the file system `path` is on; `lines(path)`: a file's lines, each
  read under it). By the process's wall clock, as pipeline::timings: at each instant the process is
  waiting on the NAS when any thread is in a NAS call, else on the network, else on the disk, else
  on other work, else computing; a phase's waits are the difference of those totals between its
  ends, so they never sum past its wall time. No phase is classed by hand.
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

# What a wait is on, in the order an instant is given to (the first with a call in it wins).
KINDS = ("nas", "net", "disk", "other")

_lock = threading.Lock()
_main = threading.main_thread().ident
_t0 = time.monotonic()
_start = int(time.time())
# (name, parent name or None) -> totals, in the order first opened.
_accs: dict = {}
_open: list = []  # open spans, every thread's
_local = threading.local()
# The waits: calls in progress by kind, when that last changed, and each kind's seconds so far.
_wlock = threading.Lock()
_active = [0] * len(KINDS)
_since = time.monotonic()
_acc = [0.0] * len(KINDS)


def _settle(now: float) -> None:
    """The time since the last change given to the kind that held it (none: computing)."""
    global _since
    for k, n in enumerate(_active):
        if n:
            _acc[k] += now - _since
            break
    _since = now


def _waits() -> list:
    with _wlock:
        _settle(time.monotonic())
        return list(_acc)


class wait:
    """A blocking call on `kind` (one of KINDS) in progress, as a context manager."""

    def __init__(self, kind: str):
        self.k = KINDS.index(kind)

    def __enter__(self) -> "wait":
        with _wlock:
            _settle(time.monotonic())
            _active[self.k] += 1
        return self

    def __exit__(self, *exc) -> None:
        with _wlock:
            _settle(time.monotonic())
            _active[self.k] -= 1


_MNT_LOCAL = 0x1000
_local_dev: dict = {}


def on(path) -> str:
    """"nas" when `path` (or the nearest folder above it there) is on a network file system (the
    NAS's share), else "disk"."""
    p = os.path.abspath(os.fspath(path))
    while not os.path.exists(p) and os.path.dirname(p) != p:
        p = os.path.dirname(p)
    try:
        dev = os.stat(p).st_dev
    except OSError:
        return "disk"
    if dev not in _local_dev:
        _local_dev[dev] = _is_local(p)
    return "disk" if _local_dev[dev] else "nas"


def _is_local(p: str) -> bool:
    # (macOS's statfs, 64-bit inodes: f_flags at byte 64, MNT_LOCAL set for a local file system;
    # elsewhere, or failing, taken as local.)
    if sys.platform != "darwin":
        return True
    try:
        import ctypes
        import struct
        libc = ctypes.CDLL(None, use_errno=True)
        buf = ctypes.create_string_buffer(4096)
        if libc.statfs(os.fsencode(p), buf) != 0:
            return True
        return bool(struct.unpack_from("<I", buf.raw, 64)[0] & _MNT_LOCAL)
    except (OSError, AttributeError):
        return True


def reading(path) -> wait:
    """A call on file `path` in progress: the NAS's or this Mac's disk (`on`)."""
    return wait(on(path))


def lines(path, **kw):
    """The lines of text file `path`, each read under `reading(path)` (the work between them not)."""
    w = reading(path)
    with w:
        f = open(path, **kw)
    with f:
        while True:
            with w:
                line = f.readline()
            if not line:
                return
            yield line


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

    def __init__(self, name: str):
        self.name, self.key = name, None

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
            _accs.setdefault(self.key, {"name": self.name, "wall_s": 0.0, "waits": [0.0] * len(KINDS), "cpu_s": 0.0, "n": 0, "bytes": 0, "files": 0, "overlapped": False, "background": me != _main})
            for o in _open:
                if o.thread != me:
                    o.overlapped = True
                    self.overlapped = True
            self.thread = me
            _open.append(self)
        st.append(self)
        self.t, self.c, self.w = time.monotonic(), _cpu(), _waits()
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
        wall, cpu, w = time.monotonic() - self.t, _cpu() - self.c, _waits()
        with _lock:
            if self in _open:
                _open.remove(self)
            a = _accs[self.key]
            a["wall_s"] += wall
            a["cpu_s"] += cpu
            a["waits"] = [x + max(0.0, b - c) for x, b, c in zip(a["waits"], w, self.w)]
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
        r = {k: v for k, v in a.items() if k != "waits" and (v or k in ("name", "wall_s", "n"))}
        r["cpu_s"] = a["cpu_s"]
        r["waits"] = {f"{k}_s": min(s, a["wall_s"]) for k, s in zip(KINDS, a["waits"])}
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
    # (A pool's worker process imports this too: the step's own process writes them.)
    import multiprocessing
    if not to or multiprocessing.parent_process() is not None:
        return
    try:
        tmp = f"{to}.{os.getpid()}.tmp"
        with open(tmp, "w") as f:
            json.dump(record(), f)
        os.replace(tmp, to)
    except OSError:
        pass


atexit.register(_write)
