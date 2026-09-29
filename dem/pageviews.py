#!/usr/bin/env python3
"""Wikipedia pageviews for heritage sites and stops & sights (a measure of how much people read
about a place, for ranking what the map shows most prominently).

For each Wikidata item of a heritage site (data/heritage/wd/items.jsonl) or a POI
(data/build/details-poi.jsonl), its Wikipedia articles in the languages of the map's regions
(English, French, Spanish, Catalan, Portuguese, Chinese, Welsh, Irish, Scottish Gaelic, Galician,
Basque …) are counted and summed. The item's articles come from data/heritage/wd/wp.jsonl
(heritagewd.py), filled here for POI items not in it.

Views come from Wikimedia's monthly pageview dumps (dumps.wikimedia.org, pageview_complete, user
agents only): each ~5 GB month is streamed through bzip2 and filtered to our articles, nothing
stored. The API allows anonymous clients ten requests a minute, too few for ~100,000 articles.
One month per season is sampled (MONTHS), so summer-heavy places aren't favoured; each month's
counts are cached in data/pageviews/months/.

Writes data/pageviews/items.json (per item: average monthly views over the sampled months).

usage: pageviews.py
"""
from __future__ import annotations

import json
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import heritagewd

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "wd"
OUT = ROOT / "data" / "pageviews"
UA = "road-elevations/0.1 (personal offline map)"
LANGS = {"en", "fr", "es", "ca", "pt", "zh", "zh-yue", "cy", "ga", "gd", "gl", "eu", "oc", "br", "co", "ast", "an", "gv"}
MONTHS = ["2025-11", "2026-02", "2026-05", "2026-08"]
DUMP = "https://dumps.wikimedia.org/other/pageview_complete/monthly/{y}/{y}-{m}/pageviews-{y}{m}-user.bz2"


def month_views(month: str, wanted: set[str]) -> dict[str, int]:
    """Views in one month of each wanted article ("lang|Title_with_underscores"), from its dump."""
    path = OUT / "months" / f"{month}.json"
    if path.exists():
        return json.loads(path.read_text())
    y, m = month.split("-")
    langs = "|".join(sorted(l.replace("-", "\\-") for l in LANGS))
    cmd = (f"curl -sSL --fail -A '{UA}' '{DUMP.format(y=y, m=m)}' | bzip2 -dc | "
           f"LC_ALL=C grep -E '^({langs})\\.wikipedia '")
    t0 = time.time()
    got: dict[str, int] = {}
    p = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace")
    for line in p.stdout:
        # wiki title page_id access monthly_total hourly
        f = line.split(" ", 5)
        if len(f) < 5:
            continue
        key = f"{f[0][:-10]}|{f[1]}"
        if key in wanted:
            got[key] = got.get(key, 0) + int(f[4])
    if p.wait() != 0:
        raise RuntimeError(f"{month}: download failed")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(got, ensure_ascii=False))
    print(f"  {month}: {len(got)} articles with views ({time.time() - t0:.0f} s)", file=sys.stderr, flush=True)
    return got


def main() -> None:
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
        with open(wp_path, "w", encoding="utf-8") as f:
            for r in wp.values():
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    arts_of = {q: [a for a in wp.get(q, {}).get("arts", []) if "|" in a and a.split("|", 1)[0] in LANGS] for q in qids}

    wanted = {a.split("|", 1)[0] + "|" + a.split("|", 1)[1].replace(" ", "_") for arts in arts_of.values() for a in arts}
    print(f"{len(wanted)} articles; months {', '.join(MONTHS)}", file=sys.stderr, flush=True)
    with ThreadPoolExecutor(2) as ex:  # dumps.wikimedia.org asks for at most two or three connections
        per_month = list(ex.map(lambda m: month_views(m, wanted), MONTHS))
    views = {a: sum(pm.get(a, 0) for pm in per_month) for a in wanted}
    months = len(MONTHS)
    per_item = {q: round(sum(views.get(a.split("|", 1)[0] + "|" + a.split("|", 1)[1].replace(" ", "_"), 0) for a in arts) / months, 1)
                for q, arts in arts_of.items() if arts}
    (OUT / "items.json").write_text(json.dumps(per_item, ensure_ascii=False))
    top = sorted(per_item.items(), key=lambda x: -x[1])[:10]
    print(f"items.json: {len(per_item)} items; most read: {top}", file=sys.stderr)


if __name__ == "__main__":
    main()
