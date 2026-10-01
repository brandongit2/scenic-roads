"""English road names from OSM, for the app's text: "明治通り (Meiji-dori)", "中山北路 (Zhongshan North Road)".

Every highway way whose name:en truly differs from its name (names.py differs: not just accents,
case, punctuation or spacing) → data/build/road-en.json, {OSM way id: English}. The server gives
it with the road (name_en: hover, profile, scenic drives) and the app puts it in parentheses after
the name (web/src/english.ts withEnglish). Only OSM's English: romanised names (name:ja-Latn) and
roads without name:en stay as they are; nothing is translated.

usage: roadnames.py   (reads data/osm/merged.osm.pbf: a few minutes, most of it osmium's filter)
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import osmium

from names import differs

ROOT = Path(__file__).resolve().parent.parent
PBF = ROOT / "data/osm/merged.osm.pbf"
WITH_EN = ROOT / "data/names/with-en.osm.pbf"
OUT = ROOT / "data/build/road-en.json"


def main() -> None:
    WITH_EN.parent.mkdir(parents=True, exist_ok=True)
    # The ways with English (a small file, without their nodes), then the roads among them.
    subprocess.run(["osmium", "tags-filter", "--overwrite", "-R", str(PBF), "w/name:en", "-o", str(WITH_EN)], check=True)
    en: dict[str, str] = {}
    for w in osmium.FileProcessor(str(WITH_EN), osmium.osm.WAY):
        if "highway" not in w.tags:
            continue
        name, e = w.tags.get("name"), (w.tags.get("name:en") or "").strip()
        if name and differs(name, e):
            en[str(w.id)] = e
    tmp = OUT.with_suffix(".tmp")
    tmp.write_text(json.dumps(en, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    tmp.replace(OUT)
    print(f"road-en.json: {len(en)} roads with an English name", file=sys.stderr)


if __name__ == "__main__":
    main()
