"""One-time repack of a depth source's loose grids on the NAS into packs (packs.py; task #452).

    uv run --project .. python repack.py <id> [<id> …] --work <local dir> [--adopt] [--sweep]

Run on the M4, from producers/depth. For each source, a z6 area at a time (the order its index
lists them in):

1. Its grids are read on the NAS by one `tar` over SSH (one sequential stream, no SMB opens), each
   depth file checked against its index's `hash`, and written into the area's pack here
   (`<work>/<id>/out/`).
2. Every `--batch-gb` of packs, they're tarred over SSH into the source's folder on the NAS
   (`.repack-incoming/` first, sizes compared there, then moved in: a pack is in its folder only
   whole), the batch's areas recorded done (`<work>/<id>/done.json`: each grid's pack and ranges),
   and deleted here. A stopped run resumes from there: the same command.
3. Once every area is done, the packs on the NAS are listed (one `stat` over SSH) and must all be
   there at their sizes; then the packed index is written (`<work>/<id>/<index>`). `--adopt` copies
   it into the drop box (`inputs/sources/`, as `_<index>.repack` then renamed: the build checks and
   takes it in like any edit). Until then the build reads the loose files, untouched.
4. `--sweep`, once the build has taken the packed index in (an accepted copy in
   `sources/inputs/sources/` with its bytes): the loose grids it packed are deleted on the NAS (one
   `xargs rm` over SSH). Never before.

The drop box's index is read once and kept (`<work>/<id>/index.orig.geojson`): a run whose drop
box index changed since stops (start again with a new --work).
`--box w,s,e,n` packs only the grids meeting a box, into `--dest` (a scratch folder on the NAS):
a trial; it never adopts or sweeps.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import tarfile
import time
import tomllib
from pathlib import Path

from packs import FILE_KEYS, PackWriter, by_area, check_hash, names, set_packed

NAS = "brandontsang@fishandchips.local"
NAS_ROOT = "/volume1/personal/projects/scenic-roads"
MOUNT = Path("/Volumes/personal/projects/scenic-roads")


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


class Remote:
    """Commands on the NAS over SSH (`host` "local": here, for tests)."""

    def __init__(self, host: str):
        self.host = host

    def argv(self, cmd: str) -> list[str]:
        if self.host == "local":
            return ["sh", "-c", cmd]
        return ["ssh", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=30", self.host, cmd]

    def run(self, cmd: str, stdin: bytes | None = None) -> bytes:
        return subprocess.run(self.argv(cmd), input=stdin, capture_output=True, check=True).stdout

    def sizes(self, folder: str, files: list[str]) -> dict[str, int]:
        """The sizes of `files` in `folder` (those there)."""
        if not files:
            return {}
        out = self.run(f"cd {shlex.quote(folder)} 2>/dev/null && xargs -0 ls -ln -- 2>/dev/null; true", "\0".join(files).encode())
        got = {}
        for l in out.decode().splitlines():
            f = l.split()
            if len(f) >= 9 and l.startswith("-"):
                got[f[-1]] = int(f[4])
        return got


def read_area(rem: Remote, src: str, feats: list[dict], into: PackWriter):
    """An area's grids read from the NAS by one tar stream into a pack, in order, each depth file
    checked against its hash."""
    want = [n for f in feats for n in names(f)]
    depth = {f["properties"]["file"]: f for f in feats}
    p = subprocess.Popen(rem.argv(f"cd {shlex.quote(src)} && tar --null -cf - -T -"), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         env={**os.environ, "COPYFILE_DISABLE": "1"})
    p.stdin.write("\0".join(want).encode())
    p.stdin.close()
    got = []
    with tarfile.open(fileobj=p.stdout, mode="r|") as t:
        for m in t:
            if not m.isfile():
                continue
            data = t.extractfile(m).read()
            if m.name in depth:
                check_hash(depth[m.name], data)
            into.add(m.name, data)
            got.append(m.name)
    p.stdout.close()
    if p.wait() != 0 or got != want:
        missing = sorted(set(want) - set(got))
        raise SystemExit(f"{src}: the tar of {len(want)} files gave {len(got)}{'; missing ' + ', '.join(missing[:5]) if missing else ''}")


def upload(rem: Remote, local: Path, dest: str, packs: list[str]):
    """`packs` (in `local`) into `dest` on the NAS: tarred into `.repack-incoming/`, sizes compared,
    then moved in."""
    inc = f"{dest}/.repack-incoming"
    tar = subprocess.Popen(["tar", "--no-xattrs", "-cf", "-", "-C", str(local), *packs], stdout=subprocess.PIPE,
                           env={"COPYFILE_DISABLE": "1", "PATH": "/usr/bin:/bin"})
    subprocess.run(rem.argv(f"mkdir -p {shlex.quote(inc)} && cd {shlex.quote(inc)} && tar xf -"), stdin=tar.stdout, check=True)
    tar.stdout.close()
    if tar.wait() != 0:
        raise SystemExit("the local tar of the packs failed")
    there = rem.sizes(inc, packs)
    bad = [p for p in packs if there.get(p) != (local / p).stat().st_size]
    if bad:
        raise SystemExit(f"on the NAS not as here: {bad[:5]}")
    rem.run(f"cd {shlex.quote(inc)} && xargs -0 -I@ mv -f @ {shlex.quote(dest)}/", "\0".join(packs).encode())


def index_name(declarations: Path, sid: str) -> str:
    d = tomllib.loads((declarations / f"{sid}.toml").read_text())
    return d["data"]["index"]


def write_json(path: Path, v):
    tmp = path.with_name(path.name + ".tmp")
    with open(tmp, "w") as f:
        json.dump(v, f, separators=(",", ":"))
        f.write("\n")
    os.replace(tmp, path)


def meets(f: dict, box: list[float]) -> bool:
    xs, ys = [], []

    def walk(v):
        if isinstance(v, list) and len(v) >= 2 and isinstance(v[0], (int, float)):
            xs.append(v[0])
            ys.append(v[1])
        elif isinstance(v, list):
            for w in v:
                walk(w)

    walk(f["geometry"]["coordinates"])
    return min(xs) <= box[2] and max(xs) >= box[0] and min(ys) <= box[3] and max(ys) >= box[1]


def repack(a, sid: str, rem: Remote) -> Path | None:
    """Source `sid` packed (resumably); its packed index's path once every pack is on the NAS."""
    work = a.work / sid
    out = work / "out"
    out.mkdir(parents=True, exist_ok=True)
    name = index_name(a.drop, sid)
    orig = work / "index.orig.geojson"
    now = (a.drop / name).read_bytes()
    if not orig.exists():
        orig.write_bytes(now)
    elif orig.read_bytes() != now and not ((work / name).exists() and (work / name).read_bytes() == now):
        # (Adopted already: the drop box holds this run's packed index.)
        raise SystemExit(f"{sid}: {a.drop / name} changed since this run began: start again with a new --work")
    index = json.loads(orig.read_text())
    feats = index["features"]
    if a.box:
        feats = [f for f in feats if meets(f, a.box)]
    src = f"{a.nas_root}/sources/fetched/{sid}"
    dest = f"{a.dest}/{sid}" if a.dest else src
    done_path = work / "done.json"
    done = json.loads(done_path.read_text()) if done_path.exists() else {}  # area → {file: [pack, at]}
    areas = by_area(feats)
    todo = [k for k in areas if k not in done]
    nfiles = sum(len(names(f)) for k in todo for f in areas[k])
    log(f"{sid}: {len(feats)} grids in {len(areas)} areas; {len(todo)} areas ({nfiles} files) to pack")
    batch, batch_bytes, t0, read_bytes, read_files = [], 0, time.time(), 0, 0

    def flush():
        nonlocal batch, batch_bytes
        if not batch:
            return
        upload(rem, out, dest, [p for _, p, _ in batch])
        for k, p, at in batch:
            done[k] = {f["properties"]["file"]: [p, {kk: list(at[f["properties"][kk]]) for kk in FILE_KEYS if kk in f["properties"]}] for f in areas[k]}
        write_json(done_path, done)
        for _, p, _ in batch:
            (out / p).unlink()
        log(f"{sid}: {len(done)}/{len(areas)} areas on the NAS")
        batch, batch_bytes = [], 0

    for k in todo:
        w = PackWriter(out, k)
        read_area(rem, src, areas[k], w)
        p, at = w.finish()
        size = (out / p).stat().st_size
        read_bytes += size
        read_files += len(at)
        batch.append((k, p, at))
        batch_bytes += size
        if batch_bytes >= a.batch_gb * 1e9:
            flush()
        if a.pause:
            time.sleep(a.pause)
    flush()
    if todo:
        dt = time.time() - t0
        log(f"{sid}: read {read_files} files, {read_bytes / 1e6:.1f} MB in {dt:.1f} s ({read_files / max(dt, 1e-9):.1f} files/s, {read_bytes / 1e6 / max(dt, 1e-9):.2f} MB/s)")
    # Every pack on the NAS whole, then the packed index.
    packs = sorted({v[0] for area in done.values() for v in area.values()})
    sizes = rem.sizes(dest, packs)
    missing = [p for p in packs if p not in sizes]
    if missing:
        raise SystemExit(f"{sid}: {len(missing)} packs not on the NAS ({missing[:3]}): run again")
    byfile = {f: v for area in done.values() for f, v in area.items()}
    for f in feats:
        v = byfile.get(f["properties"]["file"])
        if v is None and "pack" not in f["properties"]:
            raise SystemExit(f"{sid}: {f['properties']['file']} not packed: run again")
        if v is not None:
            pr = f["properties"]
            pr["pack"], pr["at"] = v[0], v[1]
    packed = work / name
    write_json(packed, {**index, "features": feats})
    log(f"{sid}: every pack on the NAS ({len(packs)}, {sum(sizes.values()) / 1e9:.2f} GB); packed index {packed}")
    return packed


def adopt(a, sid: str, packed: Path):
    """The packed index into the drop box, whole (written under an ignored name, then renamed)."""
    name = index_name(a.drop, sid)
    now = (a.drop / name).read_bytes()
    if now == packed.read_bytes():
        return
    if now != (a.work / sid / "index.orig.geojson").read_bytes():
        raise SystemExit(f"{sid}: the drop box's {name} changed since this run began: not adopted")
    tmp = a.drop / f"_{name}.repack"
    tmp.write_bytes(packed.read_bytes())
    os.replace(tmp, a.drop / name)
    log(f"{sid}: {a.drop / name} is the packed index now (the build checks and takes it in)")


def taken_in(a, sid: str, packed: Path) -> bool:
    """Whether the build has an accepted copy of the packed index (sources/inputs/sources/<stem>.*)."""
    name = index_name(a.drop, sid)
    stem = name.removesuffix(".geojson")
    want = packed.read_bytes()
    acc = a.accepted
    return any(p.stat().st_size == len(want) and p.read_bytes() == want for p in acc.glob(f"{stem}.*.geojson"))


def sweep(a, sid: str, packed: Path, rem: Remote):
    """The loose grids the packed index no longer names deleted on the NAS, once it's taken in."""
    if not taken_in(a, sid, packed):
        raise SystemExit(f"{sid}: the build hasn't taken the packed index in yet (no accepted copy in {a.accepted}): not swept")
    feats = json.loads(packed.read_text())["features"]
    if any("pack" not in f["properties"] for f in feats):
        raise SystemExit(f"{sid}: some grids aren't packed: not swept")
    files = sorted(n for f in feats for n in names(f))
    src = f"{a.nas_root}/sources/fetched/{sid}"
    rem.run(f"cd {shlex.quote(src)} && xargs -0 rm -f --", "\0".join(files).encode())
    log(f"{sid}: {len(files)} loose files deleted on the NAS")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ids", nargs="+")
    ap.add_argument("--work", required=True, type=Path)
    ap.add_argument("--nas", default=NAS, help='"local": the commands run here (tests)')
    ap.add_argument("--nas-root", default=NAS_ROOT)
    ap.add_argument("--drop", type=Path, default=MOUNT / "inputs/sources", help="the drop box (indexes, declarations)")
    ap.add_argument("--accepted", type=Path, default=MOUNT / "sources/inputs/sources", help="the build's accepted copies")
    ap.add_argument("--dest", help="a trial: the packs go to <dest>/<id> on the NAS (a scratch folder), not the source folder")
    ap.add_argument("--box", help="w,s,e,n: only the grids meeting it (a trial)")
    ap.add_argument("--batch-gb", type=float, default=2.0)
    ap.add_argument("--pause", type=float, default=0.0, help="seconds between areas (to spare the NAS)")
    ap.add_argument("--adopt", action="store_true")
    ap.add_argument("--sweep", action="store_true")
    a = ap.parse_args(argv)
    a.box = [float(v) for v in a.box.split(",")] if a.box else None
    if (a.box or a.dest) and (a.adopt or a.sweep):
        raise SystemExit("a trial (--box, --dest) never adopts or sweeps")
    rem = Remote(a.nas)
    for sid in a.ids:
        packed = repack(a, sid, rem)
        if a.adopt:
            adopt(a, sid, packed)
        if a.sweep:
            sweep(a, sid, packed, rem)


if __name__ == "__main__":
    main(sys.argv[1:])
