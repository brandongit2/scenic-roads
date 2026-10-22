#!/usr/bin/env python3
"""Wikidata's query service and Wikipedia's articles for the Python steps that still ask them
(areadetails.py, items.py, pageviews.py; the heritage chain's first part is pipeline::heritagewd,
which asks the same way in Rust: crate::wikiask). Queries go to the declared `wikidata` source
(docs/inputs.md §6.9: QLever's endpoint) through the job's proxy (curl honours HTTPS_PROXY), a 429
told to it (budget.seen).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import budget
import declared
from timings import phase

UA = "scenic-roads/0.1 (personal offline map)"
PREFIXES = ("PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> "
            "PREFIX schema: <http://schema.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
            "PREFIX wikibase: <http://wikiba.se/ontology#> ")


def sparql(q: str) -> list[dict]:
    """Run a query; on errors retry with growing pauses (longer when rate-limited, HTTP 429)."""
    for attempt in range(10):
        r = subprocess.run(["curl", "-sS", "-m", "180", "-A", UA, "-H", "Accept: application/sparql-results+json",
                            "--data-urlencode", f"query={PREFIXES}{q}", declared.url("wikidata")], capture_output=True)
        try:
            return json.loads(r.stdout)["results"]["bindings"]
        except (json.JSONDecodeError, KeyError):
            if b"429" in r.stdout[:200]:
                budget.seen(declared.url("wikidata"), 429, None)
            time.sleep((45 if b"429" in r.stdout[:200] else 10) * (attempt + 1))
    raise RuntimeError(f"WDQS failed: {r.stdout[:300]!r}")


def write_atomic(p: Path, text: str) -> None:
    """Writes a cache through a temporary file and a rename: a run stopped midway leaves the old
    file, never a cut-short one."""
    tmp = p.with_name(p.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, p)


def val(b: dict, k: str) -> str | None:
    return b[k]["value"] if k in b and b[k]["value"] != "" else None


def wikipedias(qids: list[str], progress=None) -> dict[str, dict]:
    """Per item: the number of Wikipedia articles (any language) and their language|title list.
    (`progress`, when given, is told how many are done after each batch: the items job's progress
    line.)"""
    p = phase("Wikipedia articles looked up", "net").start()
    out: dict[str, dict] = {}
    for k in range(0, len(qids), 1000):
        chunk = qids[k:k + 1000]
        q = f"""SELECT ?item (COUNT(DISTINCT ?art) AS ?n) (GROUP_CONCAT(?lt; SEPARATOR="\\t") AS ?arts) WHERE {{
          VALUES ?item {{ {" ".join(f"wd:{x}" for x in chunk)} }}
          ?art schema:about ?item ; schema:isPartOf ?site ; schema:inLanguage ?lang ; schema:name ?nm .
          FILTER(STRENDS(STR(?site), ".wikipedia.org/"))
          BIND(CONCAT(?lang, "|", ?nm) AS ?lt)
        }} GROUP BY ?item"""
        got = {val(b, "item").rsplit("/", 1)[1]: b for b in sparql(q)}
        for x in chunk:
            b = got.get(x)
            out[x] = {"n": int(val(b, "n") or 0), "arts": (val(b, "arts") or "").split("\t")} if b else {"n": 0, "arts": []}
        if k // 1000 % 20 == 0:
            print(f"  Wikipedia articles: {k + len(chunk)}/{len(qids)}", file=sys.stderr, flush=True)
        if progress:
            progress(k + len(chunk))
    p.end()
    return out
