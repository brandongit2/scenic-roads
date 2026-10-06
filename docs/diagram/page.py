#!/usr/bin/env python3
"""pipeline.html: how the map is built, where the data lives, and the progress, on one page.

usage: page.py OUT.html [--check] [--full]
  --check  data-max on texts (for the overflow check)
  --full   a whole document (the published page is a fragment: the host adds doctype and metas)
"""
import re
import sys
from html import escape as E

import diag
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
    if kind == 'shared':
        return f'<svg width="30" height="16" class="c-osm"><text class="tool" x="15" y="12.5" text-anchor="middle" style="font-size:14px">{diag.SHARED}</text></svg>'
    t = diag.TAB[kind]
    w = 5.6 * len(t) + 14
    return (f'<svg width="{w + 2:.0f}" height="15" class="c-osm"><path class="tab {kind}" d="M1,14.5 V4 Q1,1 4,1 H{w - 2:.1f} Q{w + 1:.1f},1 {w + 1:.1f},4 V14.5 Z"/>'
            f'<text class="tab-t" x="{w / 2 + 1:.1f}" y="11" text-anchor="middle">{E(t)}</text></svg>')


classes = [('base', 'Map context'), ('place', 'Places & heritage'), ('terr', 'Elevation & terrain'), ('net', 'Road, rail & ferry network'),
           ('scen', 'Scenic metrics'), ('land', 'Land cover & trees'), ('bldg', 'Buildings'), ('osm', 'OpenStreetMap (shared)'), ('mix', 'Several classes')]
shapes = [('src', 'source'), ('kept', 'kept between runs'), ('card', 'build step, over the files it writes'),
          ('later', 'dashed: planned, or built but not in use'), ('pill', 'server route'), ('layer', 'map layer'), ('computed', 'made in the browser')]
scopes = [('area', 'built per area (a z6 tile, 300–600 km)'), ('pack', 'per z3 pack near the coverage'), ('global', 'once, for the whole world')]
runs = [('shared', 'the M1’s helper may take its jobs'), ('task', 'an area’s last steps: any worker, a device’s page too')]
formats = [('packs', 'a layer’s tiles under one root tile, with an index'), ('sectioned', 'named arrays: base packs, hidata, markdata'),
           ('PMTiles', 'the worldwide basemap, read by byte range'), ('RT v7', 'columnar road tiles, 13 scenic channels a point'),
           ('Terrarium', 'height (or slope, cover…) as RGB'), ('MVT', 'vector tiles: labels, overlays, stations'),
           ('RDMT', 'landmark points in typed columns')]

legend = ('<section class="legend" aria-label="Legend">'
          '<div><h2>Class of data</h2><ul class="cols2">' + ''.join(f'<li class="c-{c}"><span class="dot"></span>{E(n)}</li>' for c, n in classes) + '</ul></div>'
          '<div><h2>Shapes</h2><ul class="cols2">' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in shapes) + '</ul></div>'
          '<div><h2>Scope</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in scopes) + '</ul></div>'
          '<div><h2>Who runs it</h2><ul>' + ''.join(f'<li>{mini(k)}<span>{E(n)}</span></li>' for k, n in runs) + '</ul></div>'
          '<div><h2>Formats</h2><ul class="cols2">' + ''.join(f'<li><code>{E(c)}</code><span class="g">{E(n)}</span></li>' for c, n in formats) + '</ul></div>'
          '</section>')

# Implementation progress (docs/plan.md §10), as of the date shown.
PROGRESS_AT = '6 October 2026, 05:45'
PHASES = [
    ('done', 'Foundations on today’s data', 'Both Macs run the new app from the NAS. Compared with today’s map in nine places (Québec, Tokyo, London, Chamonix, Vancouver, Hong Kong, Taipei, Lisbon, Northumberland): roads, elevation profiles, every tile layer and popup details all equal. The map reads only the pages a request needs from the NAS (a first hover in Tokyo: 1.9 s, where whole files take 66 s).',
     'Left: a speed check against the old app’s measurements once this Mac’s copy is complete.'),
    ('done', 'Build agent and moving the data', 'The agent runs on the build Mac from the installed app: two jobs at once, on mains power or on battery down to 30 %, away from home too (through Tailscale; whole-planet and whole-world jobs wait for home). One pause holds the whole build, from either Mac’s menu bar, the map, the build page or scenic pause. It backs up your folders and clears replaced files. A menu bar item on both Macs shows each Mac’s job, its progress bar, the time left and a checklist of every step to the end, and clears that Mac’s build caches once the build is done. Every download is kept on the NAS once and checked whole when read (canopy, raw terrain, FABDEM, leaf type, buildings, rail timetables, pageviews); today’s build’s canopy squares, leaf-type chunks and timetables are reused, not fetched again.',
     None),
    ('mostly', 'The OSM pass and the worldwide layers', 'The 2026-09-28 planet: filtered (60.6 GB, 64 % of the planet), sets and outlines, the worldwide basemap (Planetiler in 46 minutes; 28.6 GB), the planet cut into areas (about 58 GB), road values, each area’s reach, summits, labels and hiking routes’ ends. Terrain, slope and tree cover per z3 pack, the terrain repaired from AWS’s raw tiles (kept on the NAS, packed by area), and the worldwide z8 terrain for peaks. The world’s roadside buildings come from Overture once per release (2.5 billion boxes, scanned in 18 minutes).',
     'Left: a sea mask, so that polders and depressions keep their depth.'),
    ('mostly', 'Per-area building and rankings', 'Pilot done: Northumberland and the Scottish Borders built the new way match today’s data (same roads, elevations within 1.8 m, every scenery score and flag). Areas run in map order and sample only the roads they own; what an area keeps (its elevations, its scenic results) carries over to its next run, on either Mac. Trains a day come from the rail feeds of the countries a region is in, for any region (today’s 131 feeds and 185,557 of today’s 185,604 rail ways equal). Landmarks build as their own chain beside the roads; the heritage chain, which reproduces today’s outputs byte for byte, runs on the pass, and the area overlays are drawn from it.',
     'Left: the names’ and descriptions’ to-do lists, a build-twice check per step, and validation (files decode, values in range).'),
    ('done', 'In the browser', 'Live on both Macs: landmarks, area overlays, rail stops and ferries by view (the In view numbers, lists and popups equal to today’s in 163 views). Against today’s whole files: 3–4× less browser memory (53–74 MB instead of 186–232), the map done loading sooner (London 3.3 s instead of 4.7, Alps 2.9 instead of 4.7), the same frame rates. The Regions panel adds, renames and removes regions; the map shows what each published catalog was built for, and a region not built yet as pending. A place search box; the map on an iPhone or iPad over the tailnet, installable.',
     'Left: drawing, splitting and merging regions in the panel; “Keep this view” for trips.'),
    ('mostly', 'Switching over', 'The held build matched the converted map (the same 181 areas; counts and distributions within 0.2 %), and the map has served the planet’s build since 4 October. The regions: 88, by political unit: Canada’s 13 provinces and territories; every US state, DC and Puerto Rico; England, Scotland, Wales, Northern Ireland, Ireland, the Isle of Man, Guernsey, Jersey and Gibraltar; France, Monaco, Andorra, Spain, the Canary Islands, Portugal, the Azores and Madeira; Saint-Pierre-et-Miquelon and French Guiana; Japan, Taiwan, Hong Kong and Singapore. All 88 are built (284 areas) and on the map.',
     'Left: deleting the converted data, once nothing the map reads comes from it (global/legacy/: popups’ details, roads’ English).'),
    ('planned', 'Features', '3D buildings first (docs/buildings3d.md): every building in the coverage extruded on the terrain, its height measured, else from its floors, its neighbours or GHSL; their sources are on the NAS (Overture’s 103 files and GHSL’s 91 tiles, 61.8 GB). Then PLATEAU’s and BD TOPO’s measured heights, building heights in the horizons and the viewshed tool, and sharper terrain from national DEMs.',
     None),
    ('active', 'Builds anywhere', 'The build Mac’s coordinator lends work out on leases: the M1 takes the jobs of terrain, slope, tree cover, areas, landmark candidates and peaks that fit it, and any device that opens the build page, once you accept it, takes an area’s last steps as WebAssembly (its first three results checked against the build Mac’s own run, then one in eight). The areas’ Python steps are Rust ports giving the same bytes, tree cover’s the same pixels. The build page shows the whole build to anyone on the LAN or the tailnet, over HTTPS through tailscale serve.',
     'Next: the pool, in which any Mac can lead (docs/pool.md: its core and simulator built, not yet wired in); ranged reads; a page’s files kept in the browser’s storage (OPFS); journaled group commits; the claim and hand-off files retired.'),
]
GAPS = ('The plan’s gaps (docs/plan.md §10): the pool’s core, built but not wired into the agent, has its review’s open items to fix before '
        'it’s switched on: two high (a lead re-asserting every loop when listings are slow; the app rule able to leave no working lead), '
        'eight medium and seven low.')
STATE = {'done': ('done', 'st-near'), 'mostly': ('mostly done', 'st-near'), 'active': ('in progress', 'st-on'), 'later': ('later', 'st-later'),
         'planned': ('planned', 'st-later')}


def nb(s):
    """A number and its unit kept on one line."""
    return re.sub(r'(\d) (?=(?:s|ms|m|km|GB|MB|TB|minutes)\b|%)', '\\1\u00a0', s)


def progress_html():
    rows = []
    for i, (st, name, what, left) in enumerate(PHASES, 1):
        label, cls = STATE[st]
        rows.append(f'<li><span class="pn">{i}</span><div class="pb"><div class="ph"><b>{E(name)}</b><span class="chip {cls}">{E(label)}</span></div>'
                    f'<p>{E(nb(what))}</p>' + (f'<p class="left">{E(nb(left))}</p>' if left else '') + '</div></li>')
    return (f'<section class="prog" aria-label="Progress"><h3>Progress · {E(PROGRESS_AT)}</h3><ol>' + ''.join(rows) + '</ol>'
            f'<p class="gaps">{E(nb(GAPS))}</p></section>')


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
<p class="sub">OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show. The build Mac leads the build; the M1 and any device you let help take work from it. Your translations go in a NAS folder and show up within a minute or two. The map: 88 regions, built from the planet of 28 September 2026 (progress below).</p>
</div>
</header>
{legend}
<section class="you" aria-label="Your part">
<h3>Your part</h3>
<div class="you-grid">
<div><code>localhost:8080</code><span>view the map: it’s always running on both Macs; the menu bar item shows what the build Mac is doing (as does <code>scenic status</code>), and pauses or resumes the build</span></div>
<div><code>Regions panel</code><span>add a region: find an area by name or click the map, then take it or a bigger one around it (county, province, country); it reaches the map with the round after it’s built</span></div>
<div><code>translations/</code><span>drop finished translations in this NAS folder; both Macs show them within a minute or two. <code>descriptions/</code> likewise. Lists of what’s missing will appear in their <code>todo/</code></span></div>
<div><code>Build page</code><span>the whole build at a glance, on any device (the menu bar’s Copy the Build Page’s Address); “Help with this tab” lends the device to the build once you accept it on the build Mac</span></div>
</div>
<p>Now and then: rename or remove a region in the panel (what only it covered leaves the map with the next build). Everything else (building, refreshing, copying, backups) happens on its own.</p>
</section>
{progress_html()}
<figure>
<h3>Where the data lives</h3>
<div class="scroll">{storage.build(CHECK)}</div>
<figcaption>Big files live only on the NAS. The build Mac alone writes the build’s records; the M1’s jobs put their own files there too. Your Macs copy every file the map reads, as far as their budget allows, and read the rest from the NAS.</figcaption>
</figure>
<figure>
<h3>Who builds it</h3>
<div class="scroll">{workers.build(CHECK)}</div>
<figcaption>The build Mac’s agent plans and alone writes the build’s records; its coordinator lends the work out and takes it back. The M1, when it’s open, takes the jobs of the steps marked ⇄ below that fit it, and any device you let help through the build page takes an area’s last steps (TASK below). A lease not renewed for ten minutes goes back out, so a worker that sleeps or leaves costs only its work in hand. Dashed: planned.</figcaption>
</figure>
<figure>
<h3>How each layer is built</h3>
<div class="scroll">{proposed.build(CHECK)}</div>
<figcaption>Every step runs per area (a z6 tile), per z3 pack near the coverage, or once for the whole world; regions only say which areas to build, and a round publishes them as they’re done. Values that span areas (whole roads, landmarks’ fame, ferries) come from worldwide steps, and each area reads its neighbours within 110 km, so tile edges and region borders don’t show. Translations need no rebuild. Dashed: planned.</figcaption>
</figure>
</main>
{'</body></html>' if FULL else ''}
'''
open(OUT, 'w').write(page)
print(f'wrote {OUT}')
