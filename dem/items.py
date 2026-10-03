#!/usr/bin/env python3
"""Facts and pageviews for the landmark candidates' Wikidata items, per pass epoch (the `items`
job, docs/phase5.md "items").

Input (--qids): JSON {"facts": [QIDs], "views": [QIDs]}: the items whose tag is one QID (the
facts poidetails.py attached), and every item's first QID with the heritage and World Heritage
items (whose pageviews rank them). The scenic-build `items` step writes it.

Per epoch (--epoch, the pass's date): everything is fetched again at a new epoch; within one,
only items not seen yet (a run started by new coverage doesn't move anyone else's fame). Caches
under --cache:
  facts-<epoch>.json       QID → poidetails.py's record (sitelinks, descriptions, heights …)
  wp-<epoch>.jsonl         QID → its Wikipedia articles (heritagewd.wikipedias)
  months/<m>.json, <m>.counted.json   one month's views per article (pageviews.month_views)
The four months are the last November, February, May and August before the epoch, pinned for it.

Output (--out dir): facts.json (QID → record) and views.json (QID → mean monthly views over the
four months, 1 dp), with meta.json (epoch, months, counts). Every failed query or download fails
the run (no silently skipped batch).

usage: items.py --qids qids.json --epoch 2026-09-28 --cache dir --out dir
"""
from __future__ import annotations

import argparse
import datetime
import json
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import heritagewd
import pageviews
import poidetails


def months_before(epoch: str) -> list[str]:
    """The last November, February, May and August before the epoch's month, oldest first."""
    d = datetime.date.fromisoformat(epoch)
    out, y, m = [], d.year, d.month
    while len(out) < 4:
        m -= 1
        if m == 0:
            y, m = y - 1, 12
        if m in (2, 5, 8, 11):
            out.append(f"{y:04d}-{m:02d}")
    return sorted(out)


def load_json(p: Path, default):
    return json.loads(p.read_text()) if p.exists() else default


def write_json(p: Path, v) -> None:
    tmp = p.with_suffix(".tmp")
    tmp.write_text(json.dumps(v, ensure_ascii=False))
    tmp.replace(p)


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

    # Facts: this epoch's, only the new items fetched.
    fpath = cache / f"facts-{a.epoch}.json"
    facts = load_json(fpath, {})
    todo = [q for q in facts_q if q not in facts]
    print(f"facts: {len(facts_q)} items, {len(todo)} to fetch", file=sys.stderr, flush=True)
    if todo:
        got = poidetails.wikidata(todo)
        # Items QLever doesn't know (merged, deleted) are remembered as such, not asked again.
        for q in todo:
            facts[q] = got.get(q, {"sl": 0, "missing": True})
        write_json(fpath, facts)

    # Each item's Wikipedia articles: this epoch's.
    wpath = cache / f"wp-{a.epoch}.jsonl"
    wp: dict[str, dict] = {}
    if wpath.exists():
        for line in open(wpath, encoding="utf-8"):
            r = json.loads(line)
            wp[r["qid"]] = r
    need = [q for q in views_q if q not in wp]
    print(f"articles: {len(views_q)} items, {len(need)} to look up", file=sys.stderr, flush=True)
    if need:
        for q, r in heritagewd.wikipedias(need).items():
            wp[q] = {"qid": q, **r}
        tmp = wpath.with_suffix(".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            for r in wp.values():
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        tmp.replace(wpath)

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
    write_json(out / "meta.json", {"epoch": a.epoch, "months": months, "facts": len(facts_q), "views": len(views)})
    print(f"items: {len(facts_q)} facts, {len(views)} items with views", file=sys.stderr)


if __name__ == "__main__":
    main()
