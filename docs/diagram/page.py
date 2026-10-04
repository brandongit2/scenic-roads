#!/usr/bin/env python3
"""pipeline.html: how the map is built, where the data lives, and the progress, on one page.

usage: page.py OUT.html [--check] [--full]
  --check  data-max on texts (for the overflow check)
  --full   a whole document (the published page is a fragment: the host adds doctype and metas)
"""
import sys
from html import escape as E

import diag
import proposed
import storage

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
    t = diag.TAB[kind]
    w = 5.6 * len(t) + 14
    return (f'<svg width="{w + 2:.0f}" height="15" class="c-osm"><path class="tab {kind}" d="M1,14.5 V4 Q1,1 4,1 H{w - 2:.1f} Q{w + 1:.1f},1 {w + 1:.1f},4 V14.5 Z"/>'
            f'<text class="tab-t" x="{w / 2 + 1:.1f}" y="11" text-anchor="middle">{E(t)}</text></svg>')


classes = [('base', 'Map context'), ('place', 'Places & heritage'), ('terr', 'Elevation & terrain'), ('net', 'Road, rail & ferry network'),
           ('scen', 'Scenic metrics'), ('land', 'Land cover & trees'), ('bldg', 'Buildings'), ('osm', 'OpenStreetMap (shared)'), ('mix', 'Several classes')]
shapes = [('src', 'source'), ('kept', 'kept between runs'), ('card', 'build step, over the files it writes'),
          ('later', 'not built yet, or built but off'), ('pill', 'server route'), ('layer', 'map layer'), ('computed', 'made in the browser')]
scopes = [('area', 'built per area (a z6 tile, 300–600 km)'), ('pack', 'per z3 pack near the coverage'), ('global', 'once, for the whole world')]
formats = [('packs', 'a layer’s tiles under one root tile, with an index'), ('sectioned', 'named arrays: base packs, hidata, markdata'),
           ('PMTiles', 'the worldwide basemap, read by byte range'), ('RT v7', 'columnar road tiles, 13 scenic channels a point'),
           ('Terrarium', 'height (or slope, cover…) as RGB'), ('MVT', 'vector tiles: labels, overlays, stations'),
           ('RDMT', 'landmark points in typed columns')]

legend = ('<section class="legend" aria-label="Legend">'
          '<div><h2>Class of data</h2><ul class="cols2">' + ''.join(f'<li class="c-{c}"><span class="dot"></span>{E(n)}</li>' for c, n in classes) + '</ul></div>'
          '<div><h2>Shapes</h2><ul class="cols2">' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in shapes) + '</ul></div>'
          '<div><h2>Scope</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in scopes) + '</ul></div>'
          '<div><h2>Formats</h2><ul class="cols2">' + ''.join(f'<li><code>{E(c)}</code><span class="g">{E(n)}</span></li>' for c, n in formats) + '</ul></div>'
          '</section>')

# Implementation progress (docs/plan.md §10), as of the date shown.
PROGRESS_AT = '3 October 2026, 22:45'
PHASES = [
    ('done', 'Foundations on today’s data', 'Both Macs run the new app from the NAS. Compared with today’s map in nine places (Québec, Tokyo, London, Chamonix, Vancouver, Hong Kong, Taipei, Lisbon, Northumberland): roads, elevation profiles, every tile layer and popup details all equal. The map reads only the pages a request needs from the NAS (a first hover in Tokyo: 66 s before, 1.9 s now).',
     'Left: a speed check against the old app’s measurements once this Mac’s copy is complete.'),
    ('done', 'Build agent and moving the data', 'The agent runs on the build Mac from the installed app: one job at a time, on mains power or on battery down to 30 %; it backs up your folders and clears replaced files. Per-area work runs in small batches, so a failure or a new app costs one batch. A menu bar item on both Macs shows whether it’s building, paused or waiting, with the job’s progress bar, the time left and a checklist of every step to the end. Both Macs’ data is on the NAS; both reach it over the home network rather than Tailscale (60 MB/s instead of 12).',
     None),
    ('active', 'Worldwide OpenStreetMap pass', 'The first pass, on the 2026-09-28 planet: filtered (60.6 GB, 64 % of the planet), sets and outlines on the NAS, the worldwide basemap made (Planetiler in 46 minutes; 28.6 GB), the planet cut into areas (about 58 GB). The worldwide coarse terrain for peaks is made.',
     'Left: the road values, the pass’s last stage; trees for new areas.'),
    ('mostly', 'Per-area building', 'Pilot done: Northumberland and the Scottish Borders built the new way match today’s data (same roads, elevations within 1.8 m, every scenery score and flag). Elevations reach 6,053 m. Landmarks build as their own chain beside the roads: points of interest (the same every run), peaks’ prominence and isolation independent of area borders (8,225 of 8,235 Sierra Nevada peaks equal today’s; Fuji’s isolation now 2,076 km along the globe, published 2,077), Wikidata facts and pageviews. Heritage sites and designated areas come from the pass before the areas, and each area rasterises its own flags, equal to today’s on every tile. The rest of the heritage chain is written and reproduces today’s outputs byte for byte; it stays off until the switch-over’s comparison.',
     'Left: trains a day and the names to-do list; switching the heritage chain on.'),
    ('done', 'In the browser', 'Live on both Macs: landmarks, area overlays, rail stops and ferries by view (the In view numbers, lists and popups equal to today’s in 163 views). Against today’s whole files: 3–4× less browser memory (53–74 MB instead of 186–232), the map done loading sooner (London 3.3 s instead of 4.7, Alps 2.9 instead of 4.7), the same frame rates. Zoomed-out drive, ride and rail-line lists come from 500 m summaries (“≈” by the count). The Regions panel adds, renames and removes regions.',
     'Left: drawing, splitting and merging regions in the panel.'),
    ('active', 'Switching over', 'Today’s 34 regions are recipes on the NAS, held for review: once the pass ends, the agent builds them (terrain, slope, heritage sites, the areas, then roads and landmarks) into a catalog kept apart until it’s compared with today’s map.',
     'Left: the build, the comparison (heritage included), then the switch itself and deleting today’s converted data.'),
]
GAPS = ('Found when every document was checked against the code (3 October): removing a region keeps what’s already built on the map; '
        'an old pass’s files stay on the NAS (about 200 GB each); regions beyond today’s 34 would lack trees, roadside buildings, '
        'trains a day and roads’ own English, which come from today’s converted files. Each is listed in the plan to fix before '
        'regions beyond today’s are added.')
STATE = {'done': ('done', 'st-near'), 'mostly': ('mostly done', 'st-near'), 'active': ('in progress', 'st-on'), 'later': ('later', 'st-later')}


def progress_html():
    rows = []
    for i, (st, name, what, left) in enumerate(PHASES, 1):
        label, cls = STATE[st]
        rows.append(f'<li><span class="pn">{i}</span><div class="pb"><div class="ph"><b>{E(name)}</b><span class="chip {cls}">{E(label)}</span></div>'
                    f'<p>{E(what)}</p>' + (f'<p class="left">{E(left)}</p>' if left else '') + '</div></li>')
    return (f'<section class="prog" aria-label="Progress"><h3>Progress · {E(PROGRESS_AT)}</h3><ol>' + ''.join(rows) + '</ol>'
            f'<p class="gaps">{E(GAPS)}</p></section>')


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
.prog p.gaps{margin:10px 0 0;padding-top:8px;border-top:1px solid var(--rule)}
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
<header>
<div>
<h1>Scenic Roads · data pipeline</h1>
<p class="sub">OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show; only the build Mac builds; your translations go in a NAS folder and show up within a minute or two. Both Macs already run this way, serving today’s map converted; the first build from the planet is under way (progress below).</p>
</div>
</header>
{legend}
<section class="you" aria-label="Your part">
<h3>Your part</h3>
<div class="you-grid">
<div><code>localhost:8080</code><span>view the map: it’s always running on both Macs; the menu bar item (or <code>scenic status</code>) shows what the build Mac is doing</span></div>
<div><code>Regions panel</code><span>add a region: find an area by name or click the map, then take it or a bigger one around it (county, province, country); it appears once built</span></div>
<div><code>translations/</code><span>drop finished translations in this NAS folder; both Macs show them within a minute or two. <code>descriptions/</code> likewise. Lists of what’s missing will appear in their <code>todo/</code></span></div>
</div>
<p>Now and then: rename or remove a region in the panel (what’s built stays for now). Everything else (building, refreshing, copying, backups) happens on its own.</p>
</section>
{progress_html()}
<figure>
<h3>Where the data lives</h3>
<div class="scroll">{storage.build(CHECK)}</div>
<figcaption>Big files live only on the NAS, and only the build Mac writes them. Your Macs copy every file the map reads, as far as their budget allows, and read the rest from the NAS.</figcaption>
</figure>
<figure>
<h3>How each layer is built</h3>
<div class="scroll">{proposed.build(CHECK)}</div>
<figcaption>Every step runs per area (a z6 tile), per z3 pack near the coverage, or once for the whole world; regions only say which areas to build. Values that span areas (whole roads, landmarks’ fame, ferries) come from worldwide steps, and each area reads its neighbours within 110 km, so tile edges and region borders don’t show. Translations need no rebuild. Dashed steps are not built yet, or built but off.</figcaption>
</figure>
</main>
{'</body></html>' if FULL else ''}
'''
open(OUT, 'w').write(page)
print(f'wrote {OUT}')
