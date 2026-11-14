#!/usr/bin/env python3
"""The regions' symbols (docs/inputs.md §6.1.3, "Symbols"): each region's own flag, or the symbol it
goes by where it has none (its coat of arms, else its seal), and each folder's (its country's), from
Wikidata and Wikimedia Commons, written in the regions input's own shape for the PM to place in
inputs/regions/: each declaration that gains one, whole, with a [symbol] table after its text, and
symbols.zip with the images. It never writes the inputs folder itself.

  uv run --project producers python producers/flags/fetch.py --regions <a copy of inputs/regions> --out <folder>

A declaration that has a [symbol] already keeps it (a hand-made one stays as the owner made it); the
others are looked up. A region's item is the Wikidata item naming its first osm: relation (P402); a
folder's (one with a _group.toml) the country (P17) most of its regions' items are in. Its image: its
current flag (P41, preferred rank first, none ended by P582), else coat of arms (P94), else seal
(P158); none of those, no symbol (the list shows the placeholder). Commons' SVG as is up to SVG_MAX
bytes, else Commons' own rendering PNG_H px high (a PNG, or a JPEG for a JPEG's: named by what its
bytes are).

Only images under a licence on the regions' check's free list are kept (the list is
crates/inputs/src/inputs/regions/free-licences.txt, read here: one list for both), and not those in
REFUSED (a free-use claim that can't be checked). An author Commons gives only as a pointer ("see
the file history") is left out: the Credits then send to the file's page, where its history names
them.

Out: symbols.zip (the folder's symbols.zip's members still named, and the new ones; deflated, sorted,
dated 1980-01-01, so the same images are the same bytes), each declaration that gains a symbol at its
path, and originals.sha256 (each such declaration's text as it was read: `shasum -a 256 -c` in
inputs/regions/ before placing, so a file the owner changed since isn't overwritten).

Wikidata's query service and the Commons API are documented APIs, used by their policies (a
User-Agent naming the app, one request at a time, Retry-After honoured); the files come from
upload.wikimedia.org as those APIs give them.
"""
import argparse
import hashlib
import html
import io
import json
import os
import re
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile

FETCHES = "api, data"

UA = 'scenic-roads/0.1 (personal offline map)'
SVG_MAX = 60_000
PNG_H = 48
# The most bytes the regions' check takes for an image (crate::inputs::regions::symbols::MAX_BYTES;
# test_fetch.py reads it there).
MAX_BYTES = 64 << 10
PROPS = {'flag': 'P41', 'coat of arms': 'P94', 'seal': 'P158'}
RANK = {'PreferredRank': 0, 'NormalRank': 1}
FREE_LIST = os.path.join(os.path.dirname(__file__), '..', '..', 'crates', 'inputs', 'src', 'inputs', 'regions', 'free-licences.txt')
# Files whose licence tag can't be checked: Cabinda's logo, "free use" by a provincial government's
# site that no longer answers (its permission is only the site's address).
REFUSED = {'Cabinda logo.gif'}
# An author that only points elsewhere, or none.
NO_AUTHOR = re.compile(r'(?i)^((various, )?see (the )?(file|upload) (history|log)\b.*|see below\.?|(unknown ?(author)?)+|n\.a|own work)$')
BUNDLE = 'symbols.zip'


def free_list(path=FREE_LIST):
    with open(path, encoding='utf-8') as f:
        return [l.strip() for l in f if l.strip() and not l.strip().startswith('#')]


def free(licence, lines):
    """Whether `licence` is on the list, word by word ({version}: 4.0; {port}: two lower-case letters),
    as the check matches it (crate::inputs::regions::symbols::free)."""
    words = licence.split(' ')
    for line in lines:
        pat = line.split(' ')
        if len(pat) == len(words) and all(
                re.fullmatch(r'[0-9]\.[0-9]', w) if p == '{version}' else re.fullmatch(r'[a-z]{2}', w) if p == '{port}' else p == w
                for p, w in zip(pat, words)):
            return True
    return False


def kind_of(data):
    """An image's extension by its bytes."""
    if data[:8] == b'\x89PNG\r\n\x1a\n':
        return 'png'
    if data[:3] == b'\xff\xd8\xff':
        return 'jpg'
    return 'svg'


def get(url, data=None, accept=None):
    for _ in range(10):
        req = urllib.request.Request(url, data=data, headers={'User-Agent': UA, **({'Accept': accept} if accept else {})})
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code not in (429, 503):
                raise
            wait = int(e.headers.get('Retry-After') or 30)
            print(f'{e.code}: waiting {wait} s', file=sys.stderr, flush=True)
            time.sleep(wait)
    raise SystemExit(f'gave up on {url}')


def sparql(head, key='?k'):
    def opt(n, p):
        v = n.replace(' ', '_')
        return (f'OPTIONAL {{ ?item p:{p} ?{v}_s . ?{v}_s ps:{p} ?{v} ; wikibase:rank ?{v}_r . '
                f'FILTER(?{v}_r != wikibase:DeprecatedRank) FILTER NOT EXISTS {{ ?{v}_s pq:P582 [] }} }}')
    sel = ' '.join(f'?{n.replace(" ", "_")} ?{n.replace(" ", "_")}_r' for n in PROPS)
    q = f'SELECT ?k ?item {sel} WHERE {{ {head} ' + ' '.join(opt(n, p) for n, p in PROPS.items()) + ' }'
    body = get('https://query.wikidata.org/sparql', urllib.parse.urlencode({'query': q}).encode(), 'application/sparql-results+json')
    time.sleep(1)
    out = {}
    for b in json.loads(body)['results']['bindings']:
        item = b['item']['value'].rsplit('/', 1)[1]
        for n in PROPS:
            v = n.replace(' ', '_')
            if v in b:
                f = urllib.parse.unquote(b[v]['value'].rsplit('/', 1)[1]).replace('_', ' ')
                out.setdefault(b['k']['value'], []).append((list(PROPS).index(n), RANK[b[v + '_r']['value'].rsplit('#', 1)[1]], item, n, f))
    # Each key's best: flag before coat of arms before seal, preferred before normal, then by name.
    return {k: min(v)[2:] for k, v in out.items()}


def author(s):
    """Commons' author, cut to 160 characters, or empty where it only points elsewhere."""
    s = s.replace('</ref>', '').strip()
    return '' if NO_AUTHOR.match(s) else s[:160]


def values(keys):
    return ' '.join(json.dumps(k) for k in keys)


# ---- The regions input, read and written ---------------------------------------------------------

def declarations(folder):
    """Every region file and _group.toml under `folder` (how/, todo/ and the names an inputs folder
    passes over aside): path -> (its text, what it parses to)."""
    out = {}
    for d, dirs, files in os.walk(folder):
        rel = os.path.relpath(d, folder)
        dirs[:] = sorted(x for x in dirs if not x.startswith(('.', '@', '#', '_')) and not (rel == '.' and x in ('how', 'todo')))
        for f in sorted(files):
            if not f.endswith('.toml') or (f.startswith(('.', '@', '#', '_')) and f != '_group.toml'):
                continue
            p = os.path.normpath(os.path.join(rel, f))
            with open(os.path.join(d, f), encoding='utf-8') as h:
                text = h.read()
            try:
                out[p] = (text, tomllib.loads(text))
            except tomllib.TOMLDecodeError as e:
                print(f'{p}: not read ({e})', file=sys.stderr)
    return out


def toml_str(s):
    return json.dumps(s, ensure_ascii=False)


def with_symbol(text, s):
    """A declaration's text with a [symbol] table after it (its text before kept as it was)."""
    lines = [f'image = {toml_str(s["image"])}', f'kind = {toml_str(s["kind"])}', f'source = {toml_str(s["source"])}']
    if s.get('author'):
        lines.append(f'author = {toml_str(s["author"])}')
    lines.append(f'licence = {toml_str(s["licence"])}')
    return (text if text.endswith('\n') or not text else text + '\n') + '\n[symbol]\n' + '\n'.join(lines) + '\n'


def write(out, decls, symbols, images):
    """Writes the shape into `out`: each declaration of `symbols` (path -> table) with its table, the
    bundle of `images` (name -> bytes), and the originals' digests."""
    os.makedirs(out, exist_ok=True)
    sums = []
    for p, s in sorted(symbols.items()):
        text = decls[p][0]
        os.makedirs(os.path.join(out, os.path.dirname(p)), exist_ok=True)
        with open(os.path.join(out, p), 'w', encoding='utf-8') as h:
            h.write(with_symbol(text, s))
        sums.append(f'{hashlib.sha256(text.encode()).hexdigest()}  {p}\n')
    with open(os.path.join(out, 'originals.sha256'), 'w') as h:
        h.writelines(sums)
    with open(os.path.join(out, BUNDLE), 'wb') as h:
        h.write(bundle(images))


def bundle(images):
    """A zip of `images` (name -> bytes): deflated, sorted, dated 1980-01-01."""
    b = io.BytesIO()
    with zipfile.ZipFile(b, 'w') as z:
        for n in sorted(images):
            i = zipfile.ZipInfo(n, date_time=(1980, 1, 1, 0, 0, 0))
            i.compress_type = zipfile.ZIP_DEFLATED
            i.external_attr = 0o644 << 16
            z.writestr(i, images[n], compresslevel=9)
    return b.getvalue()


def kept_images(folder, decls):
    """The members of the folder's own bundle that a declaration names."""
    p = os.path.join(folder, BUNDLE)
    if not os.path.exists(p):
        return {}
    named = {d.get('symbol', {}).get('image') for _, d in decls.values()}
    with zipfile.ZipFile(p) as z:
        return {n: z.read(n) for n in z.namelist() if n in named}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--regions', required=True, help='a copy of inputs/regions (read only)')
    ap.add_argument('--out', required=True, help='where the shape is written for placing')
    a = ap.parse_args()
    lines = free_list()
    decls = declarations(a.regions)
    images = kept_images(a.regions, decls)
    want = {p for p, (_, d) in decls.items() if 'symbol' not in d}
    rel_of = {}
    for p in want:
        if not p.endswith('_group.toml'):
            rel = next((x[4:] for x in decls[p][1].get('outline', []) if isinstance(x, str) and re.fullmatch(r'osm:\d+', x)), None)
            if rel:
                rel_of[p] = rel
    rels = sorted(set(rel_of.values()))
    by_osm = {}
    for i in range(0, len(rels), 120):
        by_osm.update(sparql(f'VALUES ?k {{ {values(rels[i:i + 120])} }} ?item wdt:P402 ?k .'))
    # A folder's country: the one most of its regions' items are in.
    country = {}
    groups = sorted(p for p in want if p.endswith('_group.toml'))
    for g in groups:
        d = os.path.dirname(g)
        rs = sorted({next((x[4:] for x in v[1].get('outline', []) if isinstance(x, str) and re.fullmatch(r'osm:\d+', x)), None)
                     for q, v in decls.items() if os.path.dirname(q) == d and not q.endswith('_group.toml')} - {None})
        if not rs:
            continue
        q = f'SELECT ?c (COUNT(?r) AS ?n) WHERE {{ VALUES ?r {{ {values(rs[:400])} }} ?i wdt:P402 ?r ; wdt:P17 ?c . }} GROUP BY ?c ORDER BY DESC(?n) ?c LIMIT 1'
        b = json.loads(get('https://query.wikidata.org/sparql', urllib.parse.urlencode({'query': q}).encode(), 'application/sparql-results+json'))
        time.sleep(1)
        for row in b['results']['bindings']:
            country[g] = row['c']['value'].rsplit('/', 1)[1]
    by_item = sparql(f'VALUES ?item {{ {" ".join("wd:" + c for c in sorted(set(country.values())))} }} BIND(?item AS ?k)') if country else {}
    by_item = {k.rsplit('/', 1)[1]: v for k, v in by_item.items()}
    chosen = {p: by_osm[r] for p, r in rel_of.items() if r in by_osm}
    chosen.update({g: by_item[c] for g, c in country.items() if c in by_item})
    # The files' metadata (Commons API), 50 at a time.
    meta = {}
    names = sorted({f for _, _, f in chosen.values()})
    strip = lambda s: re.sub(r'\s+', ' ', html.unescape(re.sub(r'<[^>]+>', '', s or ''))).strip()
    for i in range(0, len(names), 50):
        q = {'action': 'query', 'format': 'json', 'formatversion': 2, 'prop': 'imageinfo', 'iiprop': 'url|size|extmetadata|mime',
             'iiurlheight': PNG_H, 'iiextmetadatafilter': 'LicenseShortName|Artist', 'titles': '|'.join('File:' + n for n in names[i:i + 50])}
        j = json.loads(get('https://commons.wikimedia.org/w/api.php?' + urllib.parse.urlencode(q)))
        norm = {n['to']: n['from'] for n in j['query'].get('normalized', [])}
        for pg in j['query']['pages']:
            ii = (pg.get('imageinfo') or [None])[0]
            if ii:
                m = ii.get('extmetadata', {})
                meta[norm.get(pg['title'], pg['title'])[5:]] = {
                    'svg': ii['mime'] == 'image/svg+xml' and ii['size'] <= SVG_MAX, 'url': ii['url'], 'thumb': ii.get('thumburl'), 'page': ii['descriptionurl'],
                    'licence': strip(m.get('LicenseShortName', {}).get('value')),
                    'author': author(strip(m.get('Artist', {}).get('value')))}
        time.sleep(1)
    symbols, fetched = {}, {}
    for p, (item, kind, f) in sorted(chosen.items()):
        m = meta.get(f)
        if not m or not (m['svg'] or m['thumb']) or f in REFUSED or not free(m['licence'], lines):
            continue
        if item not in fetched:
            data = get(m['url'] if m['svg'] else m['thumb'])
            fetched[item] = (f'{item}.{kind_of(data)}', data)
            time.sleep(0.2)
        name, data = fetched[item]
        # (The check's bound: crate::inputs::regions::symbols::MAX_BYTES.)
        if len(data) > MAX_BYTES:
            continue
        images[name] = data
        symbols[p] = {'image': name, 'kind': kind, 'source': m['page'], 'author': m['author'], 'licence': m['licence']}
    write(a.out, decls, symbols, images)
    print(f'{len(symbols)} symbols added ({len(want) - len(symbols)} declarations without one found); '
          f'{len(images)} images in {BUNDLE}')


if __name__ == '__main__':
    main()
