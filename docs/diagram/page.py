#!/usr/bin/env python3
"""pipeline.html: how the map is built, where the data lives, who builds it and the curated inputs, on one page.

usage: page.py OUT.html [--check] [--full]
  --check  data-max on texts (for the overflow check)
  --full   a whole document (the published page is a fragment: the host adds doctype and metas)
"""
import sys
from html import escape as E

import diag
import curated
import proposed
import storage
import workers

OUT = sys.argv[1]
CHECK = '--check' in sys.argv
FULL = '--full' in sys.argv


def mini(kind):
    """Legend samples."""
    if kind == 'src':
        return '<svg width="30" height="16" class="c-osm"><rect class="src" x="1" y="1" width="28" height="14" rx="6"/></svg>'
    if kind == 'kept':
        return '<svg width="30" height="16" class="c-osm"><rect class="kept" x="1" y="2" width="28" height="12" rx="2"/></svg>'
    if kind == 'card':
        return ('<svg width="30" height="20" class="c-osm"><rect class="card" x="1" y="1" width="28" height="18" rx="3"/>'
                '<path class="card-hd" d="M1,8 V4 Q1,1 4,1 H26 Q29,1 29,4 V8 Z"/><rect class="card-ol" x="1" y="1" width="28" height="18" rx="3"/></svg>')
    if kind == 'pill':
        return '<svg width="30" height="16" class="c-osm"><rect class="pill" x="1" y="2" width="28" height="12" rx="6"/></svg>'
    if kind == 'layer':
        return '<svg width="30" height="16" class="c-osm"><rect class="layer" x="1" y="1" width="28" height="14" rx="3"/></svg>'
    if kind == 'computed':
        return '<svg width="30" height="16" class="c-osm"><rect class="layer computed" x="1.5" y="1.5" width="27" height="13" rx="3"/></svg>'
    if kind == 'later':
        return ('<svg width="30" height="20" class="c-osm"><rect class="card" x="1" y="1" width="28" height="18" rx="3"/>'
                '<rect class="card-ol later" x="1" y="1" width="28" height="18" rx="3"/></svg>')
    if kind == 'cur':
        return ('<svg width="30" height="20" class="c-osm"><path class="cur" d="M3,1 H22 L29,8 V16 Q29,19 26,19 H4 Q1,19 1,16 V4 Q1,1 4,1 Z"/>'
                '<path class="cur-fold" d="M22,1 V8 H29"/></svg>')
    if kind in diag.REPRO:
        return f'<svg width="30" height="16">{diag.repro_mark(15, 8, kind, r=5)}</svg>'
    if kind == 'shared':
        return f'<svg width="30" height="16" class="c-osm"><text class="tool" x="15" y="12.5" text-anchor="middle" style="font-size:15px">{diag.SHARED}</text></svg>'
    t = diag.TAB[kind]
    w = diag.width('tab-t', t) + 14
    return (f'<svg width="{w + 2:.0f}" height="17" class="c-osm"><path class="tab {kind}" d="M1,16.5 V4 Q1,1 4,1 H{w - 2:.1f} Q{w + 1:.1f},1 {w + 1:.1f},4 V16.5 Z"/>'
            f'<text class="tab-t" x="{w / 2 + 1:.1f}" y="12.2" text-anchor="middle">{E(t)}</text></svg>')


classes = [('base', 'Map context'), ('place', 'Places & heritage'), ('terr', 'Elevation & terrain'), ('net', 'Road, rail & ferry network'),
           ('scen', 'Scenic metrics'), ('land', 'Land cover & trees'), ('bldg', 'Buildings'), ('osm', 'OpenStreetMap (shared)'), ('mix', 'Several classes')]
shapes = [('src', 'source'), ('cur', 'curated input: made by hand'), ('kept', 'kept between runs'), ('card', 'build step, over the files it writes'),
          ('later', 'dashed: planned, or built but not in use'), ('pill', 'server route'), ('layer', 'map layer'), ('computed', 'made in the browser')]
scopes = [('area', 'built per area (a z6 tile, 300–600 km)'), ('pack', 'per z3 pack near the coverage'), ('global', 'once, for the whole world')]
runs = [('shared', 'any member of the pool may take its jobs'), ('task', 'parts run as tasks: any worker, a device’s page too')]
repro = [(k, diag.REPRO[k]) for k in ('doc', 'part', 'none', 'gone')]
formats = [('packs', 'a layer’s tiles under one root tile, with an index'), ('sectioned', 'named arrays: base packs, hidata, markdata'),
           ('PMTiles', 'the worldwide basemap, read by byte range'), ('RT v7', 'columnar road tiles, 13 scenic channels a point'),
           ('Terrarium', 'height (or slope, cover…) as RGB'), ('MVT', 'vector tiles: labels, overlays, stations'),
           ('RDMT', 'landmark points in typed columns')]

legend = ('<section class="legend" aria-label="Legend">'
          '<div><h2>Class of data</h2><ul class="cols2">' + ''.join(f'<li class="c-{c}"><span class="dot"></span>{E(n)}</li>' for c, n in classes) + '</ul></div>'
          '<div><h2>Shapes</h2><ul class="cols2">' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in shapes) + '</ul></div>'
          '<div><h2>Scope</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in scopes) + '</ul></div>'
          '<div><h2>Who runs it</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in runs) + '</ul></div>'
          '<div><h2>Curated input: can it be made again?</h2><ul class="cols2">' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in repro) + '</ul></div>'
          '<div><h2>Formats</h2><ul class="cols2">' + ''.join(f'<li><code>{E(c)}</code><span class="g">{E(n)}</span></li>' for c, n in formats) + '</ul></div>'
          '</section>')

head = '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n' if FULL else ''
page = head + f'''<title>Scenic Roads Data Pipeline</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans+Condensed:wght@400;600&display=swap">
<style>{diag.CSS}{curated.CSS}</style>
{'</head><body>' if FULL else ''}
<main>
<header>
<div>
<h1>Scenic Roads · data pipeline</h1>
<p class="sub">OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show. One Mac leads the build (the M4 today; any Mac can take it over), and the others, and any device you let help, take work from it. What a person or an agent made by hand, the curated inputs, is marked apart, with how it could be made again (the list at the end). The map: 88 regions, built from the planet of 28 September 2026.</p>
</div>
</header>
{legend}
<figure>
<h3>Where the data lives</h3>
<div class="scroll">{storage.build(CHECK)}</div>
<figcaption>Big files live only on the NAS. Every Mac’s jobs put their files there and their results in the journal; the lead alone merges them into the build’s records. What was made by hand goes straight into the NAS’s inputs/, translations/, descriptions/ and sources/. Your Macs copy only what you download in the Regions panel (the World, zoomed out; regions; views), and read the rest from the NAS.</figcaption>
</figure>
<figure>
<h3>Who builds it</h3>
<div class="scroll">{workers.build(CHECK)}</div>
<figcaption>The lead’s agent plans and alone merges the journal into the build’s records; its coordinator lends the work out. Any other Mac, when it’s open, takes the jobs of the steps marked ⇄ below that fit it, and any device you let help through the build page takes the steps’ parts marked TASK. A lease not renewed for ten minutes goes back out, so a worker that sleeps or leaves costs only its work in hand. Dashed: planned.</figcaption>
</figure>
<figure>
<h3>How each layer is built</h3>
<div class="scroll">{proposed.build(CHECK)}</div>
<figcaption>Every step runs per area (a z6 tile), per z3 pack near the coverage, or once for the whole world; regions only say which areas to build, and a round publishes them as they’re done. Values that span areas (whole roads, landmarks’ fame, ferries) come from worldwide steps, and each area reads its neighbours within 110 km, so tile edges and region borders don’t show. Translations and descriptions need no rebuild. Folded corner: a curated input, with how it could be made again. Dashed: planned, or built but not yet in the map.</figcaption>
</figure>
{curated.html()}
</main>
{'</body></html>' if FULL else ''}
'''
open(OUT, 'w').write(page)
print(f'wrote {OUT}')
