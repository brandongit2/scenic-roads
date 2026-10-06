#!/usr/bin/env python3
"""Wikipedia pageviews for heritage sites and stops & sights (a measure of how much people read
about a place, for ranking what the map shows most prominently).

For each Wikidata item of a heritage site (data/heritage/wd/items.jsonl), a World Heritage Site's
component (data/build/whs-sites.json) or a POI (data/build/details-poi.jsonl), its Wikipedia
articles in the languages of the map's regions
(English, French, Spanish, Catalan, Portuguese, Chinese, Welsh, Irish, Scottish Gaelic, Galician,
Basque …) are counted and summed. The item's articles come from data/heritage/wd/wp.jsonl
(heritagewd.py), filled here for POI items not in it.

Views come from Wikimedia's monthly pageview dumps (dumps.wikimedia.org, pageview_complete, user
agents only). The API allows anonymous clients ten requests a minute, too few for ~100,000
articles. One month per season is sampled (MONTHS), so summer-heavy places aren't favoured.
A month is streamed (~5 GB through bzip2) once: every article of the map's languages is counted,
not just the ones wanted now, into the month's index, kept on the NAS (SCENIC_PAGEVIEWS_STORE:
sources/pageviews/<month>.tsv.zst, "lang|Title<TAB>views" lines after a "#langs<TAB>…" line, zstd)
and here (data/pageviews/months/); any article of those languages, of any step, any run, is then
looked up there (a language added later streams the month again). (Before the index: each month's
counts for the articles asked, data/pageviews/months/<month>.json with <month>.counted.json; still
read while they cover what's asked.)

Writes data/pageviews/items.json (per item: average monthly views over the sampled months).

usage: pageviews.py [--epoch YYYY-MM-DD]   (--epoch: that pass's months, months_before; else MONTHS)
"""
from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from compression import zstd
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import heritagewd

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "wd"
OUT = ROOT / "data" / "pageviews"
UA = "road-elevations/0.1 (personal offline map)"
LANGS = {"en", "fr", "es", "ca", "pt", "zh", "zh-yue", "ja", "cy", "ga", "gd", "gl", "eu", "oc", "br", "co", "ast", "an", "gv"}
# The languages of the indexes made before they said theirs (the first line, "#langs<TAB>en,fr,…"):
# LANGS as it was then, written out (LANGS may grow; these indexes don't).
LANGS_UNSAID = frozenset({"en", "fr", "es", "ca", "pt", "zh", "zh-yue", "ja", "cy", "ga", "gd", "gl", "eu", "oc", "br", "co", "ast", "an", "gv"})
MONTHS = ["2025-11", "2026-02", "2026-05", "2026-08"]
DUMP = "https://dumps.wikimedia.org/other/pageview_complete/monthly/{y}/{y}-{m}/pageviews-{y}{m}-user.bz2"
# The NAS's months' indexes (scenic-build sets it); none: here only.
STORE = Path(os.environ["SCENIC_PAGEVIEWS_STORE"]) if os.environ.get("SCENIC_PAGEVIEWS_STORE") else None


def months_before(epoch: str) -> list[str]:
    """The last November, February, May and August that ended at least 20 days before the epoch
    (dumps.wikimedia.org publishes a month's dump in its first days), oldest first: a pass's months,
    the same for the items job and the heritage chain."""
    import datetime

    d = datetime.date.fromisoformat(epoch)
    out, y, m = [], d.year, d.month
    while len(out) < 4:
        end = datetime.date(y, m, 1)  # the day after the month before (y, m) ended
        m -= 1
        if m == 0:
            y, m = y - 1, 12
        if m in (2, 5, 8, 11) and (d - end).days >= 20:
            out.append(f"{y:04d}-{m:02d}")
    return sorted(out)


def _cached(month: str) -> tuple[dict[str, int], set[str]]:
    """A month's views from before its index, and the articles they were counted for
    ({month}.json, {month}.counted.json; a cache from before that list counts as having counted
    the articles it has views for)."""
    path = OUT / "months" / f"{month}.json"
    counted_path = OUT / "months" / f"{month}.counted.json"
    got: dict[str, int] = json.loads(path.read_text()) if path.exists() else {}
    return got, set(json.loads(counted_path.read_text())) if counted_path.exists() else set(got)


def _size(p: Path) -> int | None:
    """A file's size; None when it isn't there. (Any other error raises: a NAS that doesn't answer
    is no reason to stream a month for an hour.)"""
    try:
        return p.stat().st_size
    except (FileNotFoundError, NotADirectoryError):
        return None


def _langs(index: Path) -> frozenset[str]:
    """The languages an index counted (its first line; one from before that line: LANGS_UNSAID)."""
    with zstd.open(index, "rt", encoding="utf-8") as f:
        first = f.readline().rstrip("\n")
    return frozenset(first.split("\t", 1)[1].split(",")) if first.startswith("#langs\t") else LANGS_UNSAID


def _index(month: str, langs: set[str], fresh: bool = False) -> Path | None:
    """The month's index here, copied whole from the NAS the first time (or again: fresh), and
    uploaded there if the NAS has none (an upload that failed before); None when neither has one
    counting every language in `langs`."""
    local = OUT / "months" / f"{month}.tsv.zst"
    nas = STORE / local.name if STORE else None
    _sweep(local.parent)
    if nas:
        _sweep(nas.parent)
    if fresh:
        local.unlink(missing_ok=True)
    if _size(local) is None and nas and _size(nas) is not None:
        _copy_whole(nas, local)
    if _size(local) is None:
        return None
    try:
        covered = _langs(local)
    except (EOFError, zstd.ZstdError, UnicodeDecodeError) as e:
        # (The NAS's copied again; that one damaged too, or none: the month streamed again.)
        print(f"  {month}: its index is damaged ({e})", file=sys.stderr, flush=True)
        if not fresh and nas and _size(nas) is not None:
            return _index(month, langs, fresh=True)
        local.unlink(missing_ok=True)
        return None
    if not langs <= covered:
        # (The NAS's may count them: another Mac's, made since a language was added.)
        if not fresh and nas and _size(nas) is not None and _size(nas) != _size(local):
            return _index(month, langs, fresh=True)
        return None
    # (Only when the NAS has none: one that differs may be the good one, this copy damaged.)
    if nas and _size(nas) is None:
        try:
            _copy_whole(local, nas)
        except OSError as e:
            print(f"  {month}: its index isn't on the NAS yet ({e}); tried again next time", file=sys.stderr, flush=True)
    return local


def _sweep(d: Path) -> None:
    """Temporary files of this Mac's copies and streams whose process is gone (killed midway)."""
    host = socket.gethostname().split(".")[0]
    try:
        names = [p.name for p in d.iterdir()]
    except FileNotFoundError:
        return
    for n in names:
        parts = n.split(".")
        if not n.endswith(".tmp") or ".tsv.zst." not in n or not parts[-2].isdigit():
            continue
        # (<month>.tsv.zst.<host>.<pid>.tmp, the NAS's and copies'; <month>.tsv.zst.<pid>.tmp here.)
        if len(parts) >= 6 and parts[-3] != host:
            continue
        try:
            os.kill(int(parts[-2]), 0)
        except ProcessLookupError:
            (d / n).unlink(missing_ok=True)
        except PermissionError:
            pass


def _copy_whole(src: Path, dst: Path) -> None:
    """Copies src to dst by a temporary name (this Mac's and process's), flushed, its length checked.
    (Its bytes count on the progress line as its month's index's: `_indexes`.)"""
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(f"{dst.name}.{socket.gethostname().split('.')[0]}.{os.getpid()}.tmp")
    try:
        with open(src, "rb") as a, open(tmp, "wb") as b:
            shutil.copyfileobj(_Counted(a, (dst.name.split(".", 1)[0], "copied")), b, 4 << 20)
            b.flush()
            os.fsync(b.fileno())
        if tmp.stat().st_size != src.stat().st_size:
            raise OSError(f"{dst}: {tmp.stat().st_size} of {src.stat().st_size} bytes copied")
        tmp.replace(dst)
    finally:
        tmp.unlink(missing_ok=True)


def _langs_of(wanted: set[str]) -> set[str]:
    return {a.split("|", 1)[0] for a in wanted}


def _dump_size(month: str) -> int:
    """A month's dump's size in bytes, as dumps.wikimedia.org says (0 when it doesn't say)."""
    y, m = month.split("-")
    try:
        req = urllib.request.Request(DUMP.format(y=y, m=m), method="HEAD", headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=30) as r:
            return int(r.headers.get("Content-Length") or 0)
    except (OSError, ValueError):
        return 0


# The dumps' bytes streamed so far, and their sizes, by month: the progress line's.
_streamed: dict[str, list[int]] = {}
# The months' indexes' bytes (compressed, as kept) copied to or from the NAS and read so far, by
# month and which: the progress line's too, so it moves while they're worked on.
_indexes: dict[tuple[str, str], list[int]] = {}
# The bytes the work's expected to move, all told (`_plan`): worked out before the progress line's
# first, so its total doesn't grow as each month starts (the bar would step back).
_planned = [0]
_lock = threading.Lock()


def _covers(p: Path, langs: set[str]) -> bool:
    """Whether the index at `p` counted every language in `langs` (a damaged one doesn't)."""
    try:
        return langs <= _langs(p)
    except (EOFError, zstd.ZstdError, UnicodeDecodeError, OSError):
        return False


def _plan(month: str, wanted: set[str]) -> int:
    """The bytes a month's work is expected to move, as far as can be told before it starts: its
    index read here (copied from the NAS first when only the NAS has one), else its dump streamed
    (when its counts from before don't cover what's asked; 5 GB when the server doesn't say)."""
    langs = _langs_of(wanted)
    local = OUT / "months" / f"{month}.tsv.zst"
    nas = STORE / local.name if STORE else None
    if _size(local) is not None and _covers(local, langs):
        return _size(local)
    if nas and _size(nas) is not None and _covers(nas, langs):
        return 2 * _size(nas)
    if not wanted - _cached(month)[1]:
        return 0
    return _dump_size(month) or 5 << 30


class _Counted:
    """A file read through, its bytes counted in `_indexes` under `key`."""

    def __init__(self, f, key: tuple[str, str]):
        self.f = f
        with _lock:
            self.counts = _indexes.setdefault(key, [0, 0])

    def read(self, n: int = -1) -> bytes:
        b = self.f.read(n)
        with _lock:
            self.counts[0] += len(b)
        return b

    def seekable(self) -> bool:
        # (Asked by zstd's reader: this is only read straight through.)
        return False


def _report(final: bool = False) -> None:
    # (Of the bytes planned (`_plan`): until the end the bar stops short of full, the work moving
    # more than planned (a dump larger than said, an index copied again).)
    with _lock:
        done = sum(v[0] for v in _streamed.values()) + sum(v[0] for v in _indexes.values())
        # (The months said are those with bytes moved.)
        n = len(set(_streamed) | {m for m, _ in _indexes})
    total = _planned[0]
    if not total:
        return
    done = total if final else min(done, total * 99 // 100)
    print(f"progress: {done >> 20}/{total >> 20} MB of the pageview dumps streamed ({n} month{'' if n == 1 else 's'})", file=sys.stderr, flush=True)


def months_views(months: list[str], wanted: set[str]) -> list[dict[str, int]]:
    """Each month's views of the wanted articles (month_views), two months streamed at once (the
    most dumps.wikimedia.org asks for), with a progress line every half minute: the bytes streamed
    of all the dumps to stream (each one's size asked first), and of the months' indexes as they're
    copied and read, so the build's status shows how far it is and when it'll be done, not the step
    before's last line for half an hour."""
    with _lock:
        _streamed.clear()
        _indexes.clear()
    _planned[0] = sum(_plan(m, wanted) for m in months)
    stop = threading.Event()

    def report() -> None:
        while not stop.wait(30):
            _report()

    threading.Thread(target=report, daemon=True).start()
    try:
        with ThreadPoolExecutor(2) as ex:
            return list(ex.map(lambda m: month_views(m, wanted), months))
    finally:
        stop.set()
        _report(final=True)


def month_views(month: str, wanted: set[str]) -> dict[str, int]:
    """Views in one month of each wanted article ("lang|Title_with_underscores"): looked up in the
    month's index; before there's one, the counts from before it when they cover what's asked; else
    the month is streamed once into its index, every article of the map's languages counted."""
    langs = _langs_of(wanted)
    index = _index(month, langs)
    if index is None:
        got, counted = _cached(month)
        if not wanted - counted:
            return {a: v for a, v in got.items() if a in wanted}
        index = _stream_index(month)
    # (A damaged index: the NAS's copied again, then the month streamed again.)
    for attempt in ("nas", "stream", None):
        try:
            return _look_up(month, index, wanted)
        except (EOFError, zstd.ZstdError, UnicodeDecodeError, ValueError) as e:
            if attempt is None:
                raise
            print(f"  {month}: its index is damaged ({e}); {'copying the NAS’s again' if attempt == 'nas' else 'streaming the month again'}", file=sys.stderr, flush=True)
            index = (_index(month, langs, fresh=True) if attempt == "nas" else None) or _stream_index(month)
    raise AssertionError("unreachable")


def _look_up(month: str, index: Path, wanted: set[str]) -> dict[str, int]:
    t0 = time.time()
    # (Marked used: the build agent's room-making lets the least recently used copies go first.)
    try:
        os.utime(index)
    except OSError:
        pass
    views: dict[str, int] = {}
    # (Its compressed bytes count on the progress line as they're read: `_indexes`.)
    with open(index, "rb") as raw, zstd.open(_Counted(raw, (month, "read")), "rt", encoding="utf-8") as f:
        for line in f:
            key, _, n = line.rstrip("\n").partition("\t")
            if key in wanted:
                # (A page's lines are summed: its access kinds, adjacent or not.)
                views[key] = views.get(key, 0) + int(n)
    print(f"  {month}: {len(views)} of {len(wanted)} articles with views, from its index ({time.time() - t0:.0f} s)", file=sys.stderr, flush=True)
    return views


def _stream_index(month: str) -> Path:
    """Streams a month's dump into its index, here then on the NAS: every article of the map's
    languages, its views summed over the dump's adjacent lines for it (its access kinds)."""
    local = OUT / "months" / f"{month}.tsv.zst"
    local.parent.mkdir(parents=True, exist_ok=True)
    print(f"  {month}: streaming its dump, every article of the map's languages counted", file=sys.stderr, flush=True)
    y, m = month.split("-")
    langs = "|".join(sorted(LANGS))
    t0 = time.time()
    # curl | bzip2 -dc | grep, each one's exit checked: a download cut short (curl's error, bzip2's
    # truncated stream) fails the month instead of keeping what arrived as its index. curl's bytes
    # reach bzip2 through here, counted (the progress line's: a few MB a second). A download that
    # stalls (under 10 kB/s for five minutes) is given up.
    curl = subprocess.Popen(["curl", "-sSL", "--fail", "--connect-timeout", "30", "--speed-limit", "10000", "--speed-time", "300", "-A", UA, DUMP.format(y=y, m=m)], stdout=subprocess.PIPE)
    bz = subprocess.Popen(["bzip2", "-dc"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    grep = subprocess.Popen(["grep", "-E", f"^({langs})\\.wikipedia "], stdin=bz.stdout, stdout=subprocess.PIPE,
                            env={**os.environ, "LC_ALL": "C"}, text=True, encoding="utf-8", errors="replace")
    bz.stdout.close()
    with _lock:
        _streamed.setdefault(month, [0, 0])

    def pump() -> None:
        try:
            while chunk := curl.stdout.read1(1 << 20):
                bz.stdin.write(chunk)
                with _lock:
                    _streamed[month][0] += len(chunk)
        except BrokenPipeError:
            # (bzip2 stopped, or grep and so bzip2: their exits say why. curl, with no one left to
            # read it, would wait on its full pipe for good.)
            curl.kill()
        finally:
            curl.stdout.close()
            try:
                bz.stdin.close()
            except BrokenPipeError:
                pass

    pumper = threading.Thread(target=pump, daemon=True)
    pumper.start()
    tmp = local.with_name(f"{local.name}.{os.getpid()}.tmp")
    rows = 0
    try:
        with zstd.open(tmp, "wt", encoding="utf-8") as out:
            out.write(f"#langs\t{','.join(sorted(LANGS))}\n")
            last, total = None, 0
            for line in grep.stdout:
                # wiki title page_id access monthly_total hourly
                f = line.split(" ", 5)
                if len(f) < 5 or not f[4].isdigit():
                    continue
                key = f"{f[0][:-10]}|{f[1]}"
                if key != last:
                    if last is not None:
                        out.write(f"{last}\t{total}\n")
                        rows += 1
                    last, total = key, 0
                total += int(f[4])
            if last is not None:
                out.write(f"{last}\t{total}\n")
                rows += 1
        pumper.join()
        rc = (curl.wait(), bz.wait(), grep.wait())
        # (grep exits 1 when nothing matched.)
        if rc[0] != 0 or rc[1] != 0 or rc[2] not in (0, 1):
            raise RuntimeError(f"{month}: download failed (curl {rc[0]}, bzip2 {rc[1]}, grep {rc[2]})")
        with open(tmp, "rb") as f:
            os.fsync(f.fileno())
        tmp.replace(local)
    finally:
        tmp.unlink(missing_ok=True)
    if STORE:
        # (One that fails is tried again the next time the month is looked up: _index.)
        try:
            _copy_whole(local, STORE / local.name)
        except OSError as e:
            print(f"  {month}: its index isn't on the NAS yet ({e})", file=sys.stderr, flush=True)
    # (The counts from before the index: it has them all now.)
    for old in (f"{month}.json", f"{month}.counted.json"):
        (OUT / "months" / old).unlink(missing_ok=True)
    print(f"  {month}: {rows} articles in its index, {local.stat().st_size >> 20} MB ({time.time() - t0:.0f} s)", file=sys.stderr, flush=True)
    return local


def main() -> None:
    global MONTHS
    if "--epoch" in sys.argv:
        MONTHS = months_before(sys.argv[sys.argv.index("--epoch") + 1])
    OUT.mkdir(parents=True, exist_ok=True)
    qids: set[str] = set()
    for line in open(W / "items.jsonl", encoding="utf-8"):
        r = json.loads(line)
        if r.get("wiki"):
            qids.add(r["qid"])
    for line in open(B / "details-poi.jsonl", encoding="utf-8"):
        q = json.loads(line).get("wikidata", "")
        if q.startswith("Q"):
            qids.add(q.split(";")[0].strip())
    # World Heritage Sites' components' items (whsshapes.py): a site is as well known as its best
    # known part (the Rideau Canal's own item has no articles; the canal's has).
    if (B / "whs-sites.json").exists():
        for s in json.loads((B / "whs-sites.json").read_text()).values():
            qids.update(s.get("q", []))
    # Every item's Wikipedia articles (shared cache with heritagewd.py).
    wp: dict[str, dict] = {}
    wp_path = W / "wp.jsonl"
    if wp_path.exists():
        for line in open(wp_path, encoding="utf-8"):
            r = json.loads(line)
            wp[r["qid"]] = r
    need = sorted(qids - set(wp))
    print(f"{len(qids)} items; articles to look up for {len(need)}", file=sys.stderr, flush=True)
    if need:
        for q, r in heritagewd.wikipedias(need).items():
            wp[q] = {"qid": q, **r}
        heritagewd.write_atomic(wp_path, "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in wp.values()))
    arts_of = {q: [a for a in wp.get(q, {}).get("arts", []) if "|" in a and a.split("|", 1)[0] in LANGS] for q in qids}

    wanted = {a.split("|", 1)[0] + "|" + a.split("|", 1)[1].replace(" ", "_") for arts in arts_of.values() for a in arts}
    print(f"{len(wanted)} articles; months {', '.join(MONTHS)}", file=sys.stderr, flush=True)
    per_month = months_views(MONTHS, wanted)
    views = {a: sum(pm.get(a, 0) for pm in per_month) for a in wanted}
    months = len(MONTHS)
    per_item = {q: round(sum(views.get(a.split("|", 1)[0] + "|" + a.split("|", 1)[1].replace(" ", "_"), 0) for a in arts) / months, 1)
                for q, arts in arts_of.items() if arts}
    heritagewd.write_atomic(OUT / "items.json", json.dumps(per_item, ensure_ascii=False))
    top = sorted(per_item.items(), key=lambda x: -x[1])[:10]
    print(f"items.json: {len(per_item)} items; most read: {top}", file=sys.stderr)


if __name__ == "__main__":
    main()
