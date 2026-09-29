#!/usr/bin/env python3
"""Which places get a written description (data/heritage/desc/WRITERS.md): heritage sites and
stops & sights with a Wikipedia article, one per Wikidata item, by how interesting they are
(interest.py): the best-known first (fame: pageviews), plus the best-known place of its kind for
LOCAL_KM around wherever it is (interest isolation), so remote regions get some too.

  targets N [--local]   rank the candidates and write data/heritage/desc/targets.jsonl (the top N
              by fame, with --local also the locally best), then fetch their articles' lead
              sections and split the ones not yet described into batches for the writers
              (v2-NNN.jsonl).
  skipped     the lines the writers skipped (the extract was about something else, or said only
              where the place is), in batches for researching from other sources (RESEARCH.md):
              research-NNN.jsonl; the researchers' notes go to research-NNN.notes.jsonl and the
              descriptions written from them, with their sources, to research-NNN.out.jsonl.

usage: desctargets.py targets N [--local] | skipped
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import heritagedetails as hd

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
D = ROOT / "data" / "heritage" / "desc"
W = ROOT / "data" / "heritage" / "wd"
LOCAL_KM = 50.0
BATCH = 125
RESEARCH_BATCH = 25
LOCAL = ["fr", "es", "pt", "ca", "zh", "cy", "ga", "gd", "gl", "eu"]
KIND = {"peak": "Mountain or hill (peak)", "waterfall": "Waterfall", "lighthouse": "Lighthouse", "viewpoint": "Viewpoint",
        "covered_bridge": "Covered bridge", "rest_area": "Rest area", "picnic_site": "Picnic site", "trailhead": "Trailhead"}


def jsonl(p: Path) -> list[dict]:
    return [json.loads(l) for l in open(p, encoding="utf-8")] if p.exists() else []


def pick_article(arts: list[str], en: str | None = None) -> tuple[str, str] | None:
    """English, else a language of the map's regions, else any (from "lang|title" strings)."""
    have = dict(a.split("|", 1) for a in arts if "|" in a)
    if en:
        have.setdefault("en", en)
    for l in ["en", *LOCAL]:
        if l in have:
            return l, have[l]
    return min(have.items()) if have else None


def candidates() -> list[dict]:
    """One candidate per Wikidata item with an article: its best-scoring site's fame and isolation."""
    wp = {r["qid"]: r.get("arts", []) for r in jsonl(W / "wp.jsonl")}
    out: dict[str, dict] = {}

    def add(qid: str, rec: dict) -> None:
        cur = out.get(qid)
        if cur is None or (rec["fa"], rec["ia"]) > (cur["fa"], cur["ia"]):
            out[qid] = rec

    hdet = {r["i"]: r for r in jsonl(B / "details-heritage.jsonl")}
    for f in json.load(open(B / "heritage.json"))["features"]:
        p = f["properties"]
        d = hdet.get(p.get("i"))
        if not d or not d.get("wiki"):
            continue
        art = (d["wiki"]["lang"], d["wiki"]["title"])
        add(d["qid"], {"qid": d["qid"], "layer": "heritage", "i": p["i"], "name": p.get("name_en") or p.get("name", ""),
                       "designation": p.get("designation", ""), "place": p.get("municipality", ""), "lang": art[0], "title": art[1],
                       "fa": p.get("fa", 0.0), "ia": p.get("ia", 0.0), "pv": p.get("pv", 0)})
    pdet = {r["i"]: r for r in jsonl(B / "details-poi.jsonl")}
    for f in json.load(open(B / "pois.json"))["features"]:
        p = f["properties"]
        d = pdet.get(p.get("i"), {})
        qid = (d.get("wikidata") or "").split(";")[0].strip()
        if not qid.startswith("Q") or p.get("fa", 0) <= 0.01:  # no fame counted (e.g. a mine tagged as a viewpoint)
            continue
        art = pick_article(wp.get(qid, []), (d.get("wd") or {}).get("w_en"))
        if not art:
            continue
        add(qid, {"qid": qid, "layer": "poi", "i": p["i"], "name": p.get("name", ""), "designation": KIND.get(p["kind"], p["kind"]),
                  "place": "", "lang": art[0], "title": art[1], "fa": p.get("fa", 0.0), "ia": p.get("ia", 0.0), "pv": p.get("pv", 0)})
    return sorted(out.values(), key=lambda r: -r["fa"])


def targets(n: int, with_local: bool = False) -> None:
    cands = candidates()
    top = cands[:n]
    local = [r for r in cands[n:] if r["ia"] >= LOCAL_KM] if with_local else []
    pick = top + local
    print(f"{len(cands)} candidates with an article; top {len(top)} by fame "
          f"(down to {top[-1]['pv'] if top else 0} views a month), plus {len(local)} locally best (≥ {LOCAL_KM:.0f} km)", file=sys.stderr)
    by_layer = {}
    for r in pick:
        by_layer[r["layer"]] = by_layer.get(r["layer"], 0) + 1
    print(f"  {by_layer}", file=sys.stderr)
    D.mkdir(parents=True, exist_ok=True)
    with open(D / "targets.jsonl", "w", encoding="utf-8") as f:
        for r in pick:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    ex = hd.fetch_extracts(pick)
    done = hd.long_descriptions(prefer_v2_only=True)
    todo = [ex[r["qid"]] for r in pick if r["qid"] in ex and r["qid"] not in done]
    for old in D.glob("v2-*.jsonl"):
        if not old.name.endswith(".out.jsonl"):
            old.unlink()
    for b in range(0, len(todo), BATCH):
        with open(D / f"v2-{b // BATCH:03d}.jsonl", "w", encoding="utf-8") as f:
            for r in todo[b:b + BATCH]:
                f.write(json.dumps({k: r[k] for k in ("qid", "name", "designation", "place", "lang", "title", "extract")}, ensure_ascii=False) + "\n")
    print(f"{len(ex)} extracts; {len(todo)} to write in {(len(todo) + BATCH - 1) // BATCH} batches (v2-NNN.jsonl)", file=sys.stderr)


def skipped() -> None:
    done = hd.long_descriptions(prefer_v2_only=True)  # pilot-round descriptions (old voice) don't count
    rows = []
    for p in sorted(D.glob("v2-[0-9][0-9][0-9].jsonl")):
        if p.with_name(p.stem + ".out.jsonl").exists():  # batches still being written don't count yet
            rows += [r for r in jsonl(p) if r["qid"] not in done]
    for old in D.glob("research-[0-9][0-9][0-9].jsonl"):
        old.unlink()
    for b in range(0, len(rows), RESEARCH_BATCH):
        with open(D / f"research-{b // RESEARCH_BATCH:03d}.jsonl", "w", encoding="utf-8") as f:
            for r in rows[b:b + RESEARCH_BATCH]:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"{len(rows)} skipped, in {(len(rows) + RESEARCH_BATCH - 1) // RESEARCH_BATCH} batches (research-NNN.jsonl)", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "targets":
        targets(int(sys.argv[2]), "--local" in sys.argv)
    elif sys.argv[1:] == ["skipped"]:
        skipped()
    else:
        print(__doc__, file=sys.stderr)
