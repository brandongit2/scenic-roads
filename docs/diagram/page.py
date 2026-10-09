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
        return f'<svg width="30" height="16" class="c-osm"><text class="tool" x="15" y="12.5" text-anchor="middle" style="font-size:14px">{diag.SHARED}</text></svg>'
    t = diag.TAB[kind]
    w = 5.6 * len(t) + 14
    return (f'<svg width="{w + 2:.0f}" height="15" class="c-osm"><path class="tab {kind}" d="M1,14.5 V4 Q1,1 4,1 H{w - 2:.1f} Q{w + 1:.1f},1 {w + 1:.1f},4 V14.5 Z"/>'
            f'<text class="tab-t" x="{w / 2 + 1:.1f}" y="11" text-anchor="middle">{E(t)}</text></svg>')


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

# Implementation progress (docs/plan.md §10), as of the date shown.
PROGRESS_AT = '9 October 2026'
PHASES = [
    ('done', 'Foundations on today’s data', 'Both Macs run the new app from the NAS. Compared with today’s map in nine places (Québec, Tokyo, London, Chamonix, Vancouver, Hong Kong, Taipei, Lisbon, Northumberland): roads, elevation profiles, every tile layer and popup details all equal. The map reads only the pages a request needs from the NAS (a first hover in Tokyo: 1.9 s, where whole files take 66 s).',
     'Left: a speed check against the old app’s measurements once this Mac has downloaded the places compared.'),
    ('done', 'Build agent and moving the data', 'The agent runs on both Macs from the installed app: two jobs at once, on mains power or on battery down to 30 %, away from home too (through Tailscale; whole-planet and whole-world jobs wait for home). One pause holds the whole build, from either Mac’s menu bar, the map, the build page or scenic pause. It backs up your folders and clears replaced files; room on a Mac’s disk is made toward the target you set (Disk Room), with jobs running. Every download is kept on the NAS once and checked whole when read.',
     None),
    ('mostly', 'The OSM pass and the worldwide layers', 'The 2026-09-28 planet: filtered (60.6 GB), sets and outlines, the worldwide basemap (Planetiler, 28.6 GB), the planet cut into areas (about 58 GB), road values, each area’s reach, summits, labels, hiking routes’ ends and the languages spoken where. Terrain and slope per z3 pack: AWS’s raw tiles repaired, GLO-30 north of 60° N, lakes and the sea flattened from the basemap’s water. Tree cover per z6 tile, assembled per z3 pack. The water at every zoom, each pixel’s exact share. The world’s roadside buildings from Overture once per release.',
     'Left: a sea mask, so that polders and depressions keep their depth. Under way: terrain and slope per z6 tile (#32). Planned: Canada’s missing small lakes filled from CanVec (#124).'),
    ('mostly', 'Per-area building and rankings', 'Areas run in map order and sample only the roads they own; what an area keeps (its elevations, its scenic results) carries over to its next run, on any Mac. The areas’ steps are all Rust now (elevations, land cover, area flags). Trains a day from the rail feeds of the countries the coverage is in (131 feeds). Landmarks build as their own chain beside the roads; the heritage chain, which reproduces the old outputs byte for byte, runs on the pass. The names’ and descriptions’ to-do lists are made after each catalog.',
     'Left: a build-twice check per step, and validation (files decode, values in range).'),
    ('done', 'In the browser', 'Landmarks, area overlays, rail stops and ferries by view; 3–4× less browser memory than the old whole files. The water drawn from its exact shares at every zoom, with the coastal shading (checked against full detail: 0.03 % of pixels differ). The Regions panel adds, renames and removes regions and downloads the World, zoomed out, regions or the view for trips. A place search box; the map on an iPhone or iPad over the tailnet, with two-finger gestures about the terrain.',
     'Left: drawing, splitting and merging regions in the panel.'),
    ('mostly', 'Switching over', 'The map has served the planet’s build since 4 October. The regions: 88, by political unit (Canada’s provinces and territories; every US state, DC and Puerto Rico; the British Isles; France, Spain, Portugal and their islands; Japan, Taiwan, Hong Kong and Singapore), all built and on the map (catalog 17, 8 October).',
     'Under way: removing the old pipeline (the Makefile and the converted global/legacy/ files, which nothing live reads now).'),
    ('active', 'Features', 'Built: the terrain repair (broken towers and pits taken whole, in one pass) and GLO-30 north of 60° N. 3D buildings (docs/buildings3d.md): the agent builds every tile of the coverage as its own chain (published 8 October, the tiles building); their z8 areas as tasks for pages and helpers, walls on the terrain and the map’s polish built, not yet published.',
     'Next: PLATEAU’s and BD TOPO’s measured heights, building heights in the horizons and the viewshed tool, sharper terrain from national DEMs.'),
    ('active', 'Builds anywhere', 'The pool is on (since 8 October): the M4 leads, any Mac can take the lead from the menu bar, the build page, the map or scenic lead, and every job, the lead’s too, hands its results to a journal on the NAS. The M1 takes the jobs that fit it; any device that opens the build page (no key) takes tasks as WebAssembly: an area’s last steps, tree cover’s rows of blocks, 3D buildings’ z8 areas (its first three checked, then one in eight).',
     'Next: the lead’s own jobs in their own processes, any member serving the build page (pool phases 2 and 4); detailed timing logs in every job (#130, under way); OPFS, ranged reads.'),
]
GAPS = ('The plan’s gaps (docs/plan.md §10, eleven), among them: the pool’s open items (the build page served by the lead alone, no re-keying '
        'while it’s on, the journal’s GC); the Python steps’ environment made inside the app’s folder; downloaded areas offline at '
        'their edges; stale terrain hi packs where the coverage left; two empty canopy squares on the equator; the water’s deeper '
        'zooms needing the basemap away from the NAS; the names’ short-falls (area tables still on the NAS, roads without language '
        'tags); a worker’s tail freeing only the lead’s cores.')
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
<style>{diag.CSS}{PROG_CSS}{curated.CSS}</style>
{'</head><body>' if FULL else ''}
<main>
<header>
<div>
<h1>Scenic Roads · data pipeline</h1>
<p class="sub">OpenStreetMap comes from one worldwide download, cut by area, and a region is only an outline of what to build, so region size and borders never show. One Mac leads the build (the M4 today; any Mac can take it over), and the others, and any device you let help, take work from it. What a person or an agent made by hand, the curated inputs, is marked apart, with how it could be made again (the list at the end). The map: 88 regions, built from the planet of 28 September 2026 (progress below).</p>
</div>
</header>
{legend}
<section class="you" aria-label="Your part">
<h3>Your part</h3>
<div class="you-grid">
<div><code>localhost:8080</code><span>view the map: it’s always running on both Macs; the menu bar item shows what the build Mac is doing (as does <code>scenic status</code>), and pauses or resumes the build</span></div>
<div><code>Regions panel</code><span>add a region: find an area by name or click the map, then take it or a bigger one around it (county, province, country); it reaches the map with the round after it’s built</span></div>
<div><code>translations/</code><span>drop finished translations in this NAS folder; both Macs show them within a minute or two. <code>descriptions/</code> likewise. Lists of what’s missing are in their <code>todo/</code></span></div>
<div><code>Build page</code><span>the whole build at a glance, on any device (a click on the menu bar item; its menu’s Copy the Build Page’s Address); “Help with this tab” lends the device, with no key; who leads, and handing the lead over</span></div>
</div>
<p>Now and then: rename or remove a region in the panel (what only it covered leaves the map with the next build). Everything else (building, refreshing, copying, backups) happens on its own.</p>
</section>
{progress_html()}
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
