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
agents only): each ~5 GB month is streamed through bzip2 and filtered to our articles, nothing
stored. The API allows anonymous clients ten requests a minute, too few for ~100,000 articles.
One month per season is sampled (MONTHS), so summer-heavy places aren't favoured; each month's
counts are cached in data/pageviews/months/, with the articles they were counted for: articles
added later (a new region's sites, a new language) stream the month again, for those only.

Writes data/pageviews/items.json (per item: average monthly views over the sampled months).

usage: pageviews.py [--epoch YYYY-MM-DD]   (--epoch: that pass's months, months_before; else MONTHS)
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import heritagewd

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "wd"
OUT = ROOT / "data" / "pageviews"
UA = "road-elevations/0.1 (personal offline map)"
LANGS = {"en", "fr", "es", "ca", "pt", "zh", "zh-yue", "ja", "cy", "ga", "gd", "gl", "eu", "oc", "br", "co", "ast", "an", "gv"}
MONTHS = ["2025-11", "2026-02", "2026-05", "2026-08"]
DUMP = "https://dumps.wikimedia.org/other/pageview_complete/monthly/{y}/{y}-{m}/pageviews-{y}{m}-user.bz2"


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


# The dumps' bytes streamed so far, and their sizes, by month: the progress line's.
_streamed: dict[str, list[int]] = {}
_lock = threading.Lock()


def _cached(month: str) -> tuple[dict[str, int], set[str]]:
    """A month's cached views, and the articles they were counted for ({month}.json,
    {month}.counted.json; a cache from before that list counts as having counted the articles it
    has views for)."""
    path = OUT / "months" / f"{month}.json"
    counted_path = OUT / "months" / f"{month}.counted.json"
    got: dict[str, int] = json.loads(path.read_text()) if path.exists() else {}
    return got, set(json.loads(counted_path.read_text())) if counted_path.exists() else set(got)


def _dump_size(month: str) -> int:
    """A month's dump's size in bytes, as dumps.wikimedia.org says (0 when it doesn't say)."""
    y, m = month.split("-")
    try:
        req = urllib.request.Request(DUMP.format(y=y, m=m), method="HEAD", headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=30) as r:
            return int(r.headers.get("Content-Length") or 0)
    except (OSError, ValueError):
        return 0


def _report() -> None:
    with _lock:
        done, total, n = sum(v[0] for v in _streamed.values()), sum(v[1] for v in _streamed.values()), len(_streamed)
    if total:
        print(f"progress: {done >> 20}/{total >> 20} MB of the pageview dumps streamed ({n} month{'' if n == 1 else 's'})", file=sys.stderr, flush=True)


def months_views(months: list[str], wanted: set[str]) -> list[dict[str, int]]:
    """Each month's views of the wanted articles (month_views), two months streamed at once (the
    most dumps.wikimedia.org asks for), with a progress line every half minute: the bytes streamed
    of all the dumps to stream (each one's size asked first), so the build's status shows how far
    it is and when it'll be done, not the step before's last line for half an hour."""
    todo = [m for m in months if wanted - _cached(m)[1]]
    with _lock:
        _streamed.clear()
        _streamed.update({m: [0, _dump_size(m)] for m in todo})
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
        _report()


def month_views(month: str, wanted: set[str]) -> dict[str, int]:
    """Views in one month of each wanted article ("lang|Title_with_underscores"), from its dump:
    the cached counts (_cached), and the month streamed again for articles not counted yet."""
    path = OUT / "months" / f"{month}.json"
    counted_path = OUT / "months" / f"{month}.counted.json"
    got, counted = _cached(month)
    todo = wanted - counted
    if not todo:
        return got
    print(f"  {month}: counting {len(todo)} articles", file=sys.stderr, flush=True)
    y, m = month.split("-")
    langs = "|".join(sorted({a.split("|", 1)[0] for a in todo}))
    t0 = time.time()
    new: dict[str, int] = {}
    # curl | bzip2 -dc | grep, each one's exit checked: a download cut short (curl's error, bzip2's
    # truncated stream) fails the month instead of caching what arrived as counted. curl's bytes
    # reach bzip2 through here, counted (the progress line's: a few MB a second).
    curl = subprocess.Popen(["curl", "-sSL", "--fail", "-A", UA, DUMP.format(y=y, m=m)], stdout=subprocess.PIPE)
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
            pass  # (bzip2 stopped: its exit says why)
        finally:
            try:
                bz.stdin.close()
            except BrokenPipeError:
                pass

    pumper = threading.Thread(target=pump, daemon=True)
    pumper.start()
    for line in grep.stdout:
        # wiki title page_id access monthly_total hourly
        f = line.split(" ", 5)
        if len(f) < 5:
            continue
        key = f"{f[0][:-10]}|{f[1]}"
        if key in todo:
            new[key] = new.get(key, 0) + int(f[4])
    pumper.join()
    rc = (curl.wait(), bz.wait(), grep.wait())
    # (grep exits 1 when nothing matched.)
    if rc[0] != 0 or rc[1] != 0 or rc[2] not in (0, 1):
        raise RuntimeError(f"{month}: download failed (curl {rc[0]}, bzip2 {rc[1]}, grep {rc[2]})")
    got.update(new)
    path.parent.mkdir(parents=True, exist_ok=True)
    for dst, v in ((path, got), (counted_path, sorted(counted | todo))):
        tmp = dst.with_suffix(".tmp")
        tmp.write_text(json.dumps(v, ensure_ascii=False))
        tmp.replace(dst)
    print(f"  {month}: {len(new)} of {len(todo)} articles with views ({time.time() - t0:.0f} s)", file=sys.stderr, flush=True)
    return got


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
