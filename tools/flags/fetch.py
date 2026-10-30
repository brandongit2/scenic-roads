#!/usr/bin/env python3
"""The Regions list's flags (web/src/flags.ts, docs/surface.md): each region's own flag, or the
symbol it goes by where it has none (its coat of arms, else its seal), from Wikidata and Wikimedia
Commons, stored in web/src/flags/ with each file's source, author and licence (index.json).

  python3 -I tools/flags/fetch.py [--server http://localhost:7311] [--out web/src/flags]

The regions come from a map server (/api/regions and /api/regions/outlines?z=4: each region's
first osm: relation and its area's ISO 3166 code). A region's item is the Wikidata item naming its
relation (P402), else its ISO 3166-2 code (P300); a country's (folders') by ISO 3166-1 (P297). Its
image: its current flag (P41, preferred rank first, none ended by P582), else coat of arms (P94),
else seal (P158); none of those, no file (the list shows the placeholder). Commons' SVG as is up to
SVG_MAX bytes, else Commons' own PNG rendering PNG_H px high (big coats of arms, raster sources).

Wikidata's query service and the Commons API are documented APIs, used by their policies (a
User-Agent naming the app, one request at a time, Retry-After honoured); the files come from
upload.wikimedia.org as those APIs give them. Runs again whenever regions change; a region it
doesn't know shows the placeholder until then.
"""
import argparse
import html
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

UA = 'scenic-roads/0.1 (personal offline map)'
SVG_MAX = 60_000
PNG_H = 48
PROPS = {'flag': 'P41', 'coat of arms': 'P94', 'seal': 'P158'}
RANK = {'PreferredRank': 0, 'NormalRank': 1}


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


def sparql(head):
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


def values(keys):
    return ' '.join(json.dumps(k) for k in keys)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--server', default='http://localhost:7311')
    ap.add_argument('--out', default=os.path.join(os.path.dirname(__file__), '..', '..', 'web', 'src', 'flags'))
    a = ap.parse_args()
    regions = json.loads(get(a.server + '/api/regions'))['regions']
    areas = {f['properties']['region']: f['properties'].get('area') or {} for f in json.loads(get(a.server + '/api/regions/outlines?z=4'))['features']}
    # (Only the first osm: part names a region's area, as the server's `area` does.)
    rels = sorted({next((p[4:] for p in r['outline'] if re.fullmatch(r'osm:\d+', p)), None) for r in regions} - {None})
    isos = sorted({x['iso'] for x in areas.values() if '-' in (x.get('iso') or '')})
    by_osm, by_iso = {}, {}
    for i in range(0, len(rels), 120):
        by_osm.update(sparql(f'VALUES ?k {{ {values(rels[i:i + 120])} }} ?item wdt:P402 ?k .'))
    for i in range(0, len(isos), 120):
        by_iso.update(sparql(f'VALUES ?k {{ {values(isos[i:i + 120])} }} ?item wdt:P300 ?k .'))
    by_iso.update(sparql('?item wdt:P297 ?k . ?item wdt:P31/wdt:P279* wd:Q56061 .'))
    items = {}
    for item, kind, f in [*by_osm.values(), *by_iso.values()]:
        items[item] = (kind, f)
    # The files' metadata (Commons API), 50 at a time.
    meta = {}
    names = sorted({f for _, f in items.values()})
    strip = lambda s: re.sub(r'\s+', ' ', html.unescape(re.sub(r'<[^>]+>', '', s or ''))).strip()
    for i in range(0, len(names), 50):
        q = {'action': 'query', 'format': 'json', 'formatversion': 2, 'prop': 'imageinfo', 'iiprop': 'url|size|extmetadata|mime',
             'iiurlheight': PNG_H, 'iiextmetadatafilter': 'LicenseShortName|Artist', 'titles': '|'.join('File:' + n for n in names[i:i + 50])}
        j = json.loads(get('https://commons.wikimedia.org/w/api.php?' + urllib.parse.urlencode(q)))
        norm = {n['to']: n['from'] for n in j['query'].get('normalized', [])}
        for p in j['query']['pages']:
            ii = (p.get('imageinfo') or [None])[0]
            if ii:
                m = ii.get('extmetadata', {})
                meta[norm.get(p['title'], p['title'])[5:]] = {
                    'svg': ii['mime'] == 'image/svg+xml' and ii['size'] <= SVG_MAX, 'url': ii['url'], 'thumb': ii.get('thumburl'), 'page': ii['descriptionurl'],
                    'licence': strip(m.get('LicenseShortName', {}).get('value')) or 'see its page',
                    'author': strip(m.get('Artist', {}).get('value'))[:160]}
        time.sleep(1)
    os.makedirs(a.out, exist_ok=True)
    old = {f for f in os.listdir(a.out) if re.fullmatch(r'Q\d+\.(svg|png)', f)}
    index = {'osm': {}, 'iso': {}, 'items': {}}
    for item, (kind, f) in sorted(items.items()):
        m = meta.get(f)
        if not m or not (m['svg'] or m['thumb']):
            continue
        file = f'{item}.{"svg" if m["svg"] else "png"}'
        path = os.path.join(a.out, file)
        if file not in old:
            body = get(m['url'] if m['svg'] else m['thumb'])
            with open(path, 'wb') as o:
                o.write(body)
            time.sleep(0.2)
        old.discard(file)
        index['items'][item] = {'file': file, 'kind': kind, 'name': f, 'page': m['page'], 'licence': m['licence'], 'author': m['author'], 'rendered': not m['svg']}
    for k, (item, _, _) in sorted(by_osm.items()):
        if item in index['items']:
            index['osm'][k] = item
    for k, (item, _, _) in sorted(by_iso.items()):
        if item in index['items']:
            index['iso'][k] = item
    for f in old:
        os.remove(os.path.join(a.out, f))
    with open(os.path.join(a.out, 'index.json'), 'w') as o:
        json.dump(index, o, indent=0, ensure_ascii=False, sort_keys=True)
        o.write('\n')
    print(f'{len(index["items"])} files ({sum(1 for v in index["items"].values() if v["rendered"])} rendered PNG), '
          f'{len(index["osm"])} relations, {len(index["iso"])} ISO codes; {len(old)} removed')


if __name__ == '__main__':
    main()
