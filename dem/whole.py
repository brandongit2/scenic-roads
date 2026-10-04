"""Files kept whole (docs/plan.md §3, Downloads; the Python steps' side of
crates/pipeline/src/whole.rs). What's written to the NAS's stores, or copied from them to a Mac's
cache, goes by a temporary name (this Mac's name and the process's, so two Macs never share one),
is flushed to the disk and has its length checked before the rename. And a kept TIFF can be checked
whole (its directories and strips or tiles inside the file), so a copy cut short is taken again
rather than read for good."""
from __future__ import annotations

import os
import shutil
import socket
import struct
from pathlib import Path


def tmp_name(path: Path) -> Path:
    """A temporary name beside `path`."""
    return path.with_name(f"{path.name}.{socket.gethostname()}.{os.getpid()}.tmp")


def sync(path: Path) -> None:
    """Flushes `path` to the disk (a NAS share: to the NAS)."""
    with path.open("rb+") as f:
        os.fsync(f.fileno())


def copy(src: Path, dst: Path) -> None:
    """Copies `src` to `dst`, whole."""
    tmp = tmp_name(dst)
    try:
        want = src.stat().st_size
        with src.open("rb") as r, tmp.open("wb") as w:
            shutil.copyfileobj(r, w, 16 << 20)
            w.flush()
            os.fsync(w.fileno())
        got = tmp.stat().st_size
        if got != want:
            raise OSError(f"copy {src} to {dst}: {got:,} of {want:,} bytes")
        tmp.rename(dst)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise


def tiff_whole(path: Path) -> bool:
    """Whether TIFF `path` is whole: each of its images' directories and strips or tiles inside the
    file (a file cut short loses its last ones, or the directory GDAL writes last)."""
    try:
        size = path.stat().st_size
        with path.open("rb") as f:

            def at(off: int, n: int) -> bytes:
                if off + n > size:
                    raise ValueError("past the end")
                f.seek(off)
                b = f.read(n)
                if len(b) != n:
                    raise ValueError("short read")
                return b

            h = at(0, 8)
            e = {b"II": "<", b"MM": ">"}.get(h[:2])
            if e is None:
                return False
            ver = struct.unpack(e + "H", h[2:4])[0]
            if ver == 42:
                big, ifd = False, struct.unpack(e + "I", h[4:8])[0]
            elif ver == 43:
                big, ifd = True, struct.unpack(e + "Q", at(8, 8))[0]
            else:
                return False
            cnt_fmt, cnt_sz, ent_sz, off_fmt, off_sz = ("Q", 8, 20, "Q", 8) if big else ("H", 2, 12, "I", 4)
            images = 0
            while ifd:
                images += 1
                if images > 64 or ifd >= size:
                    return False
                n = struct.unpack(e + cnt_fmt, at(ifd, cnt_sz))[0]
                ents = at(ifd + cnt_sz, n * ent_sz + off_sz)
                offs = lens = None
                for i in range(n):
                    ent = ents[i * ent_sz:(i + 1) * ent_sz]
                    tag, typ = struct.unpack(e + "HH", ent[:4])
                    if tag not in (273, 279, 324, 325):
                        continue
                    sz = {3: 2, 4: 4, 16: 8}.get(typ)
                    if sz is None:
                        return False
                    count = struct.unpack(e + off_fmt, ent[4:4 + off_sz])[0]
                    if count * sz <= off_sz:
                        raw = ent[ent_sz - off_sz:ent_sz - off_sz + count * sz]
                    else:
                        raw = at(struct.unpack(e + off_fmt, ent[ent_sz - off_sz:])[0], count * sz)
                    vals = struct.unpack(e + {2: "H", 4: "I", 8: "Q"}[sz] * count, raw)
                    if tag in (273, 324):
                        offs = vals
                    else:
                        lens = vals
                if offs is None or lens is None or len(offs) != len(lens):
                    return False
                if any(o + n_ > size for o, n_ in zip(offs, lens)):
                    return False
                ifd = struct.unpack(e + off_fmt, ents[n * ent_sz:])[0]
            return images > 0
    except (OSError, ValueError, struct.error):
        return False
