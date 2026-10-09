#!/usr/bin/env python3
"""Heritage site details: the short description and Wikipedia facts from heritagewd.py, and the
long (paraphrased) descriptions.

  details-heritage.jsonl  per site (by index in heritage.json, which gains `i`): Wikidata item,
                          short description (English Wikipedia's, else Wikidata's in English,
                          else in the local language), type, style, architect, inception, the
                          Wikipedia article (English, else local) and, where written, a long
                          description with its source article.

Long descriptions: the ones written in the registers' snapshot (data/heritage/desc/*.out.jsonl),
assembled by long_descriptions. Those written since are the NAS's descriptions/, which the server
lays over the records (crates/server/src/descriptions.rs).

usage: heritagedetails.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "wd"
D = ROOT / "data" / "heritage" / "desc"
LOCAL = ["fr", "es", "pt", "ca"]


def best_items() -> dict[int, dict]:
    """Per site index, its Wikidata item: of several (sharing its register ID, or found through
    OSM), one with a Wikipedia article, then the most-linked."""
    out: dict[int, dict] = {}
    rank = lambda r: (bool(r["wiki"]), r["sl"])
    for line in open(W / "items.jsonl", encoding="utf-8"):
        r = json.loads(line)
        cur = out.get(r["i"])
        if cur is None or rank(r) > rank(cur):
            out[r["i"]] = r
    return out


def article(r: dict) -> tuple[str, str] | None:
    w = r.get("wiki", {})
    if "en" in w:
        return "en", w["en"]
    for l in LOCAL:
        if l in w:
            return l, w[l]
    return next(iter(w.items()), None)


def long_descriptions() -> dict[str, dict]:
    """Written descriptions by Wikidata item: the pilot's (batch-*, rewritten fix-*), overridden by
    those in the current voice (v2-*), and those researched from other sources where the article
    didn't do (research-*, with their sources as "src": [{"t": title, "u": url}]; "drop" where
    nothing reliable was found, which removes an earlier one)."""
    files = sorted(D.glob("batch-*.out.jsonl")) + sorted(D.glob("fix-*.out.jsonl"))
    out: dict[str, dict] = {}
    for p in files + sorted(D.glob("v2-*.out.jsonl")) + sorted(D.glob("research-*.out.jsonl")):
        for line in open(p, encoding="utf-8"):
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if r.get("qid") and r.get("drop"):  # researched and found nothing: no description
                out.pop(r["qid"], None)
            elif r.get("qid") and r.get("long"):
                out[r["qid"]] = r
    return out


def assemble() -> None:
    items = best_items()
    sd = json.loads((W / "enwiki-shortdesc.json").read_text()) if (W / "enwiki-shortdesc.json").exists() else {}
    long = long_descriptions()
    ex = {}
    if (D / "extracts.jsonl").exists():
        for line in open(D / "extracts.jsonl", encoding="utf-8"):
            r = json.loads(line)
            ex[r["qid"]] = r
    fc = json.load(open(B / "heritage.json"))
    n = nl = 0
    with open(B / "details-heritage.jsonl", "w", encoding="utf-8") as out:
        for i, f in enumerate(fc["features"]):
            f["properties"]["i"] = i
            r = items.get(i)
            if not r:
                continue
            art = article(r)
            short = (sd.get(r["wiki"].get("en", "")) if "en" in r["wiki"] else None) or r["desc"].get("en") \
                or next((r["desc"][l] for l in LOCAL if l in r["desc"]), None)
            rec = {"i": i, "qid": r["qid"], "sl": r["sl"]}
            if short:
                rec["short"] = short
            for k in ("inception", "inst", "style", "arch"):
                if k in r:
                    rec[k] = r[k]
            if art:
                rec["wiki"] = {"lang": art[0], "title": art[1]}
            lg = long.get(r["qid"])
            if lg:
                src = ex.get(r["qid"], {})
                rec["long"] = lg["long"]
                rec["long_src"] = {"refs": lg["src"]} if lg.get("src") else \
                    {"lang": src.get("lang", art[0] if art else "en"), "title": src.get("title", art[1] if art else "")}
                nl += 1
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
            n += 1
    tmp = B / "heritage.json.tmp"
    tmp.write_text(json.dumps(fc, ensure_ascii=False, separators=(",", ":")))
    tmp.rename(B / "heritage.json")
    print(f"details-heritage.jsonl: {n} of {len(fc['features'])} sites with Wikidata, {nl} with a long description", file=sys.stderr)


if __name__ == "__main__":
    assemble()
