#!/usr/bin/env python3
"""pipeline.html: the 'Proposed' and 'Today' views of the pipeline on one page.

usage: page.py OUT.html [--check] [--full]
  --check  data-max on texts (for the overflow check)
  --full   a whole document (the published page is a fragment: the host adds doctype and metas)
"""
import sys
from html import escape as E

import diag
import proposed
import storage
import today

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
    t = diag.TAB[kind]
    w = 5.6 * len(t) + 14
    return (f'<svg width="{w + 2:.0f}" height="15" class="c-osm"><path class="tab {kind}" d="M1,14.5 V4 Q1,1 4,1 H{w - 2:.1f} Q{w + 1:.1f},1 {w + 1:.1f},4 V14.5 Z"/>'
            f'<text class="tab-t" x="{w / 2 + 1:.1f}" y="11" text-anchor="middle">{E(t)}</text></svg>')


classes = [('base', 'Map context'), ('place', 'Places & heritage'), ('terr', 'Elevation & terrain'), ('net', 'Road, rail & ferry network'),
           ('scen', 'Scenic metrics'), ('land', 'Land cover & trees'), ('bldg', 'Buildings'), ('osm', 'OpenStreetMap (shared)'), ('mix', 'Several classes')]
shapes = [('src', 'source'), ('kept', 'kept between runs'), ('card', 'build step, over the files it writes'),
          ('pill', 'server route'), ('layer', 'map layer'), ('computed', 'made in the browser')]
scopes = [('area', 'built per area (a z6 tile, 300–600 km)'), ('global', 'built once for the whole world')]
formats = [('flat arrays', 'one value per road point, memory-mapped'), ('z11 grid', 'one raster per layer over grid.idx'),
           ('archive', 'one file: tile blobs + sorted index'), ('PMTiles', 'one file, read by byte range'),
           ('RT v6', 'columnar road tiles, 13 scenic channels a point'), ('Terrarium', 'height (or slope, cover…) as RGB'),
           ('packs', 'proposed: one area’s tiles in one file')]

legend = ('<section class="legend" aria-label="Legend">'
          '<div><h2>Class of data</h2><ul class="cols2">' + ''.join(f'<li class="c-{c}"><span class="dot"></span>{E(n)}</li>' for c, n in classes) + '</ul></div>'
          '<div><h2>Shapes</h2><ul class="cols2">' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in shapes) + '</ul></div>'
          '<div class="only-new"><h2>Scope</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in scopes) + '</ul></div>'
          '<div><h2>Formats</h2><ul class="cols2">' + ''.join(f'<li><code>{E(c)}</code><span class="g">{E(n)}</span></li>' for c, n in formats) + '</ul></div>'
          '</section>')

head = '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n' if FULL else ''
page = head + f'''<title>Scenic Roads Data Pipeline</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans+Condensed:wght@400;600&display=swap">
<style>{diag.CSS}</style>
{'</head><body>' if FULL else ''}
<main>
<input type="radio" name="view" id="v-new" class="vsel" checked aria-label="Proposed">
<input type="radio" name="view" id="v-old" class="vsel" aria-label="Today">
<header>
<div>
<h1>Scenic Roads · data pipeline</h1>
<p class="sub">Proposed: OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show; only the build Mac builds; your translations go in a NAS folder and show up within a minute. Nothing here is built yet; “Today” shows the app as it is.</p>
</div>
<div class="tabs"><label for="v-new">Proposed</label><label for="v-old">Today</label></div>
</header>
{legend}
<div class="view-new">
<section class="you" aria-label="Your part">
<h3>Your part</h3>
<div class="you-grid">
<div><code>localhost:8080</code><span>view the map: it’s always running on both Macs (<code>scenic</code> opens it too)</span></div>
<div><code>Regions panel</code><span>add a region: search any place, then take the unit around it or a bigger one; it appears as it’s built</span></div>
<div><code>translations/&lt;area&gt;/</code><span>drop finished translations in this NAS folder; both Macs show them within a minute. <code>descriptions/</code> likewise</span></div>
</div>
<p>Now and then: “Keep this view” for trips away · remove a region in the panel · <code>scenic status</code>. Everything else (building, refreshing, copying, backups) happens on its own.</p>
</section>
<figure>
<h3>Where the data lives</h3>
<div class="scroll">{storage.build(CHECK)}</div>
<figcaption>Big files live only on the NAS, and only the build Mac writes them. Your Macs keep what you view, within a budget, and read the rest from the NAS.</figcaption>
</figure>
<figure>
<h3>How each layer is built</h3>
<div class="scroll">{proposed.build(CHECK)}</div>
<figcaption>Every step runs per area (a z6 tile) or once for the whole world; regions only say which areas to build. Values that span areas (whole roads, rail service, ferries, big parks) come from worldwide steps, and each area reads its neighbours within 100 km, so tile edges and region borders don’t show. Translations need no rebuild.</figcaption>
</figure>
</div>
<div class="view-old">
<figure>
<div class="scroll">{today.build(CHECK)}</div>
<figcaption>Make skips a step whose inputs haven’t changed, and most steps redo only what is new, from what they kept (dashed) or their last output. Along the roads row each step adds per-point arrays; tile packs them all into the road tiles.</figcaption>
</figure>
</div>
</main>
{'</body></html>' if FULL else ''}
'''
open(OUT, 'w').write(page)
print(f'wrote {OUT}')
