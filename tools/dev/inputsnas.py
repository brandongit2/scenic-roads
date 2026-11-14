#!/usr/bin/env python3
"""The staged declarations in the repository against the NAS's inputs folders (docs/inputs.md §8).

Declarations and notes are made and reviewed in the repository (producers/) and copied to the NAS
by the PM after a merge; nothing else compares the two. This does, read-only, and places them.

  inputsnas.py drift [--root DIR]
      every staged file's content hash (here) against its NAS file's (on the NAS, one SSH pass:
      `find` and `sha256sum` there, no per-file round trips). One line when all match; otherwise a
      line naming what differs, then a line a file: `differs`, `not placed` (staged, not on the NAS)
      or `only on the NAS` (in a folder the repository holds whole). Exit 0 as staged, 1 drift, 2
      not read. --root reads a local folder standing in for the NAS project folder (or the mount).
  inputsnas.py place [--root DIR] [--dry-run] [<NAS path>…]
      places what drifted (or the NAS paths named: `inputs/extents/terrain`): each staged afresh
      under inputs/.place/<time>/new/ (the repository's files by tar over SSH; a shared folder's own
      files copied there on the NAS), every file verified against the repository, then swapped in,
      the old one kept under inputs/.place/<time>/kept/; then asks for an input check of each unit
      (`scenic inputs check <unit>`; not with --root) and says what is left to do. Run only from the
      main checkout at origin/main with producers/ committed; deletes nothing.

What is staged where (MAPS, and the notes' boxes, derived): a folder the repository holds `whole`
(its NAS folder is only what is staged: a file only there is drift) or `shared` with what the owner
declares on the NAS alone (the NAS's own files are counted, not drift). A shared folder's staged
files are its `.toml` declarations and the indexes they name (`index = "…"`) beside them; the
other files there are producers' own (code, their inputs). Not drift, said in the one line:
- a declaration whose index is made neither here nor on the NAS: it waits for its index (the
  input check would refuse it, `src-missing`);
- a file in a staged folder's `held/`: staged, not yet meant for the NAS (moving it out of held/,
  reviewed, is its release; a held file found on the NAS is drift);
- a mapped folder whose NAS folder doesn't exist: not placed, its unit not made yet.
Every `.toml` under producers/ (but pyproject.toml) must be in a mapped folder: one that isn't is
`unmapped`, drift, so a new producer's declarations can't go unseen.
"""
import argparse
import datetime
import hashlib
import io
import os
import re
import shlex
import subprocess
import sys
import tarfile

# (the repository's folder, the NAS's under the project folder, whose the NAS folder is)
WHOLE, SHARED, NOTES, FILE = "whole", "shared", "notes", "file"
MAPS = [
    ("producers/extents/terrain", "inputs/extents/terrain", WHOLE),
    ("producers/sources", "inputs/sources", SHARED),
    ("producers/depth/declarations", "inputs/sources", SHARED),
    ("producers/geoid/declarations", "inputs/sources", SHARED),
    ("producers/elevation", "inputs/elevation", SHARED),
    ("producers/elevation/1m", "inputs/elevation", SHARED),
    ("producers/how/inputs-README.md", "inputs/README.md", FILE),
]
# (Each folder under producers/how/ holding a README.md is a box, copied into inputs/<box>/how/,
# beside what is there: producers/how/README.md says so, and is the staging's own.)
HOW = "producers/how"
HELD = "held"
NAS_ROOT = "/volume1/personal/projects/scenic-roads"
PLACE = "inputs/.place"
# (Names an inputs folder ignores, docs/inputs.md §3.)
IGNORED = (".", "@", "#", "_")
SAFE = re.compile(r"^[A-Za-z0-9._/+-]+$")
INDEX = re.compile(r'^index\s*=\s*"([^"]+)"', re.M)


def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


class Folder:
    """A NAS path and what the repository stages there: name (relative to it) -> repository path."""

    def __init__(self, nas, mode):
        self.nas, self.mode = nas, mode
        self.staged = {}
        self.held = {}
        self.indexes = {}  # a staged declaration's name -> the index it names
        self.repo_dirs = []


def folders(repo):
    """The NAS paths staged to, from MAPS and the notes' boxes."""
    out = {}

    def get(nas, mode):
        f = out.setdefault(nas, Folder(nas, mode))
        if f.mode != mode:
            raise SystemExit(f"inputsnas: {nas} mapped both {f.mode} and {mode}")
        return f

    def add(f, name, path, held=False):
        into = f.held if held else f.staged
        if name in f.staged or name in f.held:
            raise SystemExit(f"inputsnas: {f.nas}/{name} staged twice ({into.get(name) or f.staged.get(name)}, {path})")
        into[name] = path

    for rdir, nas, mode in MAPS:
        full = os.path.join(repo, rdir)
        if mode == FILE:
            if os.path.isfile(full):
                add(get(nas, FILE), os.path.basename(nas), rdir)
            continue
        f = get(nas, mode)
        f.repo_dirs.append(rdir)
        for sub, held in (("", False), (HELD, True)):
            d = os.path.join(full, sub)
            if not os.path.isdir(d):
                continue
            names = sorted(n for n in os.listdir(d) if not n.startswith(".") and os.path.isfile(os.path.join(d, n)))
            if mode == WHOLE:
                take = names
            else:
                decls = [n for n in names if n.endswith(".toml")]
                take = list(decls)
                for n in decls:
                    with open(os.path.join(d, n), encoding="utf-8") as fh:
                        m = INDEX.search(fh.read())
                    if m:
                        if not held:
                            f.indexes[n] = m.group(1)
                        if m.group(1) in names:
                            take.append(m.group(1))
            for n in sorted(set(take)):
                add(f, n, os.path.join(rdir, sub, n) if sub else os.path.join(rdir, n), held)
    how = os.path.join(repo, HOW)
    boxes = sorted(
        os.path.relpath(d, how) for d, _, fs in os.walk(how) if d != how and "README.md" in fs
    )
    for box in boxes:
        f = get(f"inputs/{box}/how", NOTES)
        f.repo_dirs.append(f"{HOW}/{box}")
        top = os.path.join(how, box)
        for d, ds, fs in os.walk(top):
            rel = os.path.relpath(d, top)
            ds[:] = sorted(x for x in ds if os.path.normpath(os.path.join(box, rel, x)) not in boxes)
            for n in sorted(fs):
                if not n.startswith("."):
                    name = n if rel == "." else f"{rel}/{n}"
                    add(f, name, f"{HOW}/{box}/{name}")
    return [out[k] for k in sorted(out)]


def unmapped(repo, fs):
    """Declarations under producers/ that no mapped folder stages."""
    covered = {p for f in fs for p in list(f.staged.values()) + list(f.held.values())}
    out = []
    for d, ds, names in os.walk(os.path.join(repo, "producers")):
        ds[:] = [x for x in ds if not x.startswith(".") and x not in ("node_modules", "__pycache__")]
        for n in names:
            p = os.path.relpath(os.path.join(d, n), repo)
            if n.endswith(".toml") and n != "pyproject.toml" and p not in covered:
                out.append(p)
    return sorted(out)


def nas_host():
    """The NAS by its LAN name at home, else through Tailscale by its bare name (as health.sh)."""
    r = subprocess.run(["dscacheutil", "-q", "host", "-a", "name", "fishandchips.local"], capture_output=True, text=True)
    return "fishandchips.local" if "ip_address" in r.stdout else "fishandchips"


def run_there(script, root, stdin=None):
    """A POSIX sh script in the NAS project folder (over SSH), or in a local stand-in (--root)."""
    if root:
        cmd = ["sh", "-c", f"SHA='shasum -a 256'\ncd {shlex.quote(root)} || exit 3\n{script}"]
    else:
        cmd = [
            "ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3", "-o", "HostKeyAlias=fishandchips.local", "-o", "LogLevel=ERROR",
            f"brandontsang@{nas_host()}", f"SHA=sha256sum\ncd {NAS_ROOT} || exit 3\n{script}",
        ]
    return subprocess.run(cmd, input=stdin, capture_output=True)


def q(p):
    if not SAFE.match(p) or ".." in p.split("/"):
        raise SystemExit(f"inputsnas: refusing an unusual path: {p!r}")
    return shlex.quote(p)


def hash_script(dirs, files):
    """Lists each folder (its files, one level) and hashes the files named: `dir <d>` for a folder
    there, `<sha256>  <path>` a file, `end` once done."""
    ign = " ".join(f"! -name {shlex.quote(c + '*')}" for c in IGNORED)
    s = []
    for d in dirs:
        s.append(f"if [ -d {q(d)} ]; then echo dir {q(d)}; find {q(d)} -mindepth 1 -maxdepth 1 -type f {ign} -exec $SHA {{}} + || exit 4; fi")
    for f in files:
        s.append(f"if [ -f {q(f)} ]; then $SHA {q(f)} || exit 4; fi")
    s.append("echo end")
    return "\n".join(s)


def parse_hashes(out):
    dirs, files, done = set(), {}, False
    for line in out.decode("utf-8", "replace").splitlines():
        if line == "end":
            done = True
        elif line.startswith("dir "):
            dirs.add(line[4:])
        elif re.match(r"^[0-9a-f]{64}  ", line):
            p = line[66:]
            files[p[2:] if p.startswith("./") else p] = line[:64]
    return dirs, files, done


def read_nas(fs, root):
    """What the NAS holds of each folder: (folders there, {path: sha256})."""
    dirs = sorted({f.nas for f in fs if f.mode in (WHOLE, SHARED)} | {os.path.dirname(f.nas) for f in fs if f.mode == FILE})
    files = sorted(f"{f.nas}/{n}" for f in fs if f.mode == NOTES for n in list(f.staged) + list(f.held))
    r = run_there(hash_script(dirs, files), root)
    there, hashes, done = parse_hashes(r.stdout)
    if r.returncode != 0 or not done:
        raise OSError(f"exit {r.returncode}: {r.stderr.decode('utf-8', 'replace').strip()[:300]}")
    for f in fs:  # (a notes box's folder is there when any of its files is)
        if f.mode == NOTES and any(p.startswith(f.nas + "/") for p in hashes):
            there.add(f.nas)
    return there, hashes


class Report:
    def __init__(self):
        self.differ, self.not_placed, self.only_nas, self.unmapped, self.held_placed = [], [], [], [], []
        self.waiting, self.held, self.absent = [], [], []
        self.matched = self.own = 0
        self.folders = set()  # (the NAS paths with drift: a placement's default)

    def drift(self):
        return bool(self.differ or self.not_placed or self.only_nas or self.unmapped or self.held_placed)

    def lines(self, nfolders):
        notes = []
        if self.own:
            notes.append(f"the NAS's own {self.own}")
        if self.waiting:
            notes.append(f"waiting for their index {len(self.waiting)}: {', '.join(p for _, p in self.waiting)}")
        if self.held:
            notes.append(f"held {len(self.held)}: {', '.join(self.held)}")
        if self.absent:
            notes.append(f"not placed, no NAS folder: {', '.join(self.absent)}")
        tail = f"; {'; '.join(notes)}" if notes else ""
        if not self.drift():
            return [f"inputs on the NAS: as staged ({self.matched} files in {nfolders} folders{tail})"]
        parts = [f"{len(v)} {k}" for k, v in (
            ("differ", self.differ), ("not placed", self.not_placed), ("only on the NAS", self.only_nas),
            ("held but on the NAS", self.held_placed), ("unmapped", self.unmapped)) if v]
        out = [f"inputs on the NAS: DRIFT: {', '.join(parts)} ({self.matched} match{tail}; tools/dev/inputsnas.py place)"]
        out += [f"  differs: {nas} ({p})" for nas, p in self.differ]
        out += [f"  not placed: {nas} ({p})" for nas, p in self.not_placed]
        out += [f"  only on the NAS: {nas}" for nas, _ in self.only_nas]
        out += [f"  held but on the NAS: {nas} ({p})" for nas, p in self.held_placed]
        out += [f"  unmapped: {p} (no folder of tools/dev/inputsnas.py's MAPS)" for p in self.unmapped]
        return out


def compare(repo, fs, there, hashes):
    r = Report()
    r.unmapped = unmapped(repo, fs)
    for f in fs:
        if (os.path.dirname(f.nas) if f.mode == FILE else f.nas) not in there:
            if f.staged:
                r.absent.append(f"{', '.join(f.repo_dirs) or list(f.staged.values())[0]} → {f.nas}")
            continue
        def at(n):
            return f.nas if f.mode == FILE else f"{f.nas}/{n}"
        for n, p in sorted(f.staged.items()):
            h = hashes.get(at(n))
            idx = f.indexes.get(n)
            if h is None and idx and idx not in f.staged and at(idx) not in hashes:
                r.waiting.append((at(n), os.path.basename(p)))
            elif h is None:
                r.not_placed.append((at(n), p))
                r.folders.add(f.nas)
            elif h != sha(os.path.join(repo, p)):
                r.differ.append((at(n), p))
                r.folders.add(f.nas)
            else:
                r.matched += 1
        for n, p in sorted(f.held.items()):
            (r.held_placed if at(n) in hashes else r.held).append((at(n), p) if at(n) in hashes else p)
        if f.mode in (WHOLE, SHARED):
            mine = {at(n) for n in list(f.staged) + list(f.held)}
            for p in sorted(x for x in hashes if os.path.dirname(x) == f.nas and x not in mine):
                if f.mode == WHOLE:
                    r.only_nas.append((p, None))
                    r.folders.add(f.nas)
                else:
                    r.own += 1
    return r


def this_checkout():
    here = os.path.dirname(os.path.abspath(__file__))
    return subprocess.run(["git", "-C", here, "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()


def drift(args):
    repo = this_checkout()
    fs = folders(repo)
    try:
        there, hashes = read_nas(fs, args.root)
    except OSError as e:
        print(f"inputs on the NAS: NOT READ ({'the stand-in ' + args.root if args.root else 'ssh to the NAS'}: {e})")
        return 2
    r = compare(repo, fs, there, hashes)
    print("\n".join(r.lines(len([f for f in fs if f.staged]))))
    return 1 if r.drift() else 0


def git(repo, *a):
    return subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True)


def refuse_unless_main(repo):
    """None when the repository is the main checkout at origin/main with producers/ committed."""
    gd, cd = git(repo, "rev-parse", "--absolute-git-dir").stdout.strip(), git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir").stdout.strip()
    if not gd or os.path.realpath(gd) != os.path.realpath(cd):
        return "this is a worktree, not the main checkout: place from the main checkout, at origin/main"
    head, main = git(repo, "rev-parse", "HEAD").stdout.strip(), git(repo, "rev-parse", "origin/main").stdout.strip()
    if not main or head != main:
        return f"HEAD ({head[:9]}) isn't origin/main ({main[:9] or 'none'}): pull (or check out main) first"
    dirty = git(repo, "status", "--porcelain", "--", "producers").stdout.strip()
    if dirty:
        return f"producers/ has uncommitted changes: {dirty.splitlines()[0]}"
    return None


def tar_of(repo, members):
    """A tar of the repository's files, (name in the archive, repository path) each."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for name, p in members:
            ti = t.gettarinfo(os.path.join(repo, p), arcname=name)
            ti.uid = ti.gid = 0
            ti.uname = ti.gname = ""
            with open(os.path.join(repo, p), "rb") as fh:
                t.addfile(ti, fh)
    return buf.getvalue()


def place(args):
    repo = this_checkout()
    why = refuse_unless_main(repo)
    if why:
        print(f"inputsnas place: refused: {why}", file=sys.stderr)
        return 2
    fs = folders(repo)
    try:
        there, hashes = read_nas(fs, args.root)
    except OSError as e:
        print(f"inputsnas place: the NAS not read: {e}", file=sys.stderr)
        return 2
    r = compare(repo, fs, there, hashes)
    by = {f.nas: f for f in fs}
    targets = sorted(args.paths or r.folders)
    for t in targets:
        if t not in by:
            print(f"inputsnas place: {t} isn't a staged NAS path (they are: {', '.join(sorted(by))})", file=sys.stderr)
            return 2
    if not targets:
        print("\n".join(r.lines(len([f for f in fs if f.staged]))))
        print("nothing to place")
        return 0
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    base = f"{PLACE}/{stamp}"
    waiting = {w for w, _ in r.waiting}
    print(f"placing {len(targets)}: {', '.join(targets)} (staged in {base}/new/, the old kept in {base}/kept/)")
    if args.dry_run:
        for t in targets:
            f = by[t]
            print(f"  {t}: {len(f.staged)} staged files ({f.mode})")
        print("dry run: nothing written")
        return 0
    if run_there(f"[ ! -e {q(base)} ] || exit 5", args.root).returncode != 0:
        print(f"inputsnas place: {base} is already there", file=sys.stderr)
        return 2
    kept = []
    for t in targets:
        f = by[t]
        new, old = f"{base}/new/{t}", f"{base}/kept/{t}"
        if f.mode == FILE:
            into, members = os.path.dirname(new), [(os.path.basename(t), f.staged[os.path.basename(t)])]
        else:
            into = new
            members = [(n, p) for n, p in sorted(f.staged.items()) if f"{t}/{n}" not in waiting]
        expect = {(new if f.mode == FILE else f"{new}/{n}"): sha(os.path.join(repo, p)) for n, p in members}
        carry = ""
        if f.mode in (SHARED, NOTES):  # (the NAS's own files of the folder, carried over on the NAS)
            carry = f"if [ -d {q(t)} ]; then cp -pR {q(t)}/. {q(new)}/ || exit 6; fi"
            if f.mode == SHARED:
                for p, h in hashes.items():
                    n = os.path.basename(p)
                    if os.path.dirname(p) == t and n not in dict(members):
                        expect[f"{new}/{n}"] = h
        s = run_there(f"mkdir -p {q(into)} || exit 6\n{carry}\ntar -C {q(into)} -xf - || exit 7", args.root, tar_of(repo, members))
        if s.returncode != 0:
            print(f"inputsnas place: staging {t} failed (exit {s.returncode}: {s.stderr.decode('utf-8', 'replace').strip()[:300]}); left as it is in {new}", file=sys.stderr)
            return 1
        v = run_there(hash_script([], sorted(expect)), args.root)
        _, got, done = parse_hashes(v.stdout)
        bad = sorted(p for p, h in expect.items() if got.get(p) != h)
        if v.returncode != 0 or not done or bad:
            print(f"inputsnas place: {t} not verified ({len(bad)} of {len(expect)} files differ or are missing: {', '.join(bad[:5])}); not swapped; left in {new}", file=sys.stderr)
            return 1
        w = run_there(
            f"if [ -e {q(t)} ]; then mkdir -p {q(os.path.dirname(old))} && mv {q(t)} {q(old)} || exit 8; fi\nmv {q(new)} {q(t)} || exit 9",
            args.root,
        )
        if w.returncode != 0:
            print(f"inputsnas place: swapping {t} in failed (exit {w.returncode}: {w.stderr.decode('utf-8', 'replace').strip()[:300]}): look at {t}, {old} and {new}", file=sys.stderr)
            return 1
        kept.append(old)
        print(f"  {t}: {len(expect)} files verified, swapped in; the old one kept as {old}")
    units = sorted({t.split("/")[1] for t in targets if by[t].mode in (WHOLE, SHARED)})
    for u in units:
        if args.root:
            print(f"  (a stand-in root: no input check asked; on the NAS: scenic inputs check {u})")
            continue
        c = subprocess.run(["scenic", "inputs", "check", u], capture_output=True, text=True)
        print(f"  scenic inputs check {u}: {'asked' if c.returncode == 0 else 'FAILED, exit %d' % c.returncode} {(c.stdout + c.stderr).strip()}".rstrip())
    print("next:")
    if units:
        print(f"  watch the check of {', '.join(units)} (`scenic inputs`, the build page): accept the warnings you expect (`scenic inputs accept <unit> <finding id>`)")
    print(f"  once the new version is taken in, delete the kept folder {base}/ yourself (this command deletes nothing)")
    print("  then `tools/dev/inputsnas.py drift` says: as staged")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(prog="inputsnas.py", description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("drift", "place"):
        p = sub.add_parser(name)
        p.add_argument("--root", help="a local folder standing in for the NAS project folder")
        if name == "place":
            p.add_argument("--dry-run", action="store_true")
            p.add_argument("paths", nargs="*", metavar="NAS path")
    args = ap.parse_args(argv)
    if args.root:
        args.root = os.path.abspath(args.root)
    return drift(args) if args.cmd == "drift" else place(args)


if __name__ == "__main__":
    sys.exit(main())
