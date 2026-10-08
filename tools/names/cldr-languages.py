#!/usr/bin/env python3
"""The languages spoken per territory, from CLDR's territoryInfo.json (cldr-json, cldr-core,
supplemental): crates/names/src/territory-languages.tsv, which the names crate compiles in.

A territory's languages are those CLDR marks official or de facto official there, by the share of
people who speak them, largest first; script subtags are dropped (zh_Hant is zh). Languages official
only regionally (Catalan in Spain, Welsh in Britain) are left to the subdivisions' refinements
(crates/names/src/spoken.rs, REFINED), which also set the order where it matters.

usage: cldr-languages.py <territoryInfo.json> > crates/names/src/territory-languages.tsv
"""
import json
import sys

STATUSES = {"official", "de_facto_official"}


def main(path: str) -> None:
    d = json.load(open(path, encoding="utf-8"))
    sup = d["supplemental"]
    info = sup["territoryInfo"]
    print(f"# CLDR {sup['version']['_cldrVersion']} territoryInfo: territory, then its official and de facto official languages, most spoken first")
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
            print(code + "\t" + ",".join(seen))


if __name__ == "__main__":
    main(sys.argv[1])
