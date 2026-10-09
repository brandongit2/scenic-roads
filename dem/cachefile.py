"""The build agent's caches, read and written through one accessor: the Python steps' side of
crates/store/src/cachefile.rs (docs/plan.md §8, Room on the disk), the same protocol, so the
agent's room-making may delete any cache file no job uses, at any moment, mid-job too.

- A job holds each cache file it uses (`hold`): opened, with a shared flock on it, kept until the
  process ends (or `release`). A file deleted between its opening and its lock (no name left) is
  let go and looked for again: filled again (`refill`), never read as missing.
- A file is made by a temporary name, made and locked at once (`scratch`), then named only if the
  name is free (`publish`: a hard link, never a rename over a file there).
- A damaged one goes with `discard`."""
from __future__ import annotations

import errno
import fcntl
import itertools
import os
import time
from pathlib import Path
from typing import Callable

# The files this process holds: (device, inode) -> the open file (its lock with it).
_held: dict[tuple[int, int], object] = {}
_n = itertools.count()


def _id(st: os.stat_result) -> tuple[int, int]:
    return (st.st_dev, st.st_ino)


def _open_shared(path: Path):
    """`path` opened with a shared lock, when its name still names it once locked; None when it
    isn't there, or was deleted meanwhile (look again)."""
    try:
        f = open(path, "rb")
    except FileNotFoundError:
        return None
    fcntl.flock(f.fileno(), fcntl.LOCK_SH)
    st = os.fstat(f.fileno())
    try:
        same = st.st_nlink > 0 and _id(os.stat(path)) == _id(st)
    except FileNotFoundError:
        same = False
    if not same:
        f.close()
        return None
    return f


def _keep(f, path: Path) -> None:
    _held[_id(os.fstat(f.fileno()))] = f
    try:
        os.utime(path)
    except OSError:
        pass


def release(path: Path) -> None:
    """Lets go of the file at `path`, when this process holds it."""
    try:
        f = _held.pop(_id(os.stat(path)), None)
    except FileNotFoundError:
        return
    if f is not None:
        f.close()


def hold_existing(path: Path) -> bool:
    """Holds the cache file at `path` (marked used) when it's there: whether it is."""
    for _ in range(100):
        f = _open_shared(path)
        if f is not None:
            _keep(f, path)
            return True
        if not path.exists():
            return False
    raise OSError(f"{path}: deleted each time it was opened")


def scratch(path: Path) -> tuple[Path, object]:
    """A temporary file beside cache file `path` (`<name>.<pid>.<n>.tmp`), made and locked
    (shared) at once, so room-making never takes it while it's written: its name and the open
    file. Written in place (opened again for writing, truncated: the same file), then `publish`ed."""
    for _ in range(100):
        # (A folder room-making takes as it's made, empty: made again.)
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
        except OSError:
            if not path.parent.is_dir():
                continue
        tmp = path.with_name(f"{path.name}.{os.getpid()}.{next(_n)}.tmp")
        try:
            fd = os.open(tmp, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o644)
        except OSError as e:
            # (Its folder taken meanwhile: gone, or, as it goes, EINVAL on macOS.)
            if e.errno in (errno.ENOENT, errno.EINVAL):
                continue
            raise
        f = os.fdopen(fd, "rb+")
        fcntl.flock(f.fileno(), fcntl.LOCK_SH)
        if os.fstat(f.fileno()).st_nlink == 0:
            f.close()
            continue
        return tmp, f
    raise OSError(f"{path}: no temporary file could be kept beside it")


def publish(f, tmp: Path, path: Path) -> None:
    """Gives scratch file `tmp` the name `path` if that's free (another process's copy made
    meanwhile wins), held by this process; the temporary name goes either way."""
    try:
        try:
            same = _id(os.stat(tmp)) == _id(os.fstat(f.fileno()))
        except FileNotFoundError:
            raise OSError(f"{tmp}: gone as it was written")
        if not same:
            # (A writer that put another file there: that one.)
            f.close()
            f = _open_shared(tmp)
            if f is None:
                raise OSError(f"{tmp}: gone as it was written")
        try:
            os.link(tmp, path)
        except FileExistsError:
            f.close()
            return
        _keep(f, path)
    finally:
        tmp.unlink(missing_ok=True)


def create(path: Path, fill: Callable[[Path], None]) -> None:
    """Makes the cache file at `path` when it isn't there: `fill` writes the scratch file's name
    in place, then it's named (`publish`) and held."""
    tmp, f = scratch(path)
    try:
        fill(tmp)
    except BaseException:
        f.close()
        tmp.unlink(missing_ok=True)
        raise
    publish(f, tmp, path)


def hold(path: Path, refill: Callable[[Path], None]) -> None:
    """Holds the cache file at `path`, made first by `refill` (into a scratch file, in place) when
    it isn't there."""
    for _ in range(100):
        if hold_existing(path):
            return
        create(path, refill)
    raise OSError(f"{path}: deleted each time it was opened")


def discard(path: Path) -> None:
    """Deletes a damaged cache file (cut short: no use to anyone), so it's taken again: with the
    exclusive lock when it can be had within seconds, else anyway; only while its name still names
    the file looked at. This process's own hold goes first."""
    release(path)
    try:
        f = open(path, "rb")
    except FileNotFoundError:
        return
    with f:
        t = time.time()
        while time.time() - t < 3:
            try:
                fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                time.sleep(0.05)
        try:
            if _id(os.lstat(path)) == _id(os.fstat(f.fileno())):
                path.unlink()
        except FileNotFoundError:
            pass


def try_remove(path: Path) -> bool:
    """Deletes the cache file at `path` unless a job holds it (an exclusive lock, not waited for;
    its name checked to still be that file): whether it did."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except OSError:
        return False
    with os.fdopen(fd, "rb") as f:
        try:
            fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            if _id(os.lstat(path)) != _id(os.fstat(f.fileno())):
                return False
            path.unlink()
            return True
        except OSError:
            return False
