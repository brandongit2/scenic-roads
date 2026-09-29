#!/usr/bin/env python3
"""Check heritage descriptions against their source extracts (data/heritage/desc/WRITERS.md): runs
of six or more words shared with the extract, and lengths over 55 words.

usage: desccheck.py <batch.jsonl>   (reads <batch>.out.jsonl next to it)
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path


def words(s: str) -> list[str]:
    return re.findall(r"[\w'’-]+", s.lower())


def longest_run(a: list[str], b: list[str]) -> tuple[int, str]:
    best, end = 0, 0
    prev = [0] * (len(b) + 1)
    for i in range(1, len(a) + 1):
        cur = [0] * (len(b) + 1)
        for j in range(1, len(b) + 1):
            if a[i - 1] == b[j - 1]:
                cur[j] = prev[j - 1] + 1
                if cur[j] > best:
                    best, end = cur[j], i
        prev = cur
    return best, " ".join(a[end - best:end])


def main() -> None:
    src = Path(sys.argv[1])
    out = src.with_name(src.name.replace(".jsonl", ".out.jsonl"))
    ex = {json.loads(l)["qid"]: json.loads(l) for l in open(src, encoding="utf-8") if l.strip()}
    n = issues = 0
    for line in open(out, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        n += 1
        text, e = r.get("long", ""), ex.get(r.get("qid"), {})
        run, span = longest_run(words(text), words(e.get("extract", "")))
        nw = len(words(text))
        notes = []
        if run >= 6:
            notes.append(f"{run}-word run from the extract: \"{span}\"")
        if nw > 55:
            notes.append(f"{nw} words")
        if notes:
            issues += 1
            print(f"{r.get('qid')} ({e.get('name', '?')}): " + "; ".join(notes))
    print(f"{n} descriptions checked, {issues} to look at" if issues else f"{n} descriptions checked, nothing to fix")


if __name__ == "__main__":
    main()
