#!/usr/bin/env python3
"""Wikidata facts for heritage sites, matched by their register ID, else through OpenStreetMap.

Each site in data/build/heritage.json whose register has a Wikidata property (NHLE P1216, Mérimée
P380, HES P709, NRHP P649, Cadw P1459/P3007, DGPC P1702, CRHP P477, RPCQ P633, IPAC P1600, UNESCO
P757, Irish SMR P4057, Japan's national cultural properties P4275, Taiwan's NCHDB P6890; Parks Canada DFHD links to the item itself) is looked up on the Wikidata
Query Service in batches: descriptions (en, fr, es, pt, ca), Wikipedia article titles in those
languages, the number of Wikipedia/Wikimedia sitelinks (a notability measure), inception, and the
labels of its type (P31), architectural style (P149) and architect (P84). English Wikipedia's
short descriptions ("Cathedral in Salisbury, England") are then fetched for the English titles.

Queries go to QLever's Wikidata endpoint (qlever.dev, University of Freiburg): the Wikidata Query
Service was rate-limiting to one request a minute during an outage (2026-09-28).

Sites with no item, or whose register item has no Wikipedia article, are then linked through
OpenStreetMap: an OSM feature tagged with a Wikidata item (data/heritage/osm/named.geojsonseq)
at the site (its outline within 40 m) that carries the same register ID (ref:mhs, HE_ref, ref:nrhp
…) or clearly the same name (three quarters of the two names' words shared). Registers with no Wikidata property (Ontario, Andalusia, Northern
Ireland, Castilla y León …) are linked this way only, and the item carrying a register ID is often
not the one with the article.

Every item's Wikipedia articles in any language are counted (`wpn`); an item with none in the
languages above links its article in another language (Welsh, Galician, Chinese …).

Writes data/heritage/wd/items.jsonl (one line per matched site: its index in heritage.json and
the facts) and data/heritage/wd/enwiki-shortdesc.json. Results are cached per register ID
(ids.jsonl), per item (wp.jsonl), so reruns only look up what is new.

usage: heritagewd.py
"""
from __future__ import annotations

import json
import os
import math
import re
import subprocess
import sys
import time
import unicodedata
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
W = ROOT / "data" / "heritage" / "wd"
UA = "road-elevations/0.1 (personal offline map)"
WDQS = "https://qlever.dev/api/wikidata"
PREFIXES = ("PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> "
            "PREFIX schema: <http://schema.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
            "PREFIX wikibase: <http://wikiba.se/ontology#> ")
LANGS = ["en", "fr", "es", "pt", "ca"]
BATCH = 300

# (property, pattern on the site's url) — the first group is the register ID.
RULES = [
    ("P1216", r"historicengland\.org\.uk/listing/the-list/list-entry/(\d+)"),
    ("P380", r"pop\.culture\.gouv\.fr/notice/merimee/(\w+)"),
    ("P709", r"portal\.historicenvironment\.scot/designation/(\w+)"),
    ("P1459", r"cadwpublic-api\.azurewebsites\.net/reports/listedbuilding/FullReport\?lang=en&id=(\d+)"),
    ("P3007", r"cadwpublic-api\.azurewebsites\.net/reports/sam/FullReport\?lang=en&id=(\w+)"),
    ("P1702", r"patrimoniocultural\.pt/.*/view/(\d+)"),
    ("P477", r"historicplaces\.ca/en/rep-reg/place-lieu\.aspx\?id=(\d+)"),
    ("P633", r"patrimoine-culturel\.gouv\.qc\.ca/rpcq/detail\.do\?methode=consulter&id=(\d+)"),
    ("P1600", r"invarquit\.cultura\.gencat\.cat/card/(\d+)"),
    ("P757", r"whc\.unesco\.org/en/list/(\d+)"),
    ("P4275", r"kunishitei\.bunka\.go\.jp/heritage/detail/(\d+/\d+)"),
    ("P6890", r"nchdb\.boch\.gov\.tw/assets/advanceSearch/(\w+/\d+)"),
    ("P4057", r"query=[^&]*?%2CSMRS%2C([A-Z]{2}\d{3}-\d{3}(?:\d{3})?)"),
    ("QID", r"wikidata\.org/wiki/(Q\d+)"),
]


def sparql(q: str) -> list[dict]:
    """Run a query; on errors retry with growing pauses (longer when rate-limited, HTTP 429)."""
    for attempt in range(10):
        r = subprocess.run(["curl", "-sS", "-m", "180", "-A", UA, "-H", "Accept: application/sparql-results+json",
                            "--data-urlencode", f"query={PREFIXES}{q}", WDQS], capture_output=True)
        try:
            return json.loads(r.stdout)["results"]["bindings"]
        except (json.JSONDecodeError, KeyError):
            time.sleep((45 if b"429" in r.stdout[:200] else 10) * (attempt + 1))
    raise RuntimeError(f"WDQS failed: {r.stdout[:300]!r}")


def query(prop: str, ids: list[str]) -> list[dict]:
    values = " ".join(f'"{i}"' if prop != "QID" else f"wd:{i}" for i in ids)
    bind = f"VALUES ?id {{ {values} }} ?item wdt:{prop} ?id ." if prop != "QID" else f"VALUES ?item {{ {values} }} BIND(STRAFTER(STR(?item), 'entity/') AS ?id)"
    opt = "\n".join(
        f'OPTIONAL {{ ?item schema:description ?dd_{l} FILTER(LANG(?dd_{l}) = "{l}") }}\n'
        f'OPTIONAL {{ ?art_{l} schema:about ?item ; schema:isPartOf <https://{l}.wikipedia.org/> ; schema:name ?ww_{l} }}'
        for l in LANGS)
    q = f"""SELECT ?id ?item (SAMPLE(?sl0) AS ?sl) (SAMPLE(?inc0) AS ?inc)
      {" ".join(f"(SAMPLE(?dd_{l}) AS ?d_{l}) (SAMPLE(?ww_{l}) AS ?w_{l})" for l in LANGS)}
      (GROUP_CONCAT(DISTINCT ?instL; SEPARATOR="|") AS ?inst)
      (GROUP_CONCAT(DISTINCT ?styleL; SEPARATOR="|") AS ?style)
      (GROUP_CONCAT(DISTINCT ?archL; SEPARATOR="|") AS ?arch) WHERE {{
      {bind}
      OPTIONAL {{ ?item wikibase:sitelinks ?sl0 }}
      OPTIONAL {{ ?item wdt:P571 ?inc0 }}
      {opt}
      OPTIONAL {{ ?item wdt:P31 ?i . ?i rdfs:label ?instL FILTER(LANG(?instL) = "en") }}
      OPTIONAL {{ ?item wdt:P149 ?s . ?s rdfs:label ?styleL FILTER(LANG(?styleL) = "en") }}
      OPTIONAL {{ ?item wdt:P84 ?a . ?a rdfs:label ?archL FILTER(LANG(?archL) = "en") }}
    }} GROUP BY ?id ?item"""
    return sparql(q)


def write_atomic(p: Path, text: str) -> None:
    """Writes a cache through a temporary file and a rename: a run stopped midway leaves the old
    file, never a cut-short one."""
    tmp = p.with_name(p.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, p)


def val(b: dict, k: str) -> str | None:
    return b[k]["value"] if k in b and b[k]["value"] != "" else None


def shortdescs(titles: list[str]) -> dict[str, str]:
    """English Wikipedia short descriptions, 50 titles a request: every title asked for is in the
    answer ("" when it has none). A batch that can't be fetched (an HTTP error, an error answer,
    no answer after five tries) fails the run instead of leaving titles out."""
    out: dict[str, str] = {}
    for i in range(0, len(titles), 50):
        chunk = titles[i:i + 50]
        url = "https://en.wikipedia.org/w/api.php?" + urllib.parse.urlencode(
            {"action": "query", "format": "json", "formatversion": 2, "prop": "pageprops", "ppprop": "wikibase-shortdesc",
             "redirects": 1, "titles": "|".join(chunk)})
        d, last = None, ""
        for attempt in range(5):
            r = subprocess.run(["curl", "-sS", "--fail", "-m", "60", "-A", UA, url], capture_output=True)
            try:
                if r.returncode != 0:
                    raise ValueError(f"curl exit {r.returncode}: {r.stderr.decode(errors='replace').strip()}")
                d = json.loads(r.stdout)
                if "error" in d or "query" not in d:
                    raise ValueError(f"answer without a query: {str(d)[:200]}")
                break
            except (ValueError, json.JSONDecodeError) as e:
                d, last = None, str(e)
                time.sleep(5 * (attempt + 1))
        if d is None:
            raise RuntimeError(f"short descriptions: titles {i}–{i + len(chunk)} failed five times ({last})")
        out.update((t, "") for t in chunk)
        back = {n["to"]: n["from"] for n in d.get("query", {}).get("normalized", []) + d.get("query", {}).get("redirects", [])}
        for p in d.get("query", {}).get("pages", []):
            sd = p.get("pageprops", {}).get("wikibase-shortdesc")
            if sd:
                t = p["title"]
                while t in back:
                    out[t] = sd
                    t = back[t]
                out[t] = sd
        if i // 50 % 20 == 0:
            print(f"  short descriptions: {i + len(chunk)}/{len(titles)}", file=sys.stderr, flush=True)
    return out


# ---- linking through OpenStreetMap -----------------------------------------------------------

# OSM tags holding a register's IDs (by the site's Wikidata property).
OSM_REF = {"P380": ("ref:mhs",), "P1216": ("HE_ref", "ref:GB:nhle"), "P649": ("ref:nrhp",), "P4057": ("ref:IE:smr",),
           "P1702": ("ref:dgpc",), "P709": ("ref:hs", "ref:GB:hs")}
STOP = {"the", "of", "and", "de", "du", "des", "la", "le", "les", "l", "d", "et", "y", "del", "dels", "da", "do", "das", "dos",
        "e", "a", "an", "en", "el", "els", "i", "o", "at", "in", "on", "au", "aux", "sur", "near", "former", "ancien", "ancienne"}
NAME_KEYS = ("name", "name:en", "name:fr", "name:es", "name:pt", "name:ca", "official_name", "alt_name", "old_name", "loc_name")


def tokens(s: str) -> set[str]:
    s = unicodedata.normalize("NFKD", s.lower().replace("’", "'").replace("'s ", " ").replace("'s", ""))
    s = "".join(c for c in s if not unicodedata.combining(c))
    out = set()
    for t in re.findall(r"[a-z0-9]+", s):
        t = {"st": "saint", "ste": "sainte", "sta": "santa", "sto": "santo", "mt": "mount", "ch": "church"}.get(t, t)
        if t not in STOP:
            out.add(t)
    return out


def same_name(a: set[str], b: set[str]) -> float:
    """Words shared by two names, of all their words (0 unless a word of 4+ letters is shared). Both
    ways, so a part of a place ("Magdalen College, Kitchen") doesn't take the whole's item."""
    common = a & b
    if not common or not any(len(t) >= 4 and not t.isdigit() for t in common):
        return 0.0
    return len(common) / len(a | b)


def osm_index() -> tuple[dict, list]:
    """OSM features with a Wikidata item: grid of ~1 km cells → feature indices; features as
    (qid, bbox, name tokens, register refs)."""
    feats, grid = [], {}
    with open(ROOT / "data" / "heritage" / "osm" / "named.geojsonseq", encoding="utf-8") as f:
        for line in f:
            if '"wikidata"' not in line:
                continue
            g = json.loads(line.strip().lstrip("\x1e"))
            p = g["properties"]
            qid = p.get("wikidata", "").split(";")[0].strip()
            if not re.fullmatch(r"Q\d+", qid):
                continue
            xs, ys = [], []

            def walk(c):
                if isinstance(c[0], (int, float)):
                    xs.append(c[0]); ys.append(c[1])
                else:
                    for x in c:
                        walk(x)
            walk(g["geometry"]["coordinates"])
            names = set()
            for k in NAME_KEYS:
                if p.get(k):
                    names |= {frozenset(tokens(v)) for v in p[k].split(";")}
            refs = {p[k].strip().upper() for keys in OSM_REF.values() for k in keys if p.get(k)}
            n = len(feats)
            feats.append((qid, (min(xs), min(ys), max(xs), max(ys)), [x for x in names if x], refs))
            for cx in range(int(min(xs) * 100) - 1, int(max(xs) * 100) + 2):
                for cy in range(int(min(ys) * 100) - 1, int(max(ys) * 100) + 2):
                    grid.setdefault((cx, cy), []).append(n)
    return grid, feats


def osm_link(site: dict, rid: str | None, grid: dict, feats: list) -> str | None:
    """The Wikidata item of the OSM feature at a site: same register ID, else clearly the same name."""
    lon, lat = site["geometry"]["coordinates"]
    pad_x, pad_y = 40 / (111320 * math.cos(math.radians(lat))), 40 / 110540
    p = site["properties"]
    names = [t for t in (tokens(p.get("name") or ""), tokens(p.get("name_en") or "")) if t]
    best, key = None, None
    for n in grid.get((int(lon * 100), int(lat * 100)), []):
        qid, (x0, y0, x1, y1), onames, refs = feats[n]
        if not (x0 - pad_x <= lon <= x1 + pad_x and y0 - pad_y <= lat <= y1 + pad_y):
            continue
        idm = bool(rid and rid.upper() in refs)
        sim = max((same_name(a, b) for a in names for b in onames), default=0.0)
        if not idm and sim < 0.75:
            continue
        k = (idm, sim, -((x1 - x0) * (y1 - y0)))
        if key is None or k > key:
            best, key = qid, k
    return best


def wikipedias(qids: list[str]) -> dict[str, dict]:
    """Per item: the number of Wikipedia articles (any language) and their language|title list."""
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
    return out


# Languages to link when an item has no article in LANGS, most relevant to the map's regions first.
OTHER = ["cy", "ga", "gd", "gl", "eu", "ast", "an", "oc", "br", "co", "zh", "zh-yue", "de", "it", "nl", "ru", "pl", "ja"]


def other_article(arts: list[str]) -> tuple[str, str] | None:
    have = dict(a.split("|", 1) for a in arts if "|" in a)
    for l in OTHER:
        if l in have:
            return l, have[l]
    return min(have.items()) if have else None


def main():
    W.mkdir(parents=True, exist_ok=True)
    feats = json.load(open(B / "heritage.json"))["features"]
    # NRHP sites carry the National Archives link; their reference numbers come from the NPS layer.
    nara = {}
    nr = ROOT / "data" / "heritage" / "nrhp.json"
    if nr.exists():
        for f in json.load(open(nr))["features"]:
            p = f["properties"]
            if p.get("NARA_URL") and p.get("NRIS_Refnum"):
                nara[p["NARA_URL"]] = p["NRIS_Refnum"]
    by_prop: dict[str, dict[str, list[int]]] = {}
    for i, f in enumerate(feats):
        url = f["properties"].get("url") or ""
        if url in nara:
            by_prop.setdefault("P649", {}).setdefault(nara[url], []).append(i)
            continue
        m = re.search(r"npgallery\.nps\.gov/AssetDetail/NRIS/(\d+)", url)
        if m:
            by_prop.setdefault("P649", {}).setdefault(m.group(1), []).append(i)
            continue
        for prop, pat in RULES:
            m = re.search(pat, url)
            if m:
                by_prop.setdefault(prop, {}).setdefault(m.group(1), []).append(i)
                break
    print({p: len(v) for p, v in by_prop.items()}, file=sys.stderr)

    # Results per register ID (data/heritage/wd/ids.jsonl: prop, id, result rows, [] = no item),
    # so a rerun only queries IDs it hasn't seen (new regions, new register entries).
    id_cache = W / "ids.jsonl"
    seen: dict[tuple[str, str], list] = {}
    if id_cache.exists():
        for line in open(id_cache, encoding="utf-8"):
            r = json.loads(line)
            seen[(r["prop"], r["id"])] = r["rows"]
    else:
        # From the batch files of earlier runs (batch n of each property's sorted IDs).
        for prop, ids in by_prop.items():
            keys = sorted(ids)
            for k in range(0, len(keys), BATCH):
                f = W / f"{prop}-{k // BATCH:04d}.json"
                if f.exists():
                    rows = json.loads(f.read_text())
                    got: dict[str, list] = {}
                    for b in rows:
                        got.setdefault(val(b, "id"), []).append(b)
                    for key in keys[k:k + BATCH]:
                        seen[(prop, key)] = got.get(key, [])
    jobs = []
    for prop, ids in by_prop.items():
        keys = sorted(k for k in ids if (prop, k) not in seen)
        for k in range(0, len(keys), BATCH):
            jobs.append((prop, keys[k:k + BATCH]))
    print(f"{sum(len(v) for v in by_prop.values())} register IDs, {sum(len(j[1]) for j in jobs)} to look up", file=sys.stderr)

    def run(job):
        prop, keys = job
        rows = query(prop, keys)
        got: dict[str, list] = {}
        for b in rows:
            got.setdefault(val(b, "id"), []).append(b)
        return prop, {k: got.get(k, []) for k in keys}

    with ThreadPoolExecutor(2) as ex:
        for n, (prop, res) in enumerate(ex.map(run, jobs)):
            for k, rows in res.items():
                seen[(prop, k)] = rows
            if n % 25 == 0:
                print(f"  batches {n + 1}/{len(jobs)}", file=sys.stderr, flush=True)
    write_atomic(id_cache, "".join(json.dumps({"prop": prop, "id": k, "rows": rows}, ensure_ascii=False) + "\n" for (prop, k), rows in seen.items()))

    def record(b: dict) -> dict:
        rec = {
            "qid": val(b, "item").rsplit("/", 1)[1],
            "sl": int(val(b, "sl") or 0),
            "desc": {l: val(b, f"d_{l}") for l in LANGS if val(b, f"d_{l}")},
            "wiki": {l: val(b, f"w_{l}") for l in LANGS if val(b, f"w_{l}")},
        }
        if val(b, "inc"):
            rec["inception"] = val(b, "inc")[:10]
        for k in ("inst", "style", "arch"):
            if val(b, k):
                rec[k] = val(b, k).split("|")
        return rec

    items = []
    rid_of: dict[int, tuple[str, str]] = {}
    for prop, ids in by_prop.items():
        for rid, sites in ids.items():
            recs = [record(b) for b in seen.get((prop, rid), [])]
            for i in sites:
                rid_of[i] = (prop, rid)
                items.extend({"i": i, "prop": prop, "id": rid, **r} for r in recs)

    # Sites with no item, or none with an article in LANGS: the Wikidata item of the OSM feature there.
    linked = {r["i"] for r in items if r["wiki"]}
    todo = [i for i in range(len(feats)) if i not in linked]
    print(f"OSM links: indexing features with a Wikidata item; {len(todo)} sites to link", file=sys.stderr, flush=True)
    grid, ofeats = osm_index()
    have_q = {(r["i"], r["qid"]) for r in items}
    osm_sites: dict[str, list[int]] = {}
    for i in todo:
        prop, rid = rid_of.get(i, (None, None))
        q = osm_link(feats[i], rid if prop in OSM_REF else None, grid, ofeats)
        if q and (i, q) not in have_q:
            osm_sites.setdefault(q, []).append(i)
    new_q = sorted(q for q in osm_sites if ("QID", q) not in seen)
    print(f"OSM links: {sum(len(v) for v in osm_sites.values())} sites to {len(osm_sites)} items ({len(new_q)} to look up)", file=sys.stderr, flush=True)
    with ThreadPoolExecutor(2) as ex:
        for prop, res in ex.map(run, [("QID", new_q[k:k + BATCH]) for k in range(0, len(new_q), BATCH)]):
            for k, rows in res.items():
                seen[(prop, k)] = rows
    write_atomic(id_cache, "".join(json.dumps({"prop": prop, "id": k, "rows": rows}, ensure_ascii=False) + "\n" for (prop, k), rows in seen.items()))
    for q, sites in osm_sites.items():
        for b in seen.get(("QID", q), []):
            r = record(b)
            items.extend({"i": i, "prop": "OSM", "id": q, **r} for i in sites)

    # Wikipedia articles in any language, per item (data/heritage/wd/wp.jsonl).
    wp_cache = W / "wp.jsonl"
    wp: dict[str, dict] = {}
    if wp_cache.exists():
        for line in open(wp_cache, encoding="utf-8"):
            r = json.loads(line)
            wp[r["qid"]] = r
    need = sorted({r["qid"] for r in items} - set(wp))
    print(f"Wikipedia articles: {len(need)} items to look up", file=sys.stderr, flush=True)
    for q, r in wikipedias(need).items():
        wp[q] = {"qid": q, **r}
    write_atomic(wp_cache, "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in wp.values()))
    for r in items:
        w = wp.get(r["qid"], {})
        r["wpn"] = w.get("n", 0)
        if not r["wiki"]:
            other = other_article(w.get("arts", []))
            if other:
                r["wiki"][other[0]] = other[1]
    items.sort(key=lambda r: r["i"])
    print(f"{len({r['i'] for r in items if r['wiki']})} of {len(feats)} sites with a Wikipedia article", file=sys.stderr)
    write_atomic(W / "items.jsonl", "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in items))
    titles = sorted({r["wiki"]["en"] for r in items if "en" in r["wiki"]})
    sd_path = W / "enwiki-shortdesc.json"
    have = json.loads(sd_path.read_text()) if sd_path.exists() else {}
    todo = [t for t in titles if t not in have]
    print(f"{len(items)} sites matched to Wikidata; {len(titles)} English articles ({len(todo)} short descriptions to fetch)", file=sys.stderr)
    # (Titles with no description are "": not asked again.)
    have.update(shortdescs(todo))
    write_atomic(sd_path, json.dumps(have, ensure_ascii=False))
    print(f"done: {len(have)} short descriptions", file=sys.stderr)


if __name__ == "__main__":
    main()
