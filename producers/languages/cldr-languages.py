#!/usr/bin/env python3
"""The languages input (docs/inputs.md §6.2): territories.tsv and about.toml for the NAS's
inputs/languages/, from CLDR's territoryInfo.json (cldr-json, cldr-core, supplemental).

A territory's languages are those CLDR marks official or de facto official there, by the share of
people who speak them, largest first; script subtags are dropped (zh_Hant is zh). Languages official
only regionally (Catalan in Spain, Welsh in Britain), and territories whose names are read otherwise
(Hong Kong's and Macau's in Cantonese), are CLDR's list refined: REFINED below, a line of its own per
subdivision (ISO 3166-2) or in place of the territory's, which also sets the order where it matters.

With --base, the refinements are applied to a territories.tsv already made instead of to CLDR's
JSON (its first line's "CLDR <version>" kept): how the input's first version was made from the
table the app compiled in before #136 (crates/names/src/territory-languages.tsv).

The files are written to --out; drop them into inputs/languages/ on the NAS, where the input check
takes them in (scenic inputs).

usage: cldr-languages.py (<territoryInfo.json> | --base <territories.tsv>) --out <folder>
"""
import argparse
import json
import os
import re

STATUSES = {"official", "de_facto_official"}

# Territories and subdivisions whose languages aren't CLDR's territory list as it is: an ISO 3166
# code and its languages in lookup order.
REFINED = {
    # Canada: Quebec French first, New Brunswick both.
    "CA-QC": ["fr", "en"],
    "CA-NB": ["en", "fr"],
    # Spain's co-official languages where they are.
    "ES-CT": ["ca", "es"],
    "ES-IB": ["ca", "es"],
    "ES-VC": ["es", "ca"],
    "ES-GA": ["gl", "es"],
    "ES-PV": ["es", "eu"],
    "ES-NC": ["es", "eu"],
    # Britain: Wales Welsh after English; Scotland Gaelic.
    "GB-WLS": ["en", "cy"],
    "GB-SCT": ["en", "gd"],
    # France: Brittany Breton after French.
    "FR-BRE": ["fr", "br"],
    # Hong Kong's and Macau's names are read in Cantonese (the government's romanisation), not
    # Mandarin's Pinyin, which Chinese lines carry.
    "HK": ["yue", "en"],
    "MO": ["yue", "pt"],
}

ABOUT = """name = "Unicode CLDR {version}, territoryInfo"
what = "Languages by territory"
credit = "Unicode CLDR"
licence = "Unicode-3.0"
licence_url = "https://www.unicode.org/license.txt"
source = "https://github.com/unicode-org/cldr-json"
territory = "world"
notes = "Official and de facto official languages per territory, most spoken first, refined per subdivision (producers/languages/cldr-languages.py)."
"""


def from_cldr(path: str) -> tuple[str, dict[str, list[str]]]:
    d = json.load(open(path, encoding="utf-8"))
    sup = d["supplemental"]
    info = sup["territoryInfo"]
    table = {}
    for code in sorted(info):
        if not (len(code) == 2 and code.isalpha() and code.isupper()):
            continue
        pops = info[code].get("languagePopulation", {})
        langs = []
        for tag, v in pops.items():
            if v.get("_officialStatus") in STATUSES:
                langs.append((-float(v.get("_populationPercent", 0)), tag.split("_")[0]))
        seen = []
        for _, l in sorted(langs):
            if l not in seen:
                seen.append(l)
        if seen:
            table[code] = seen
    return sup["version"]["_cldrVersion"], table


def from_base(path: str) -> tuple[str, dict[str, list[str]]]:
    version = None
    table = {}
    for line in open(path, encoding="utf-8").read().splitlines():
        if line.startswith("#"):
            m = re.search(r"CLDR (\S+)", line)
            if m and version is None:
                version = m.group(1)
            continue
        if "\t" not in line:
            continue
        code, langs = line.split("\t", 1)
        table[code] = langs.split(",")
    if version is None:
        raise SystemExit(f"{path}: no 'CLDR <version>' in its comments")
    return version, table


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("territory_info", nargs="?", help="CLDR's supplemental/territoryInfo.json")
    ap.add_argument("--base", help="a territories.tsv to refine instead")
    ap.add_argument("--out", required=True, help="the folder to write territories.tsv and about.toml to")
    a = ap.parse_args()
    if (a.territory_info is None) == (a.base is None):
        ap.error("give territoryInfo.json or --base, one of them")
    version, table = from_base(a.base) if a.base else from_cldr(a.territory_info)
    table.update(REFINED)
    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "territories.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write(f"# CLDR {version} territoryInfo: territory, then its official and de facto official languages, most spoken first\n")
        f.write("# then the refinements (producers/languages/cldr-languages.py, REFINED)\n")
        for code in sorted(table):
            f.write(code + "\t" + ",".join(table[code]) + "\n")
    with open(os.path.join(a.out, "about.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(ABOUT.format(version=version))


if __name__ == "__main__":
    main()
