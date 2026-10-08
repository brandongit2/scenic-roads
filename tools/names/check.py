"""Checker for a file of translation answers (translations/todo/README.md): one valid line per
name, and none of the mistakes translators were caught making. Standalone (Python 3.10+, standard
library only). Adapted from the place-translations work's tools/check.py: the same rules, on the
lines by language.

usage: python3 check.py <answers>.jsonl [<todo list>.jsonl] [--all]

With the list (translations/todo/<language>.jsonl), each answer's name, kind and languages must be
one of its entries'. Prints the problems (one per line) or "N lines, all good"; exit status 1 when
there are problems.

The rules, and the failure each one was added for:
  format     one {"n", "kind", "langs", "main", "sub"} JSON line per name
  romanise   no CJK or kana left in the English
  same       English that's the name itself (case, accents, punctuation aside) must be null
  half       a translation that kept the original's articles/prepositions ("Lake de la Point")
  order      the original's word order ("Lake Long" for Lac Long)
  saint      a church without "of" ("Church Saint-Martin")
  macron     ja: Hepburn without macrons (Tokyo, Ryukyu)
  kunrei     ja: Kunrei/Nihon-shiki spellings (Otubo, Isikawa, Todurasawa) or the "stu" typo
  accents    accents dropped from names kept as written (Thérèse → Therese)
  saint-name a saint's name translated (Saint-Pierre → Saint Peter)
  lowercase  English starting with a lowercase word, usually a leftover article ("la Serré Waterfall")
  mount      "Laval Mount": English puts Mount first (Mount Laval)
  lake-adj   "Lake Upper Rowan": a describing word goes before the generic (Upper Rowan Lake)
  nulls      Latin-script answers (not Welsh, Irish or Gaelic) with more than a fifth of the names
             that aren't settlements null
"""
from __future__ import annotations

import json
import re
import sys
import unicodedata
from pathlib import Path


def latin(s: str) -> bool:
    """Mostly Latin letters (accents allowed)."""
    lat = other = 0
    for c in s:
        if c.isalpha():
            if "LATIN" in unicodedata.name(c, ""):
                lat += 1
            else:
                other += 1
    return lat >= other


def norm(s: str) -> str:
    s = unicodedata.normalize("NFKD", s.lower())
    return re.sub(r"[\W_]+", "", "".join(c for c in s if not unicodedata.combining(c)))


def differs(name: str, en: str | None) -> bool:
    """English that is more than the name again (accents, case, punctuation, spacing aside)."""
    return bool(en) and norm(en) != norm(name)


def strip_accents(w: str) -> str:
    return "".join(c for c in unicodedata.normalize("NFKD", w) if not unicodedata.combining(c))


# Latin names worth translating have one of these (French, Spanish, Portuguese, Catalan, Welsh, Irish
# and Scottish Gaelic generic words); English ones (Lake, Fort, Reserve) are left out on purpose.
GENERIC = re.compile(r"(^|[\s\-'’])(" + "|".join("""
lac lacs rivière riviere fleuve ruisseau étang etang baie anse cap pointe île ile îles iles îlot ilot mont monts montagne
montagnes col pic forêt foret parc réserve reserve chute chutes cascade lagune grotte gorge gorges vallée vallee plage
marais château chateau église eglise cathédrale cathedrale abbaye basilique chapelle pont gare musée musee tour phare
moulin
río rio lago laguna embalse sierra monte montes isla islas bahía bahia cabo punta playa parque reserva pico puerto cueva
castillo iglesia catedral monasterio ermita puente estación estacion museo torre faro molino fuente barranco valle
lagoa serra ilha ilhéu ilheu baía praia gruta castelo igreja sé mosteiro convento capela ponte estação estacao museu
farol moinho ribeira vale
riu llac estany muntanya illa badia platja castell església esglesia estació
afon llyn mynydd coed eglwys
sliabh abhainn inis oileán oilean caisleán caislean teampall""".split()) + r")([\s\-'’]|$)", re.I)

_GEN = ("lake|lakes|river|pond|stream|brook|creek|mill|castle|church|chapel|mount|mountain|mountains|pass|wood|woods|"
        "forest|park|bay|island|islands|point|cape|beach|valley|waterfall|falls|station|square|hamlet|tower|bridge|"
        "spring|fountain|abbey|priory|cathedral|basilica|museum|reserve|peak|hill|cave|gorge|marsh|marshes|lagoon|"
        "reservoir|harbour|port|canal|meadow|field|fields|moor|rock|house|manor|farm|monastery|convent")
_ROM = r"de|du|des|d'|d’|la|le|les|l'|l’|à|au|aux|del|dels|da|do|dos|das|el|los|las|y|et|en|di|della"
HALF = re.compile(rf"^({_GEN})\s+({_ROM})(\s|$)|\s({_ROM})\s*(la\s+|le\s+|les\s+|l'|l’)?({_GEN}|clear|black|white|green|red)$", re.I)
_ADJ = (r"long|round|black|green|white|red|blue|yellow|grey|gray|little|big|great|clear|deep|crooked|lost|"
        r"beaver|trout|grand|small|upper|lower|north|south|east|west|old|new|high|low|dry|cold|hot|dead|bear|"
        r"wolf|fox|duck|swan|eagle|pike|pine|pines|birch|cedar|maple|oak|stone|sand|mud|narrow|wide|hidden|"
        r"salmon|moose|caribou|otter|loon|heron|rat|rats|spruce|castor|vert|noir|rond|blanc|rouge|perdu")
ORDER = re.compile(rf"^(lake|river|pond|brook|creek|stream|mount|hill|island|bay|castle|mill|wood|forest|pass)\s+({_ADJ})$", re.I)
LAKE_ADJ = re.compile(r"^(Lake|River|Pond|Brook|Creek|Stream)\s+(Upper|Lower|Little|Big|Great|Grand|North|South|East|West|"
                      r"Old|New|Middle|Long|Small|Petit|Petite|Upper)\s+\S")
SAINT = re.compile(r"^(church|chapel|cathedral|basilica|abbey|priory|castle|monastery|convent|hermitage)\s+"
                   r"(saint|sainte|san|santa|santo|são|sant|st\.?|notre)\b", re.I)
MOUNT_LAST = re.compile(r"\S\s+Mount$")
MACRON = re.compile("[āēīōūâêîôûĀĒĪŌŪÂÊÎÔÛ]")
# Romanised Japanese words: ones that split wholly into syllables. Hepburn's, and the Kunrei-shiki /
# Nihon-shiki ones it replaces (si ti tu hu zi di du …): a word that splits only with those is
# Kunrei (Otubo, Isikawa, Hukuoka, Kozima, Todurasawa); English words don't split at all (Studio,
# Medical, Shrine). Doubled consonants (Hokkaido, Matsuyama's "tt"), "tch" and n' are folded first.
_HEP = ("kya kyu kyo gya gyu gyo sha shi shu she sho cha chi chu che cho tsu nya nyu nyo hya hyu hyo bya byu byo "
        "pya pyu pyo mya myu myo rya ryu ryo ja ji ju je jo fa fi fu fe fo "
        "ka ki ku ke ko ga gi gu ge go sa su se so za zu ze zo ta te to da de do na ni nu ne no ha hi he ho "
        "ba bi bu be bo pa pi pu pe po ma mi mu me mo ya yu yo ra ri ru re ro wa wo va vi vu ve vo a i u e o n").split()
_KUN = "sya syu syo zya zyu zyo tya tyu tyo dya dyu dyo si zi ti tu hu di du".split()
_HEP_RE = re.compile("(?:" + "|".join(sorted(_HEP, key=len, reverse=True)) + ")+")
_ANY_RE = re.compile("(?:" + "|".join(sorted(_HEP + _KUN, key=len, reverse=True)) + ")+")


def _fold(w: str) -> str:
    w = w.lower().replace("'", "").replace("tch", "ch")
    return re.sub(r"([bcdfghjkmprstz])\1", r"\1", w)


def kunrei(word: str) -> str | None:
    """'kunrei' when a romanised word needs Kunrei/Nihon-shiki syllables, 'typo' for "stu"
    (Nakakomastu for Nakakomatsu), else None."""
    w = _fold(word)
    if _HEP_RE.fullmatch(w):
        return None
    if _ANY_RE.fullmatch(w):
        return "kunrei"
    if "stu" in w and _HEP_RE.fullmatch(w.replace("stu", "tsu")):
        return "typo"
    return None
ENGLISH_WORDS = set(_GEN.split("|")) | set("""
line lines shrine temple district village town city ward street road route trail observatory viewpoint observation
deck platform garden gardens hall gate school university institute site ruins tomb tombs mound dam tunnel spring
springs hot plateau ridge wetland lighthouse memorial monument building market center centre national prefectural
quasi natural historic history old new upper lower east west north south central first second third main branch
drain channel ditch pool lagoon estuary strait sea ocean inlet cove sound headland peninsula summit highland
highlands shopping station stations street avenue sanctuary cemetery pagoda residence house district's paradise""".split())
SAINT_OUT = re.compile(r"\b(?:Saint|St\.?)\s+([A-Z][\w’']+)")
SAINT_IN = re.compile(r"\b(?:Saint|Sainte|San|Santa|Santo|São|Sant|Sankt|St)[\s\-]+([^\W\d_][\w’']*)", re.I)
ESTABLISHED = {"lawrence", "john", "quebec", "montreal"}


def problem(name: str, en: str | None, region: str) -> str | None:
    """What's wrong with one line's English, or None. (Format, nulls and drift are batch-level.)"""
    en = (en or "").strip()
    if not en:
        return None if latin(name) else f"{name!r} needs an English name (it isn't in Latin script)"
    if not latin(en) or any(unicodedata.name(c, "").startswith(("CJK", "HIRAGANA", "KATAKANA")) for c in en):
        return f"{en!r} still has characters to romanise"
    if not differs(name, en):
        return f"{en!r} is the name itself: write null"
    if HALF.search(en):
        return f"{en!r} is half translated: put it all in English word order ({name!r})"
    if ORDER.search(en) or LAKE_ADJ.search(en):
        return f"{en!r} is in the original's word order ({name!r}): English puts the describing word first"
    if SAINT.search(en):
        return f"{en!r}: write \"Church of Saint-…\" (or \"St …'s Church\")"
    if en[0].islower():
        return f"{en!r} starts with a lowercase word: drop leftover articles, title case (Cascade de la Serré: Serré Waterfall)"
    if MOUNT_LAST.search(en):
        return f"{en!r}: English puts Mount first (Mont Laval: Mount Laval)"
    if region == "jp" and MACRON.search(en):
        return f"{en!r}: no macrons (Hepburn without them: Tokyo, Ryukyu)"
    if region == "jp":
        k = next((x for w in re.findall(r"[A-Za-z']+", en) if w.lower() not in ENGLISH_WORDS and (x := kunrei(w))), None)
        if k == "kunrei":
            return f"{en!r}: Hepburn spellings, please (tsu, shi, chi, fu, ji, zu: Otsubo, Ishikawa)"
        if k == "typo":
            return f"{en!r}: \"stu\" is a typo for \"tsu\" (Nakakomatsu)"
    if latin(name):
        lost = [w for w in re.findall(r"[^\W\d_]+", name)[1:] if strip_accents(w) != w and not GENERIC.search(w)
                and re.search(rf"\b{re.escape(strip_accents(w))}\b", en) and strip_accents(w).lower() not in ESTABLISHED]
        if lost:
            return f"{en!r}: keep the accents in names as written ({', '.join(lost)})"
        new = [m for m in SAINT_OUT.findall(en)
               if strip_accents(m).lower() not in {strip_accents(x).lower() for x in SAINT_IN.findall(name)}
               and m.lower() not in ESTABLISHED]
        if new:
            return f"{en!r}: keep the saint's name as written ({name!r}: Saint-Pierre, not Saint Peter)"
    return None


def _words(s: str) -> set[str]:
    return set(re.findall(r"[a-z]{4,}", strip_accents(s.lower())))


def drift(names: list[str], ens: list[str | None]) -> list[str]:
    """Stretches of 100 lines whose English belongs to the names a few lines away (a translator that
    lost its place): Latin-script names keep their proper part, so an English line shares a word
    with its own name far more often than with a neighbour's."""
    bad = []
    for w in range(0, len(names), 100):
        idx = [i for i in range(w, min(w + 100, len(names))) if ens[i] and latin(names[i])]
        hits = {k: sum(1 for i in idx if 0 <= i + k < len(names) and _words(ens[i]) & _words(names[i + k]))
                for k in range(-5, 6)}
        k = max(hits, key=hits.get)
        if k and hits[k] >= 5 and hits[k] > 2 * hits[0]:
            bad.append(f"lines {w + 1}–{w + 100}: the English is {abs(k)} line{'s' * (abs(k) > 1)} "
                       f"{'ahead of' if k > 0 else 'behind'} its names (redo them, each on its own name's line)")
    return bad


KINDS = {"road", "settlement", "other"}


def check(path: str, todo: str | None, show_all: bool = False) -> int:
    bad, rows = [], []
    for i, line in enumerate(open(path, encoding="utf-8"), 1):
        try:
            x = json.loads(line)
            kinds = x["kind"] if isinstance(x.get("kind"), list) else [x.get("kind")]
            langs = x["langs"] if isinstance(x.get("langs"), list) else [x.get("langs")]
            assert isinstance(x.get("n"), str) and x["n"]
            assert kinds and all(k in KINDS for k in kinds), "kind"
            assert langs and all(isinstance(l, str) and 2 <= len(l.split("_")[0].split("-")[0]) <= 4 for l in langs), "langs"
            assert x.get("main") is None or isinstance(x["main"], str)
            assert x.get("sub") is None or isinstance(x["sub"], str)
            rows.append((i, x, kinds, langs))
        except (ValueError, AssertionError, KeyError, TypeError) as e:
            bad.append(f"line {i}: not a {{\"n\", \"kind\", \"langs\", \"main\", \"sub\"}} JSON line ({e}): {line.strip()[:80]}")
    if todo:
        entries = {}
        for line in open(todo, encoding="utf-8"):
            e = json.loads(line)
            entries[(e["n"], e["kind"])] = set(e["langs"])
        for i, x, kinds, langs in rows:
            if not any((x["n"], k) in entries for k in kinds):
                bad.append(f"line {i}: {x['n']!r} as {'/'.join(kinds)} isn't on the list (the name exactly as given, and its kind)")
    for i, x, kinds, langs in rows:
        main = (x.get("main") or "").strip() or x["n"]
        en = x.get("sub") if main == x["n"] else main
        region = "jp" if "ja" in langs else "gb" if set(langs) & {"cy", "ga", "gd"} else "other"
        p = problem(x["n"], en, region)
        if p:
            bad.append(f"line {i}: {p}")
    cand = [(i, x) for i, x, kinds, langs in rows if "settlement" not in kinds and latin(x["n"]) and not set(langs) & {"cy", "ga", "gd", "en"}]
    empty = [x for i, x in cand if not (x.get("sub") or "").strip() and ((x.get("main") or x["n"]) == x["n"])]
    if len(cand) >= 10 and len(empty) > 0.2 * len(cand):
        bad.append(f"{len(empty)} of the {len(cand)} names that aren't settlements are null ("
                   + ", ".join(repr(x["n"]) for x in empty[:4]) + " …): English leaves few of them alone, "
                   "so give each its English (Río Urdiales: Urdiales River, Pico Bajero: Bajero Peak)")
    if bad:
        shown = bad if show_all else bad[:40]
        print("\n".join(shown) + (f"\n… and {len(bad) - len(shown)} more" if len(bad) > len(shown) else ""), file=sys.stderr)
        return 1
    print(f"{len(rows)} lines, all good", file=sys.stderr)
    return 0


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "--all"]
    if not args:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(check(args[0], args[1] if len(args) > 1 else None, "--all" in sys.argv))
