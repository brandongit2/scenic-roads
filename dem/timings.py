"""A step program's timings: the Python twin of jobrt::timings (docs/building.md §2, Timings).

Its run in named phases, each with its wall time, its own CPU time (task #1116, as jobrt::timings:
each thread's, `time.thread_time`, charged to the span innermost on it as it opens or closes one;
the process's CPU no thread was charged, its threads in no phase and its finished children, to the
main thread's innermost span as that changes, else the run's `untimed_cpu_s`), its wall time split by what it waited on (measured, as store::waits does)
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
  read under it). By the process's wall clock, as jobrt::timings: at each instant the process is
  waiting on the NAS when any thread is in a NAS call, else on the network, else on the disk, else
  on other work, else computing; a phase's waits are the difference of those totals between its
  ends, so they never sum past its wall time. No phase is classed by hand. Beside them, as
  store::waits (task #891), the seconds two ran at once by PAIRS: two kinds' calls both in
  progress, and a kind with computing (the process's CPU time while that kind held the instant,
  at most that time): `both_s`, extra, never in the sum.
- Run by a step (scenic-build, within one of its phases), the phases go where `SCENIC_PHASES_TO`
  says as the program exits, and come in as that phase's sub-phases. Run by hand: nothing kept.
- A phase on a thread but the main one is marked `background`; a phase while another thread's is
  open, `overlapped`. Python reads no other thread's CPU, so a thread's comes in as it opens or
  closes a span, and the CPU in no phase isn't settled while another thread has a span open: it
  goes to the main thread's span at its first change after, said in `uncertain_cpu_s`, as is any
  pool work while another's span was open. A main thread's top-level phase keeps the process's
  CPU while it was open, `job_cpu_s`.
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
# The pairs whose time at once is counted (store::waits::PAIRS): the waits two by two, then each
# with computing (CPU, 4).
CPU = len(KINDS)
PAIRS = ((0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3), (0, CPU), (1, CPU), (2, CPU), (3, CPU))
_WAIT_PAIRS = 6

_lock = threading.Lock()
_main = threading.main_thread().ident
_t0 = time.monotonic()
_start = int(time.time())
# (name, parent name or None) -> totals, in the order first opened.
_accs: dict = {}
_open: list = []  # open spans, every thread's
_local = threading.local()
# The CPU (task #1116): what threads were charged so far; the process's CPU and that when the CPU
# in no phase was last settled; whether a thread but the main one had a span open since; the CPU
# in no phase while the main thread had none.
_charged = 0.0
_settled = (0.0, 0.0)  # (the record's CPU is the process's since it began)
_others = False
_untimed_cpu = 0.0
# The waits: calls in progress by kind, when that last changed, each kind's seconds so far, then
# each of PAIRS'; the kind holding the instant, since when and the process's CPU time then.
_wlock = threading.Lock()
_active = [0] * len(KINDS)
_since = time.monotonic()
_acc = [0.0] * (len(KINDS) + len(PAIRS))
_top = None
_top_since = _since
_top_cpu = 0.0


def _settle(now: float) -> None:
    """The time since the last change given to the kind that held it (none: computing), and to
    the pairs of kinds both in progress."""
    global _since
    if _top is not None:
        d = now - _since
        _acc[_top] += d
        for p, (i, j) in enumerate(PAIRS[:_WAIT_PAIRS]):
            if _active[i] and _active[j]:
                _acc[len(KINDS) + p] += d
    _since = now


def _close(now: float) -> None:
    """The holding kind's time with computing since `_top_since` (the CPU time meanwhile, at most
    that time); a new span from `now`."""
    global _top_since, _top_cpu
    cpu = time.process_time()
    if _top is not None and KINDS[_top] != "disk":  # (the disk's: its own CPU, not counted)
        _acc[len(KINDS) + _WAIT_PAIRS + _top] += min(max(0.0, cpu - _top_cpu), now - _top_since)
    _top_since, _top_cpu = now, cpu


def _retop(now: float) -> None:
    global _top
    top = next((k for k, n in enumerate(_active) if n), None)
    if top != _top:
        _close(now)
        _top = top


def _waits() -> list:
    with _wlock:
        now = time.monotonic()
        _settle(now)
        if _top is not None:
            _close(now)
        return list(_acc)


class wait:
    """A blocking call on `kind` (one of KINDS) in progress, as a context manager."""

    def __init__(self, kind: str):
        self.k = KINDS.index(kind)

    def __enter__(self) -> "wait":
        with _wlock:
            now = time.monotonic()
            _settle(now)
            _active[self.k] += 1
            _retop(now)
        return self

    def __exit__(self, *exc) -> None:
        with _wlock:
            now = time.monotonic()
            _settle(now)
            _active[self.k] -= 1
            _retop(now)


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


def _innermost(st: list):
    """The innermost span on a thread's stack that keeps totals (a folded one's are its parent's)."""
    return next((p for p in reversed(st) if p.key is not None), None)


def _charge(p, cpu: float, unsure: bool = False) -> None:
    while p is not None:
        p.cpu += cpu
        if unsure:
            p.unsure += cpu
        p = p.parent


def _settle_thread() -> None:
    """This thread's CPU since it was last charged, to its innermost span (under `_lock`)."""
    global _charged
    now = time.thread_time()
    d = max(0.0, now - getattr(_local, "mark", now))
    _local.mark = now
    p = _innermost(_stack())
    if p is not None:
        _charge(p, d)
        _charged += d


def _settle_loose() -> float:
    """The process's CPU since the last settle that no thread was charged (threads in no phase,
    child programs reaped) to the main thread's innermost span, or the run's CPU in no phase (under
    `_lock`). Another thread's CPU comes in only as it changes span (Python reads no other thread's
    CPU), so while one has a span open nothing is settled: its CPU so far can't be told from the
    CPU in no phase (taken in then, it would be counted twice as the thread's span ends). It's
    settled at the first change of the main thread's span after, said uncertain (another's span
    was open). The process's CPU now."""
    global _settled, _others, _untimed_cpu
    now = _cpu()
    if any(o.thread != _main for o in _open):
        _others = True
        return now
    loose = (now - _settled[0]) - (_charged - _settled[1])
    if loose >= 0:
        main = next((o for o in reversed(_open) if o.thread == _main), None)
        if main is not None:
            _charge(main, loose, _others)
        else:
            _untimed_cpu += loose
        _settled = (now, _charged)
        _others = any(o.thread != _main for o in _open)
    return now


class phase:
    """A phase, as a context manager (or `start()`/`end()`)."""

    def __init__(self, name: str):
        self.name, self.key = name, None

    def start(self) -> "phase":
        global _others
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
        self.parent, self.cpu, self.unsure, self.job0 = parent, 0.0, 0.0, None
        me = threading.get_ident()
        with _lock:
            # (The CPU so far charged to the span this thread was in, the main thread's with the
            # process's in no phase.)
            _settle_thread()
            if me == _main:
                job = _settle_loose()
                if parent is None:
                    self.job0 = job
            else:
                _others = True
            _accs.setdefault(self.key, {"name": self.name, "wall_s": 0.0, "waits": [0.0] * (len(KINDS) + len(PAIRS)), "cpu_s": 0.0, "uncertain_cpu_s": 0.0, "n": 0, "bytes": 0, "files": 0, "overlapped": False, "background": me != _main})
            for o in _open:
                if o.thread != me:
                    o.overlapped = True
                    self.overlapped = True
            self.thread = me
            _open.append(self)
            st.append(self)
        self.t, self.w = time.monotonic(), _waits()
        return self

    def count(self, nbytes: int = 0, files: int = 0) -> None:
        if self.key is not None:
            self.bytes += nbytes
            self.files += files
        else:
            count(nbytes, files)

    def end(self) -> None:
        st = _stack()
        if self.key is None:
            if self in st:
                st.remove(self)
            return
        wall, w = time.monotonic() - self.t, _waits()
        with _lock:
            # (Its CPU to now: this thread's, and the main thread's span the process's in no phase.)
            _settle_thread()
            job = _settle_loose() if self.thread == _main else None
            if self in st:
                st.remove(self)
            if self in _open:
                _open.remove(self)
            a = _accs[self.key]
            a["wall_s"] += wall
            a["cpu_s"] += self.cpu
            a["uncertain_cpu_s"] += self.unsure
            if self.job0 is not None and job is not None:
                a["job_cpu_s"] = a.get("job_cpu_s", 0.0) + job - self.job0
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
    """The run so far, as jobrt::timings's record has it."""
    with _lock:
        _settle_thread()
        cpu = _settle_loose()
        accs = dict(_accs)
        untimed_cpu = _untimed_cpu
    def rec(key, a):
        r = {k: v for k, v in a.items() if k != "waits" and (v or k in ("name", "wall_s", "n"))}
        r["cpu_s"] = a["cpu_s"]
        r["waits"] = {f"{k}_s": min(s, a["wall_s"]) for k, s in zip(KINDS, a["waits"])}
        both = [min(s, a["wall_s"]) for s in a["waits"][len(KINDS):]]
        if any(both):
            r["waits"]["both_s"] = both
        subs = [rec(k, s) for k, s in accs.items() if k[1] == key[0] and key[1] is None]
        if subs:
            r["sub"] = subs
        return r
    phases = [rec(k, a) for k, a in accs.items() if k[1] is None and a["n"] > 0]
    wall = time.monotonic() - _t0
    timed = sum(p["wall_s"] for p in phases if not p.get("background"))
    return {"v": 2, "kind": os.path.basename(sys.argv[0]) if sys.argv else "", "id": "", "start": _start, "wall_s": wall, "cpu_s": cpu, "untimed_cpu_s": untimed_cpu, "ok": ok, "untimed_s": max(0.0, wall - timed), "overhead_s": 0.0, "phases": phases}


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
