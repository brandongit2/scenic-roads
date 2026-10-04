#!/usr/bin/env python3
"""Facts and pageviews for the landmark candidates' Wikidata items, per pass epoch (the `items`
job, docs/phase5.md "items").

Input (--qids): JSON {"facts": [QIDs], "views": [QIDs]}, from the current units' candidates:
"facts" the items of tags that are one QID (as today: a multi-QID tag gets no facts), "views"
every candidate's first QID. (The heritage sites' pageviews are still the heritage job's.) The
scenic-build `items` step writes it.

Per epoch (--epoch, the pass's date): everything is fetched again at a new epoch; within one,
only items not seen yet (a run started by new coverage doesn't move anyone else's fame). Caches
under --cache, appended a chunk at a time (a run stopped midway keeps what it fetched):
  facts-<epoch>.jsonl      QID → poidetails.py's record (sitelinks, descriptions, heights …)
  wp-<epoch>.jsonl         QID → its Wikipedia articles (heritagewd.wikipedias)
  fetched-<epoch>.json     the first and last days anything was fetched for the epoch
  months/<m>.json, <m>.counted.json   one month's views per article (pageviews.month_views)
Older epochs' files, and months older than the epoch's, go at the end of a run.
The four months are the last November, February, May and August whose dumps are out by the epoch
(ended at least 20 days before it), pinned for it.

Output (--out dir): facts.json (QID → record) and views.json (QID → mean monthly views over the
four months, 1 dp), with meta.json (epoch, months, the days fetched, counts). Every failed query or
download fails the run (no silently skipped batch).

usage: items.py --qids qids.json --epoch 2026-09-28 --cache dir --out dir
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import heritagewd
import pageviews
import poidetails

# Items fetched between cache writes.
CHUNK = 5000


# The epoch's four months (pageviews.months_before, shared with the heritage chain).
months_before = pageviews.months_before


def load_json(p: Path, default):
    return json.loads(p.read_text()) if p.exists() else default


def write_json(p: Path, v) -> None:
    tmp = p.with_suffix(".tmp")
    tmp.write_text(json.dumps(v, ensure_ascii=False))
    tmp.replace(p)


def read_jsonl(p: Path) -> dict[str, dict]:
    """Records by "qid"; a last line cut short (a run stopped mid-write) is dropped from the file."""
    out: dict[str, dict] = {}
    if not p.exists():
        return out
    b = p.read_bytes()
    whole = b[: b.rfind(b"\n") + 1]
    if len(whole) != len(b):
        with open(p, "r+b") as f:
            f.truncate(len(whole))
    for line in whole.decode("utf-8").splitlines():
        r = json.loads(line)
        out[r["qid"]] = r
    return out


def append_jsonl(p: Path, rows: list[dict]) -> None:
    with open(p, "a", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
        f.flush()
        os.fsync(f.fileno())


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--qids", required=True)
    ap.add_argument("--epoch", required=True)
    ap.add_argument("--cache", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    cache, out = Path(a.cache), Path(a.out)
    cache.mkdir(parents=True, exist_ok=True)
    out.mkdir(parents=True, exist_ok=True)
    want = json.loads(Path(a.qids).read_text())
    facts_q = sorted(set(want.get("facts", [])))
    views_q = sorted(set(want.get("views", [])))
    dpath = cache / f"fetched-{a.epoch}.json"
    fetched = load_json(dpath, {})

    def note_fetch() -> None:
        today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
        fetched.setdefault("first", today)
        fetched["last"] = today
        write_json(dpath, fetched)

    # Facts: this epoch's, only the new items fetched.
    fpath = cache / f"facts-{a.epoch}.jsonl"
    facts = {q: {k: v for k, v in r.items() if k != "qid"} for q, r in read_jsonl(fpath).items()}
    todo = [q for q in facts_q if q not in facts]
    print(f"facts: {len(facts_q)} items, {len(todo)} to fetch", file=sys.stderr, flush=True)
    for k in range(0, len(todo), CHUNK):
        # (A line the build agent shows as this job's progress.)
        print(f"progress: {k}/{len(todo)} items' facts fetched from Wikidata", file=sys.stderr, flush=True)
        part = todo[k:k + CHUNK]
        got = poidetails.wikidata(part)
        # Items QLever doesn't know (merged, deleted) are remembered as such, not asked again.
        rows = [{"qid": q, **got.get(q, {"sl": 0, "missing": True})} for q in part]
        append_jsonl(fpath, rows)
        for r in rows:
            facts[r["qid"]] = {k2: v for k2, v in r.items() if k2 != "qid"}
        note_fetch()

    # Each item's Wikipedia articles: this epoch's.
    wpath = cache / f"wp-{a.epoch}.jsonl"
    wp = read_jsonl(wpath)
    need = [q for q in views_q if q not in wp]
    print(f"articles: {len(views_q)} items, {len(need)} to look up", file=sys.stderr, flush=True)
    for k in range(0, len(need), CHUNK):
        print(f"progress: {k}/{len(need)} items' Wikipedia articles looked up", file=sys.stderr, flush=True)
        rows = [{"qid": q, **r} for q, r in heritagewd.wikipedias(need[k:k + CHUNK]).items()]
        append_jsonl(wpath, rows)
        wp.update((r["qid"], r) for r in rows)
        note_fetch()

    # Pageviews over the epoch's four months (pageviews.month_views caches each month's counts).
    months = months_before(a.epoch)
    pageviews.OUT = cache
    arts_of = {q: [x for x in wp.get(q, {}).get("arts", []) if "|" in x and x.split("|", 1)[0] in pageviews.LANGS] for q in views_q}
    norm = lambda x: x.split("|", 1)[0] + "|" + x.split("|", 1)[1].replace(" ", "_")
    wanted = {norm(x) for arts in arts_of.values() for x in arts}
    print(f"views: {len(wanted)} articles over {', '.join(months)}", file=sys.stderr, flush=True)
    with ThreadPoolExecutor(2) as ex:  # dumps.wikimedia.org asks for at most two or three connections
        per_month = list(ex.map(lambda m: pageviews.month_views(m, wanted), months))
    views = {q: round(sum(pm.get(norm(x), 0) for pm in per_month for x in arts) / len(months), 1) for q, arts in arts_of.items() if arts}

    write_json(out / "facts.json", {q: facts[q] for q in facts_q if not facts[q].get("missing")})
    write_json(out / "views.json", views)
    write_json(out / "meta.json", {"epoch": a.epoch, "months": months, "fetched": [fetched.get("first"), fetched.get("last")], "facts": len(facts_q), "views": len(views)})
    # Older epochs' caches go, and months before this epoch's (later epochs' months are later).
    for p in cache.iterdir():
        stem = p.name.split(".", 1)[0]
        if p.is_file() and "-" in stem and stem.split("-", 1)[0] in ("facts", "wp", "fetched") and stem.split("-", 1)[1] < a.epoch:
            p.unlink()
    for p in (cache / "months").glob("*.json"):
        if p.name[:7] < months[0]:
            p.unlink()
    print(f"items: {len(facts_q)} facts, {len(views)} items with views", file=sys.stderr)


if __name__ == "__main__":
    main()
