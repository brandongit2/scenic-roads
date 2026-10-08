#!/usr/bin/env python3
"""The one-time conversion of the translation work's area tables (translations/<area>/places-<area>
.jsonl and roads-<area>.jsonl, from place-translations' out/display/, copied 2026-10-02) into lines
by language (docs/plan.md §7): {"n", "kind", "langs", "main", "sub", "via"}.

- Dropped: lines taken from particular things' sources (via "osm": one thing's name:en filed for the
  name, and Japan's romanised kana readings among them) and lines not done (via "todo", "skipped").
- Kept: lines made from the name itself (via "native", "rule:…", "agent:…"). Places' lines hold for
  settlements and other things, roads' for roads, and each for the other table's kinds too where
  that table has no line for the name in the language (the old lookup's fallback). Each holds for
  the languages spoken in its area (AREA: the area tables' readings, so Taiwan's Hanyu Pinyin is
  Chinese, Hong Kong's romanisation Cantonese), then those its old box reached besides (EXTRA),
  where no area's own line for the language disagrees.
- Where two areas' lines for a name, kind and language disagree (places in France and in Quebec, in
  mainland Portugal and on its islands, in Britain and North America), a name kept as it is in one
  and translated in the other is split by kind: the kept one is the settlement's (Mont-Blanc, a town
  in Quebec), the translation the other things' (Mont Blanc). Else the language's home area wins
  (HOME). Each choice is logged.

Output: <out>/<language>.jsonl (a line goes in its first language's file; English names of the
languages, so the area tables' loader, which reads area codes in file names, skips them), and
<out>/conversion-log.txt (JSON lines, but not .jsonl: no loader reads it).

usage: convert-tables.py <old translations folder> <out folder>
"""
import collections
import glob
import json
import os
import sys

AREA = {"jp": ["ja"], "tw": ["zh"], "hk": ["yue"], "sg": ["en", "zh", "ms", "ta"], "fr": ["fr"],
        "ib": ["es", "pt", "ca", "gl", "eu"], "pt": ["pt"], "na": ["en", "fr"], "gb": ["en", "cy", "ga", "gd"]}
# The languages the old boxes reached besides an area's own: its box held northern Spain (fr),
# the French coast north of 49.8° N (gb), southern Corsica (ib), Shenzhen's edge (hk). Filed after
# the area's own: where another area's line is the language's own, that one wins.
EXTRA = {"fr": ["es"], "gb": ["fr"], "ib": ["fr"], "hk": ["zh"]}
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
            for lang in AREA[area] + EXTRA.get(area, []):
                by[(d["n"], group, lang)][area] = (main_, d.get("sub"), via)
    log = []
    # (n, group, lang) -> [(kinds, main, sub, via, area)]
    resolved = {}
    for (n, group, lang), areas in by.items():
        kinds = ("road",) if group == "road" else ("settlement", "other")
        # Where the language is an area's own and another's extra (the old boxes' reach), its own
        # areas' lines win.
        primary = {a: v for a, v in areas.items() if lang in AREA[a]}
        if primary and len(primary) < len(areas):
            if len({(m, s) for m, s, _ in areas.values()}) > 1:
                log.append({"n": n, "lang": lang, "lines": areas, "chose": f"{', '.join(sorted(primary))}'s (the language is theirs; the others' only by the old boxes' reach)"})
            areas = primary
        values = {(m, s) for m, s, _ in areas.values()}
        if len(values) == 1:
            a, (m, s, via) = sorted(areas.items())[0]
            resolved[(n, group, lang)] = [(kinds, m, s, via, a)]
            continue
        kept = {a: v for a, v in areas.items() if v[0] == n and not v[1]}
        moved = {a: v for a, v in areas.items() if a not in kept}
        if group == "place" and kept and moved and len({(m, s) for m, s, _ in moved.values()}) == 1:
            ka, kv = sorted(kept.items())[0]
            ma, mv = sorted(moved.items())[0]
            resolved[(n, group, lang)] = [(("settlement",), kv[0], kv[1], kv[2], ka), (("other",), mv[0], mv[1], mv[2], ma)]
            log.append({"n": n, "lang": lang, "lines": areas, "chose": f"split: settlement {ka}'s, other {ma}'s"})
        else:
            home = HOME.get(lang)
            pick = home if home in areas else sorted(areas)[0]
            m, s, via = areas[pick]
            resolved[(n, group, lang)] = [(kinds, m, s, via, pick)]
            log.append({"n": n, "lang": lang, "lines": areas, "chose": f"{pick}'s (the language's home area)"})
    # The old lookup fell back to the other table: a road with no roads line read the places line,
    # anything else with no places line the roads line. So a line holds for the other table's kinds
    # too where that table has none (in the split, the home area's line, else the other things').
    lines = collections.defaultdict(list)
    for (n, group, lang), entries in resolved.items():
        other_group = "place" if group == "road" else "road"
        fallback = (n, other_group, lang) not in resolved
        extra = ("settlement", "other") if group == "road" else ("road",)
        home = HOME.get(lang)
        target = next((i for i, e in enumerate(entries) if e[4] == home), len(entries) - 1)
        for i, (kinds, m, s, via, _) in enumerate(entries):
            if fallback and i == target:
                kinds = kinds + extra
                stats[("fallback", group)] += 1
            lines[(n, kinds, m, s, via)].append(lang)
    files = collections.defaultdict(list)
    for (n, kinds, m, s, via), langs in lines.items():
        order = list(dict.fromkeys(langs))
        files[order[0]].append({"n": n, "kind": kinds[0] if len(kinds) == 1 else list(kinds), "langs": order, "main": m, "sub": s, "via": via})
    for lang, rows in files.items():
        rows.sort(key=lambda r: (r["n"], str(r["kind"])))
        path = os.path.join(out, NAMES[lang] + ".jsonl")
        with open(path + ".tmp", "w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        os.replace(path + ".tmp", path)
        stats[("out", lang)] = len(rows)
    with open(os.path.join(out, "conversion-log.txt"), "w", encoding="utf-8") as f:
        for e in log:
            f.write(json.dumps(e, ensure_ascii=False) + "\n")
    stats[("disagreements", "settled")] = len(log)
    stats[("disagreements", "names")] = len({e["n"] for e in log})
    for k, v in sorted(stats.items(), key=str):
        print(*k, v, sep="\t")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
