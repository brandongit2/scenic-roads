#!/usr/bin/env python3
"""The one-time conversion of the translation work's area tables (translations/<area>/places-<area>
.jsonl and roads-<area>.jsonl, from place-translations' out/display/, copied 2026-10-02) into lines
by language (docs/plan.md §7): {"n", "kind", "langs", "main", "sub", "via"}.

- Dropped: lines taken from particular things' sources (via "osm": one thing's name:en filed for the
  name, and Japan's romanised kana readings among them) and lines not done (via "todo", "skipped").
- Kept: lines made from the name itself (via "native", "rule:…", "agent:…"). Places' lines hold for
  settlements and other things, roads' for roads; each for the languages spoken in its area (AREA:
  the area tables' readings, so Taiwan's Hanyu Pinyin is Chinese, Hong Kong's romanisation Cantonese).
- Where two areas' lines for a name, kind and language disagree (places in France and in Quebec, in
  mainland Portugal and on its islands, in Britain and North America), a name kept as it is in one
  and translated in the other is split by kind: the kept one is the settlement's (Mont-Blanc, a town
  in Quebec), the translation the other things' (Mont Blanc). Else the language's home area wins
  (HOME). Each choice is logged.

Output: <out>/<language>.jsonl (a line goes in its first language's file; English names of the
languages, so the area tables' loader, which reads area codes in file names, skips them), and
<out>/../conversion-log.jsonl.

usage: convert-tables.py <old translations folder> <out folder>
"""
import collections
import glob
import json
import os
import sys

AREA = {"jp": ["ja"], "tw": ["zh"], "hk": ["yue"], "sg": ["en", "zh", "ms", "ta"], "fr": ["fr"],
        "ib": ["es", "pt", "ca", "gl", "eu"], "pt": ["pt"], "na": ["en", "fr"], "gb": ["en", "cy", "ga", "gd"]}
HOME = {"fr": "fr", "pt": "ib", "en": "gb", "zh": "tw", "es": "ib", "ca": "ib", "gl": "ib", "eu": "ib"}
NAMES = {"ja": "japanese", "zh": "chinese", "yue": "cantonese", "en": "english", "ms": "malay", "ta": "tamil",
         "fr": "french", "es": "spanish", "pt": "portuguese", "ca": "catalan", "gl": "galician", "eu": "basque",
         "cy": "welsh", "ga": "irish", "gd": "scottish-gaelic"}
DROP = {"osm", "todo", "skipped"}


def main(src: str, out: str) -> None:
    os.makedirs(out, exist_ok=True)
    # (n, group, lang) -> {area: (main, sub, via)}; group: place or road.
    by = collections.defaultdict(dict)
    stats = collections.Counter()
    for f in sorted(glob.glob(os.path.join(src, "*", "*.jsonl"))):
        area = os.path.basename(os.path.dirname(f))
        group = "road" if os.path.basename(f).startswith("roads-") else "place"
        for line in open(f, encoding="utf-8"):
            d = json.loads(line)
            via = d.get("via") or ""
            head = via.split(":")[0]
            stats[(group, head if head in DROP else "kept")] += 1
            if head in DROP:
                continue
            main_ = d.get("main") or d["n"]
            for lang in AREA[area]:
                by[(d["n"], group, lang)][area] = (main_, d.get("sub"), via)
    log = []
    # (n, kind, main, sub, via) -> langs
    lines = collections.defaultdict(list)
    for (n, group, lang), areas in by.items():
        kinds = ["road"] if group == "road" else ["settlement", "other"]
        values = {(m, s) for m, s, _ in areas.values()}
        if len(values) == 1:
            m, s, via = next(iter(areas.values()))
            lines[(n, tuple(kinds), m, s, via)].append(lang)
            continue
        kept = {a: v for a, v in areas.items() if v[0] == n and not v[1]}
        moved = {a: v for a, v in areas.items() if a not in kept}
        if group == "place" and kept and moved and len({(m, s) for m, s, _ in moved.values()}) == 1:
            ka, kv = sorted(kept.items())[0]
            ma, mv = sorted(moved.items())[0]
            lines[(n, ("settlement",), kv[0], kv[1], kv[2])].append(lang)
            lines[(n, ("other",), mv[0], mv[1], mv[2])].append(lang)
            log.append({"n": n, "lang": lang, "lines": areas, "chose": f"split: settlement {ka}'s, other {ma}'s"})
        else:
            home = HOME.get(lang)
            pick = home if home in areas else sorted(areas)[0]
            m, s, via = areas[pick]
            lines[(n, tuple(kinds), m, s, via)].append(lang)
            log.append({"n": n, "lang": lang, "lines": areas, "chose": f"{pick}'s (the language's home area)"})
    files = collections.defaultdict(list)
    for (n, kinds, m, s, via), langs in lines.items():
        order = [l for l in NAMES if l in langs]
        files[order[0]].append({"n": n, "kind": kinds[0] if len(kinds) == 1 else list(kinds), "langs": order, "main": m, "sub": s, "via": via})
    for lang, rows in files.items():
        rows.sort(key=lambda r: (r["n"], str(r["kind"])))
        path = os.path.join(out, NAMES[lang] + ".jsonl")
        with open(path + ".tmp", "w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        os.replace(path + ".tmp", path)
        stats[("out", lang)] = len(rows)
    with open(os.path.join(os.path.dirname(out.rstrip("/")), "conversion-log.jsonl"), "w", encoding="utf-8") as f:
        for e in log:
            f.write(json.dumps(e, ensure_ascii=False) + "\n")
    stats[("disagreements", "settled")] = len(log)
    stats[("disagreements", "names")] = len({e["n"] for e in log})
    for k, v in sorted(stats.items(), key=str):
        print(*k, v, sep="\t")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
