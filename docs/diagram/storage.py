"""Proposed: where the data lives (docs/plan.md v5): the NAS holds everything, only the build Mac writes it,
the app Macs run the published app and keep what they view."""
from html import escape as E

from diag import W, Diagram, rpath


def build(check=False):
    d = Diagram('s', check)
    tx = d.tx
    el, ar, lb = [], [], []

    def box(x, y, w, h, title, lines=(), nas=False):
        el.append(f'<rect class="st-box{" nas" if nas else ""}" x="{x}" y="{y}" width="{w}" height="{h}" rx="9"/>')
        el.append(tx('st-t', x + 14, y + 23, title, w - 28))
        for i, s in enumerate(lines):
            el.append(tx('st-l', x + 14, y + 45 + 16.5 * i, s, w - 28))

    def arrow(*pts, label=None, at=None, anchor='start', dashed=False):
        ar.append(f'<path class="st-a{" dashed" if dashed else ""}" d="{rpath(pts)}" marker-end="url(#sm-a)"/>')
        if label:
            lb.append(tx('st-lbl', at[0], at[1], label, anchor=anchor))

    rows = [('translations/', 'your drop-ins (and descriptions/): the servers read them directly'),
            ('inputs/', 'your region outlines, files fetched by hand, keys'),
            ('sources/', 'OSM pieces and sets (twice a year), raw AWS, canopy, registers …'),
            ('base/', 'per area (z6 tile): its ways, with elevations and scenic values'),
            ('global/', 'worldwide results (whole roads, rail, fame, names), sliced per area'),
            ('layers/', 'everything the map reads: one worldwide pyramid per kind, in packs'),
            ('app/ · catalog/', 'the app your Macs run; what is current'),
            ('state/', 'requests from any Mac; progress; backups of your folders')]
    top, hh = 16, 47 + 16.5 * len(rows) + 6
    box(16, top, 212, hh, 'Online sources', ['OSM planet, twice a year', 'Geofabrik: region outlines', 'AWS terrain tiles',
                                            'DEMs · canopy · land cover', 'Overture buildings', 'registers · Wikidata', 'timetables'])
    nx, nw = 286, 560
    box(nx, top, nw, hh, 'NAS · the source of truth', nas=True)
    for i, (f, s) in enumerate(rows):
        y = top + 47 + 16.5 * i
        el.append(tx('st-f', nx + 14, y, f, 118))
        el.append(tx('st-l', nx + 140, y, s, nw - 154))
    ax, aw = 904, 360
    box(ax, top, aw, hh, 'Your Macs', ['the map is always on: a login item', 'keep what you view, within a budget',
                                       '“Keep this view” for trips away', 'read the rest from the NAS (timeouts,', 'an offline banner)',
                                       'Regions panel → request files'])
    bx = 1326
    box(bx, top, W - 16 - bx, hh, 'Browser', ['the map: MapLibre', 'and WebGL'])
    by, bh = top + hh + 52, 74
    box(nx, by, nw, bh, 'Build Mac (M4) · the only builder', ['one area or one worldwide step at a time: staged on its SSD, pushed back,',
                                                                'cleared; low priority; the planet passes through twice a year'])

    mid = top + hh / 2
    arrow((228, mid), (nx, mid), label='downloads', at=(257, mid - 7), anchor='middle')
    arrow((nx + nw, top + 46), (ax, top + 46), label='copies', at=(875, top + 39), anchor='middle')
    arrow((nx + nw, top + 96), (ax, top + 96), label='on demand', at=(875, top + 89), anchor='middle', dashed=True)
    arrow((ax, top + hh - 34), (nx + nw, top + hh - 34), label='requests', at=(875, top + hh - 41), anchor='middle', dashed=True)
    arrow((ax + aw, mid), (bx, mid), label='tiles · JSON', at=(1295, mid - 7), anchor='middle')
    arrow((nx + 200, top + hh), (nx + 200, by), label='inputs', at=(nx + 192, top + hh + 30), anchor='end')
    arrow((nx + 360, by), (nx + 360, top + hh), label='outputs', at=(nx + 368, top + hh + 30))

    h = by + bh + 14
    aria = ('Proposed storage. Online sources are downloaded onto the NAS, the source of truth: your translation and description '
            'folders, inputs with your region outlines, sources, base data per area, worldwide results, the layers, the published app, '
            'the catalog and build state. Only the build Mac writes there, one area or one worldwide step at a time, staging on its '
            'SSD. Your Macs run the map as a login item, keep what you view within a budget, read the rest from the NAS and ask for '
            'builds with request files.')
    marker = ('<marker id="sm-a" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
              '<path class="st-mk" d="M0,0.8 L10,5 L0,9.2 z"/></marker>')
    return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
            f'<defs>{marker}</defs>' + ''.join(ar + el + lb) + '</svg>')
