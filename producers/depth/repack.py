"""One-time repack of a depth source's loose grids on the NAS into packs (packs.py; task #452).

    uv run --project .. python repack.py <id> [<id> …] --work <local dir> [--adopt] [--sweep]

Run on the M4, from producers/depth. For each source, a z6 tile at a time (the order its index
lists them in):

1. Its grids are read on the NAS by one `tar` over SSH (one sequential stream, no SMB opens), each
   depth file checked against its index's `hash` (the low-water files have none there),
   and written into the z6 tile's pack here (`<work>/<id>/out/`), every member's hash recorded in the
   packed index (`at`) and checked again from the pack written.
2. Every `--batch-gb` of packs, they're tarred over SSH into the source's folder on the NAS
   (`.repack-incoming/` first, sizes compared there, then moved in: a pack is in its folder only
   whole), the batch's z6 tiles recorded done (`<work>/<id>/done.json`, its `areas` key kept from when a z6 tile was called an area: each z6 tile's pack, its size,
   and its grids' ranges), and deleted here. A stopped run resumes from there: the same command.
   done.json names the run's folder on the NAS and box: a --work used for a trial refuses a real
   run (and the reverse).
3. Once every z6 tile is done, the packs on the NAS are listed (one `ls` over SSH) and must all be
   there at their recorded sizes; then the packed index is written (`<work>/<id>/<index>`). `--adopt` copies
   it into the inputs folder (`inputs/sources/`, as `_<index>.repack` then renamed: the build checks and
   takes it in like any edit). Until then the build reads the loose files, untouched.
4. `--sweep`, once the build reads the packed index: the inputs folder holds exactly its bytes, and the
   build's current accepted version of the unit (`state/status.json`'s `inputs` entry for
   `sources`, its `@index`) names an accepted copy with exactly its bytes; and safely past the
   switch: the lead's status fresh (its beat and its file's time within `--stale-min`, 10 min);
   the accepted versions naming that same copy of the index for `--min-age-h` (24 h) at least,
   from when a sweep first saw it (`done.json`'s `taken_in_seen`, with the copy seen: another copy
   seen, as after a rollback or a new pack of the source, starts the wait again; a new acceptance
   of the unit for other files doesn't); and no depth or depth-lo job running on any Mac (the
   lead's in status.json, each other Mac's in its own helper file `state/helpers/<host>.json`
   however stale its beat (the lead's own left from before it led passed over; a stale one's job
   named with how to clear it), a file that doesn't read refusing; the old fields `job`, `beside`, `more` too),
   none its waiting list names, and none held to resume in the members' progress files
   (`state/pool/progress/<member>.json`). A job reads the records once, when it starts
   (`Out::open`), and its grids by the paths they gave it all through its run; a running one
   refuses whenever it started: the accepted copies are content-named, so a rollback and a return
   to the same packed copy between two sweeps leave no trace here, and a job started on the loose
   index in between would look like one started after the switch. Jobs planned or leased but not
   started read the records when they start.
   Then the packs are checked on the NAS at their sizes once more and the build checked to
   read the packed index still, and the loose grids it packed are deleted (one `xargs rm` over
   SSH).
   The sweep is the point of no return: after it the loose index can't be rolled back to (its
   files are gone); until it, putting the loose index back in the inputs folder is a full rollback.

The inputs folder's index is read once and kept (`<work>/<id>/index.orig.geojson`): a run whose inputs-folder index changed since stops (start again with a new --work).
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

from packs import FILE_KEYS, PackWriter, by_tile, check_hash, names

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
        return ["ssh", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=30",
                    # AES-GCM first (2.4x the default's rate from the NAS); else the next in the list
                    "-c", "aes128-gcm@openssh.com,chacha20-poly1305@openssh.com,aes128-ctr", self.host, cmd]

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


def read_tile(rem: Remote, src: str, feats: list[dict], into: PackWriter):
    """A z6 tile's grids read from the NAS by one tar stream into a pack, in order, each depth file
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
    name = index_name(a.inputs, sid)
    orig = work / "index.orig.geojson"
    now = (a.inputs / name).read_bytes()
    if not orig.exists():
        orig.write_bytes(now)
    elif orig.read_bytes() != now and not ((work / name).exists() and (work / name).read_bytes() == now):
        # (Adopted already: the inputs folder holds this run's packed index.)
        raise SystemExit(f"{sid}: {a.inputs / name} changed since this run began: start again with a new --work")
    index = json.loads(orig.read_text())
    feats = index["features"]
    if a.box:
        feats = [f for f in feats if meets(f, a.box)]
    src = f"{a.nas_root}/sources/fetched/{sid}"
    dest = f"{a.dest}/{sid}" if a.dest else src
    done_path = work / "done.json"
    run = {"dest": dest, "box": a.box}
    saved = json.loads(done_path.read_text()) if done_path.exists() else {"run": run, "areas": {}}
    if saved.get("run") != run:
        raise SystemExit(f"{sid}: {done_path} is a run's into {saved.get('run')}, not {run}: use another --work")
    done = saved["areas"]  # z6 tile → {"pack", "size", "files": {file: at}}
    tiles = by_tile(feats)
    todo = [k for k in tiles if k not in done]
    nfiles = sum(len(names(f)) for k in todo for f in tiles[k])
    log(f"{sid}: {len(feats)} grids in {len(tiles)} z6 tiles; {len(todo)} z6 tiles ({nfiles} files) to pack")
    batch, batch_bytes, t0, read_bytes, read_files = [], 0, time.time(), 0, 0

    def flush():
        nonlocal batch, batch_bytes
        if not batch:
            return
        upload(rem, out, dest, [p for _, p, _ in batch])
        for k, p, at in batch:
            files = {f["properties"]["file"]: {kk: list(at[f["properties"][kk]]) for kk in FILE_KEYS if kk in f["properties"]} for f in tiles[k]}
            done[k] = {"pack": p, "size": (out / p).stat().st_size, "files": files}
        write_json(done_path, {**saved, "run": run, "areas": done})
        for _, p, _ in batch:
            (out / p).unlink()
        log(f"{sid}: {len(done)}/{len(tiles)} z6 tiles on the NAS")
        batch, batch_bytes = [], 0

    for k in todo:
        w = PackWriter(out, k)
        read_tile(rem, src, tiles[k], w)
        p, at = w.finish()
        verify(out / p, at)
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
    want = {d["pack"]: d["size"] for d in done.values()}
    packs = sorted(want)
    sizes = rem.sizes(dest, packs)
    bad = [p for p in packs if sizes.get(p) != want[p]]
    if bad:
        raise SystemExit(f"{sid}: {len(bad)} packs not on the NAS at their sizes ({bad[:3]}): run again")
    byfile = {f: (d["pack"], at) for d in done.values() for f, at in d["files"].items()}
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
    """The packed index into the inputs folder, whole (written under an ignored name, then renamed)."""
    name = index_name(a.inputs, sid)
    now = (a.inputs / name).read_bytes()
    if now == packed.read_bytes():
        return
    if now != (a.work / sid / "index.orig.geojson").read_bytes():
        raise SystemExit(f"{sid}: the inputs folder's {name} changed since this run began: not adopted")
    tmp = a.inputs / f"_{name}.repack"
    tmp.write_bytes(packed.read_bytes())
    os.replace(tmp, a.inputs / name)
    log(f"{sid}: {a.inputs / name} is the packed index now (the build checks and takes it in)")


def verify(pack: Path, at: dict[str, tuple[int, int, str]]):
    """Every member of a pack written read back by its range against its recorded hash."""
    with open(pack, "rb") as f:
        for n, (off, ln, h) in at.items():
            f.seek(off)
            if hashlib.sha256(f.read(ln)).hexdigest()[:16] != h:
                raise SystemExit(f"{pack}: {n} isn't as read")


def current_version(a, name: str):
    """The build's current accepted version of the sources unit and its copy of index `name`:
    (the @index named in state/status.json's `inputs`, the copy's path), None for what's missing."""
    try:
        status = json.loads((a.root / "state/status.json").read_text())
    except (OSError, ValueError):
        return None, None
    ver = [u.get("version") for u in status.get("inputs") or [] if u.get("unit") == "sources"]
    if len(ver) != 1 or not ver[0]:
        return None, None
    try:
        copy = json.loads((a.root / ver[0]).read_text())["files"][name]["file"]
    except (OSError, ValueError, KeyError, TypeError):
        return ver[0], None
    return ver[0], copy


def taken_in(a, sid: str, packed: Path) -> bool:
    """Whether the build reads the packed index now: the inputs folder holds its bytes, and the unit's
    current accepted version (state/status.json's `sources` entry, its @index) names a copy with
    its bytes."""
    name = index_name(a.inputs, sid)
    want = packed.read_bytes()
    if (a.inputs / name).read_bytes() != want:
        return False
    _, copy = current_version(a, name)
    return copy is not None and (a.root / copy).read_bytes() == want


DEPTH_STEPS = ("depth", "depth-lo")
# A helper file beaten longer ago than this (the lead's status leaves the helper out, agent::helpers).
STALE_HELPER_S = 600


class Unsure(Exception):
    """What the sweep can't tell (why it refuses)."""


def depth_jobs(a) -> list[str]:
    """The depth and depth-lo jobs running on any Mac (their jobs, and an older agent's `job`,
    `beside` and `more`): the lead's as its status says, and each other Mac's as its own helper
    file (state/helpers/<host>.json) says, whatever its beat's age (the lead's status leaves out a
    helper beaten over ten minutes ago, crate::agent::helpers, whose job may still run: a Mac
    asleep or off the NAS carries on when it's back), whenever they started (see the module's
    docstring, step 4); the depth steps the lead's status says wait; and the depth jobs held to
    resume in the members' progress files (state/pool/progress/<member>.json,
    crate::agent::resume). The lead's own helper file, left from before it led, is passed over: its
    status says its Mac's jobs. A job from a helper file names the file and its beat's age, and one
    beaten over STALE_HELPER_S ago says how to clear it (nothing removes a helper file: a Mac gone
    for good leaves its last jobs there). A helper file that doesn't read raises Unsure."""
    status = json.loads((a.root / "state/status.json").read_text())
    found = []
    now = time.time()

    def job(v, src=""):
        if isinstance(v, dict) and isinstance(v.get("id"), str) and v["id"].split(" ", 1)[0] in DEPTH_STEPS:
            found.append(v["id"] + src)

    def jobs_of(who, src=""):
        for j in [*(who.get("jobs") or []), who.get("job"), who.get("beside"), *(who.get("more") or [])]:
            job(j, src)

    for who in [status, *(status.get("helpers") or [])]:
        jobs_of(who)
    hdir = a.root / "state/helpers"
    for p in sorted(hdir.glob("*.json")) if hdir.is_dir() else []:
        if p.stem == status.get("host"):
            continue
        rel = f"state/helpers/{p.name}"
        try:
            h = json.loads(p.read_text())
            if not isinstance(h, dict):
                raise ValueError("not an object")
        except (OSError, ValueError) as e:
            raise Unsure(f"helper file {rel} doesn't read ({e}): its depth jobs unknown; if its Mac's agent "
                         "no longer runs (retired, renamed), remove the file, else wait for its next beat")
        beat = h.get("beat")
        age = now - beat if isinstance(beat, (int, float)) else None
        said = "no beat" if age is None else f"beat {age / 3600:.1f} h ago"
        hint = ("" if age is not None and age <= STALE_HELPER_S else
                "; if that Mac's agent isn't running it (gone for good, or the job ended), remove the file")
        jobs_of(h, f" ({rel}, {said}{hint})")
    for w in status.get("waiting") or []:
        if isinstance(w, dict) and w.get("step") in DEPTH_STEPS:
            found.append(f"waiting: {w.get('what')}")
    pool = status.get("pool") or {}
    for m in pool.get("members") or []:
        p = a.root / f"state/pool/progress/{m}.json"
        try:
            held = json.loads(p.read_text()).get("jobs") or {}
        except FileNotFoundError:
            continue
        for k, h in held.items():
            if k.split(" ", 1)[0] in DEPTH_STEPS and not h.get("done"):
                found.append(f"held to resume on {m}: {k}")
    return found


def safe_to_sweep(a, sid: str, packed: Path, done_path: Path) -> str | None:
    """Why the loose grids may not go yet (None: they may). The wait (`--min-age-h`) runs from when
    a sweep first saw the build's current version name the packed index, kept in done.json with
    the copy of the index it names; another copy seen (a rollback, the source packed anew) starts it
    again, another version naming the same copy doesn't."""
    saved = json.loads(done_path.read_text())
    name = index_name(a.inputs, sid)
    ver, copy = current_version(a, name)
    if not taken_in(a, sid, packed):
        if saved.pop("taken_in_seen", None) is not None:
            write_json(done_path, saved)
        if ver is None:
            return "the lead's status names no accepted version of the sources unit"
        return "the build doesn't read the packed index (the inputs folder's, or its current accepted version's, isn't it)"
    st = a.root / "state/status.json"
    status = json.loads(st.read_text())
    now = time.time()
    beat = float(status.get("beat") or 0)
    age = now - min(beat, st.stat().st_mtime)
    if age > a.stale_min * 60:
        return f"the lead's status is stale ({age:.0f} s old): is the build running?"
    seen = saved.get("taken_in_seen")
    if not isinstance(seen, dict) or seen.get("copy") != copy:
        seen = saved["taken_in_seen"] = {"copy": copy, "at": now}
        write_json(done_path, saved)
    if now - seen["at"] < a.min_age_h * 3600:
        return f"the packed index taken in only {(now - seen['at']) / 3600:.1f} h ago (wait {a.min_age_h:g} h, a margin to roll back in)"
    try:
        jobs = depth_jobs(a)
    except Unsure as e:
        return str(e)
    if jobs:
        return f"depth jobs running, waiting or held ({', '.join(jobs[:5])}): any may read the loose index"
    return None


def sweep(a, sid: str, packed: Path, rem: Remote):
    """The loose grids the packed index no longer names deleted on the NAS, once it's safely taken
    in (`safe_to_sweep`): the point of no return."""
    why = safe_to_sweep(a, sid, packed, a.work / sid / "done.json")
    if why:
        raise SystemExit(f"{sid}: not swept: {why}")
    feats = json.loads(packed.read_text())["features"]
    if any("pack" not in f["properties"] for f in feats):
        raise SystemExit(f"{sid}: some grids aren't packed: not swept")
    files = sorted(n for f in feats for n in names(f))
    src = f"{a.nas_root}/sources/fetched/{sid}"
    # (The packs there at their sizes once more, and the build still on the packed index, just
    # before.)
    want = {d["pack"]: d["size"] for d in json.loads((a.work / sid / "done.json").read_text())["areas"].values()}
    there = rem.sizes(src, sorted(want))
    bad = [p for p in want if there.get(p) != want[p]]
    if bad:
        raise SystemExit(f"{sid}: not swept: {len(bad)} packs not on the NAS at their sizes ({bad[:3]})")
    if not taken_in(a, sid, packed):
        raise SystemExit(f"{sid}: not swept: the build no longer reads the packed index")
    rem.run(f"cd {shlex.quote(src)} && xargs -0 rm -f --", "\0".join(files).encode())
    log(f"{sid}: {len(files)} loose files deleted on the NAS")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ids", nargs="+")
    ap.add_argument("--work", required=True, type=Path)
    ap.add_argument("--nas", default=NAS, help='"local": the commands run here (tests)')
    ap.add_argument("--nas-root", default=NAS_ROOT)
    ap.add_argument("--inputs", type=Path, default=MOUNT / "inputs/sources", help="the inputs folder (indexes, declarations)")
    ap.add_argument("--root", type=Path, default=MOUNT, help="the build's root (state/status.json, the accepted copies)")
    ap.add_argument("--dest", help="a trial: the packs go to <dest>/<id> on the NAS (a scratch folder), not the source folder")
    ap.add_argument("--box", help="w,s,e,n: only the grids meeting it (a trial)")
    ap.add_argument("--batch-gb", type=float, default=2.0)
    ap.add_argument("--pause", type=float, default=0.0, help="seconds between z6 tiles (to spare the NAS)")
    ap.add_argument("--min-age-h", type=float, default=24.0, help="--sweep: hours the packed index must have been taken in")
    ap.add_argument("--stale-min", type=float, default=10.0, help="--sweep: the lead's status beat at most this old")
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
