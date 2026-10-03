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

# Implementation progress (docs/plan.md §10), as of the date shown.
PROGRESS_AT = '3 October 2026, 03:00'
PHASES = [
    ('done-ish', 'Foundations on today’s data', 'Both Macs run the new app from the NAS. Compared with today’s map in nine places (Québec, Tokyo, London, Chamonix, Vancouver, Hong Kong, Taipei, Lisbon, Northumberland): roads, elevation profiles, every tile layer and popup details all equal. The map reads only the pages a request needs from the NAS (a first hover in Tokyo: 66 s before, 1.9 s now).',
     'Left: the speed check against today’s app once this Mac’s local copy is in.'),
    ('mostly', 'Build agent and moving the data', 'The agent runs on the build Mac from the installed app: one job at a time, waits for mains power, backs up your folders and clears replaced files. Both Macs’ data is on the NAS, checked file by file; the old build folders are gone.',
     'Left: the build Mac’s caches, being copied, then moved into the agent’s.'),
    ('active', 'Worldwide OpenStreetMap pass', 'Cuts the planet a quarter at a time so it fits the build Mac’s memory and disk (checked identical to the old cut). Running on the full planet since 3 October, 00:40: the planet is filtered (60.6 GB, 68 % of it) and being copied to the NAS, slowly while the NAS is also busy with its own sync (13 MB/s).',
     'Left: the worldwide sets, outlines, basemap, cutting into areas and road values (several hours); trees and overlays for new areas.'),
    ('mostly', 'Per-area building', 'Pilot done: Northumberland and the Scottish Borders built the new way match today’s data (same roads, elevations within 1 m, every scenery score and flag), and roads cross their border seamlessly.',
     'Left: rankings for new areas (fame, ferries, rail service).'),
    ('active', 'In the browser', 'The Regions panel, the status bar and switching to new data in place. Tiles that fail while the NAS is busy or away are asked for again by themselves. Landmarks by view: today’s 584,000 stops & sights and heritage sites become per-area points and zoomed-out tiles; the In view numbers, Sights lists and popups come from the server, equal to today’s in every one of 163 views checked, and the browser holds only what’s near the view (its landmarks memory: about 1 MB instead of 180 MB).',
     'Left: publishing landmarks by view; overlays, stations and ferries the same way; faster zoomed-out drive lists (designed, under review); landmarks for new areas.'),
    ('later', 'Switching over', 'Today’s 30 regions as outlines, built the new way after the pass, compared, then switched to.', ''),
]
STATE = {'done-ish': ('nearly done', 'st-near'), 'mostly': ('mostly done', 'st-near'), 'active': ('in progress', 'st-on'), 'later': ('later', 'st-later')}


def progress_html():
    rows = []
    for i, (st, name, what, left) in enumerate(PHASES, 1):
        label, cls = STATE[st]
        rows.append(f'<li><span class="pn">{i}</span><div class="pb"><div class="ph"><b>{E(name)}</b><span class="chip {cls}">{E(label)}</span></div>'
                    f'<p>{E(what)}</p>' + (f'<p class="left">{E(left)}</p>' if left else '') + '</div></li>')
    return (f'<section class="prog" aria-label="Progress"><h3>Progress · {E(PROGRESS_AT)}</h3><ol>' + ''.join(rows) + '</ol></section>')


PROG_CSS = """
.prog{margin:0 0 16px;padding:12px 14px;border:1px solid var(--rule);border-radius:10px;background:var(--surface)}
.prog h3{font:600 10.5px/1 var(--sans);letter-spacing:.08em;color:var(--faint);text-transform:uppercase;margin:0 0 10px}
.prog ol{list-style:none;margin:0;padding:0;display:grid;gap:8px}
.prog li{display:flex;gap:10px;align-items:flex-start}
.prog .pn{flex:none;width:20px;height:20px;border-radius:50%;display:grid;place-items:center;font:600 11px/1 var(--mono);color:var(--muted);border:1px solid var(--rule)}
.prog .pb{min-width:0;flex:1}
.prog .ph{display:flex;flex-wrap:wrap;gap:6px 10px;align-items:baseline}
.prog .ph b{font:600 14px/1.3 var(--sans);color:var(--fg)}
.prog p{margin:3px 0 0;color:var(--muted);font-size:12.5px;line-height:1.45}
.prog p.left{color:var(--fg)}
.chip{font:500 10.5px/1 var(--mono);padding:3px 6px;border-radius:999px;border:1px solid currentColor;white-space:nowrap}
.st-near{color:var(--c-land)}.st-on{color:var(--c-net)}.st-later{color:var(--faint)}
"""

head = '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n' if FULL else ''
page = head + f'''<title>Scenic Roads Data Pipeline</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans+Condensed:wght@400;600&display=swap">
<style>{diag.CSS}{PROG_CSS}</style>
{'</head><body>' if FULL else ''}
<main>
<input type="radio" name="view" id="v-new" class="vsel" checked aria-label="Proposed">
<input type="radio" name="view" id="v-old" class="vsel" aria-label="Today">
<header>
<div>
<h1>Scenic Roads · data pipeline</h1>
<p class="sub">Proposed: OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show; only the build Mac builds; your translations go in a NAS folder and show up within a minute. Being built now (progress below); “Today” shows the app as it is.</p>
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
{progress_html()}
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
