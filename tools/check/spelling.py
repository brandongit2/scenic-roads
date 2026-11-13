#!/usr/bin/env python3
"""Canadian spelling guard (task #1374; tools/dev/agent-rules.md, Spelling): -ize and -yze, never
-ise and -yse, in anything tracked (or new and not ignored) that a person or a program reads.

  python3 tools/check/spelling.py            scan the repository (tools/app/guards.sh runs it: check.sh and publish)
  python3 tools/check/spelling.py PATH ...   scan these files only

The list is explicit: each STEMS entry is a word's base without its ending, flagged as
stem + is(e|es|ed|ing|ation|ations|er|ers|able), or for YSE_STEMS stem + ys(e|ed|es|ing|er|ers)
("analyses" is analysis's plural, so it is not flagged). Words that only look like -ise stay
unlisted (raise, otherwise, exercise, promise, precise, supervise, emphasis, ...), and -our and
-re spellings are not checked at all. Identifiers are split at camelCase, digits and underscores,
so `equaliseHistogram` and `EQUALISE_MAX` are found as well as prose.

Not flagged: URLs, vendored or generated code (web/maplibre/, crates/geos-src/, lockfiles), data files (geojson, csv),
binary files. A deliberate use is marked on its own line with `spelling: kept` (and why), e.g. a
stored name an older file or peer still sends, a quotation, a body's proper name ("Organisation").
This file and its test name the forbidden words, so they are not scanned.
"""
import os
import re
import subprocess
import sys

YSE_STEMS = ["anal", "paral", "catal", "dial", "hydrol", "electrol"]
STEMS = """
equal general raster optim summar recogn normal serial initial final standard util real author organ
capital harmon memo penal quant roman skolem synthes emphas vector minim maxim custom categor priorit
synchron special character local central virtual visual token sanit random digit legal stabil symbol
mobil neutral ideal global formal hospital item label modern personal popular polar rational sympath
theor urban vocal memorial fossil magnet ion oxid militar national natural margin liberal industrial
immun jeopard mechan apolog critic colon fertil steril scrutin traumat terror ital galvan hypothes
monet subsid tyrann vandal energ crystal cannibal commercial conceptual contextual criminal decentral
demoral fantas formul hybrid individual legitim lobotom moral monopol orchestr plagiar pressur proselyt
satir sensit social stereotyp strateg subordinat tantal universal verbal vaporiz
""".split()
STEMS = sorted(set(STEMS))
# The list is a net of real words: a stem that builds a non-word never matches anything. A real
# word of another meaning is marked where it is used (`spelling: kept`).

ISE_END = ("e", "es", "ed", "ing", "ation", "ations", "er", "ers", "able")
YSE_END = ("e", "ed", "es", "ing", "er", "ers")


def build_words():
    words = {}
    for s in STEMS:
        for e in ISE_END:
            words[s + "is" + e] = s + "iz" + e
    for s in YSE_STEMS:
        for e in YSE_END:
            if s == "anal" and e == "es":
                continue  # "analyses": the plural of analysis
            words[s + "ys" + e] = s + "yz" + e
    return words


WORDS = build_words()
URL = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://\S+|\b[0-9a-f]{16,}\b")
PART = re.compile(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+")
MARK = "spelling: kept"
SKIP_PREFIX = ("web/maplibre/", "crates/geos-src/")
SKIP_NAME = re.compile(r"(?:\.lock|-lock\.yaml|-lock\.json|\.min\.js|\.geojson|\.csv|\.tsv|\.ndjson)$")
SELF = {"tools/check/spelling.py", "tools/check/test_spelling.py"}


def scan_text(text):
    """[(line number, word, suggestion)] of the -ise/-yse words in text."""
    found = []
    for n, line in enumerate(text.split("\n"), 1):
        if MARK in line:
            continue
        line = URL.sub(" ", line)
        for tok in re.finditer(r"[A-Za-z]+", line):
            for p in PART.finditer(tok.group(0)):
                w = p.group(0).lower()
                if w in WORDS:
                    found.append((n, p.group(0), WORDS[w]))
    return found


def files(root):
    out = subprocess.run(["git", "ls-files", "-z", "-c", "-o", "--exclude-standard"], cwd=root,
                         capture_output=True, text=True, check=True).stdout
    return [f for f in out.split("\0") if f]


def scan(root, paths=None):
    bad = []
    for f in paths if paths is not None else files(root):
        if f in SELF or f.startswith(SKIP_PREFIX) or SKIP_NAME.search(f):
            continue
        p = os.path.join(root, f)
        try:
            with open(p, encoding="utf-8") as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError):
            continue
        for n, w, s in scan_text(text):
            bad.append(f"{f}:{n}: {w}  <- Canadian spelling is {s} (tools/dev/agent-rules.md); a deliberate use says `{MARK}` and why")
    return bad


def main(argv):
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    bad = scan(root, argv or None)
    for b in bad:
        print(b)
    if bad:
        print(f"spelling: {len(bad)} -ise/-yse spellings (tools/check/spelling.py)")
        return 1
    print("spelling: no -ise/-yse spellings")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
