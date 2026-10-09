"""Where the data lives (docs/plan.md v7 §3): the NAS holds everything; the lead (the M4 today) alone merges the
journal into the build's records, and every member's jobs put their files there (workers.py); what was made by hand
goes into inputs/, translations/, descriptions/ and sources/ (the curated inputs: page.py); the app Macs run the
published app and copy what you download."""
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

    rows = [('translations/', 'drop-ins (and descriptions/), shown within a minute or two; todo/'),
            ('inputs/', 'your region recipes (88) and outlines, ferry timetables, keys'),
            ('sources/', 'the planet (the NAS fetches it), its pieces and sets; every download, once'),
            ('base/ · hidata/', 'per area (z6 tile): its ways with elevations and scenic values; query parts'),
            ('markdata/ · ovdata/', 'landmark points per area; area and park details per z3 tile'),
            ('global/', 'worldwide results: whole roads, road → areas, trains a day, languages'),
            ('layers/', 'what the map draws, in packs (detail near the coverage); basemap, water'),
            ('work/ · cache/', 'build intermediates; what each area keeps for its next run, on either Mac'),
            ('app/ · catalog/', 'the app your Macs run; a catalog a round: every file the map reads'),
            ('state/', 'who leads (terms); the journal; the records: what each file is made from')]
    top, hh = 16, 47 + 16.5 * len(rows) + 6
    box(16, top, 212, hh, 'Online sources', ['OSM planet, twice a year', 'AWS terrain · GLO-30', 'DEMs · canopy · land cover',
                                            'Overture · GHSL buildings', 'Wikidata · pageviews', 'registers (a snapshot)', 'timetables'])
    nx, nw = 286, 560
    box(nx, top, nw, hh, 'NAS · the source of truth', nas=True)
    for i, (f, s) in enumerate(rows):
        y = top + 47 + 16.5 * i
        el.append(tx('st-f', nx + 14, y, f, 138))
        el.append(tx('st-l', nx + 160, y, s, nw - 174))
    ax, aw = 904, 360
    box(ax, top, aw, hh, 'Your Macs', ['the map is always on (a launcher)', 'copy what you download: the World,',
                                       'zoomed out; regions; views', 'read the rest from the NAS (timeouts,', 'an offline banner)',
                                       'Regions panel → recipes in inputs/', 'menu bar: the build, who leads', 'each helps build (below)'])
    bx = 1326
    box(bx, top, W - 16 - bx, hh, 'Browser', ['the map: MapLibre', 'and WebGL', 'the build page, on', 'any device (below)'])
    by, bh = top + hh + 52, 74
    lw = 440
    box(nx, by, lw, bh, 'The lead (the M4 today) · and every member', ['two jobs at once, staged on its SSD; paused while asleep,',
                                                                       'away from the NAS or on battery below 30 %; nothing lost'])
    hx = ax
    box(hx, by, aw, bh, 'Made by hand (the curated inputs, below)', ['you, Claude agents (translators, writers,',
                                                                     'research), downloads a script can’t make'])

    mid = top + hh / 2
    arrow((228, top + 46), (nx, top + 46), label='the planet', at=(257, top + 39), anchor='middle')
    arrow((122, top + hh), (122, by + bh / 2), (nx, by + bh / 2), label='downloads (kept on the NAS)', at=(130, by + bh / 2 - 7))
    arrow((nx + nw, top + 46), (ax, top + 46), label='copies', at=(875, top + 39), anchor='middle')
    arrow((nx + nw, top + 96), (ax, top + 96), label='on demand', at=(875, top + 89), anchor='middle', dashed=True)
    arrow((ax, top + hh - 50), (nx + nw, top + hh - 50), label='recipes', at=(875, top + hh - 57), anchor='middle', dashed=True)
    arrow((ax + aw, mid), (bx, mid), label='tiles · JSON', at=(1295, mid - 7), anchor='middle')
    arrow((nx + 200, top + hh), (nx + 200, by), label='inputs', at=(nx + 192, top + hh + 30), anchor='end')
    arrow((nx + 360, by), (nx + 360, top + hh), label='outputs', at=(nx + 368, top + hh + 30))
    xh = nx + nw - 50
    arrow((hx, by + bh / 2), (xh, by + bh / 2), (xh, top + hh), label='by hand', at=(xh + 8, top + hh + 30))

    h = by + bh + 14
    aria = ('Where the data lives. The NAS is the source of truth: the translation and description folders with their to-do '
            'lists, inputs with your 88 region recipes, sources (the planet the NAS fetches itself, and every download, kept '
            'once), base data and query data per area, landmark points, worldwide results, the layers with the worldwide basemap '
            'and water, build intermediates and what each area keeps for its next run, the published app, a catalog per round, '
            'and the build’s terms, journal and records. The lead, the M4 today, and every member run two jobs at once, staging '
            'on their SSDs. What was made by hand, by you, by Claude agents or by downloads a script can’t make, goes into the '
            'NAS directly. Your Macs run the map under a launcher, copy only what you download in the Regions panel, read the '
            'rest from the NAS, and write region recipes from the Regions panel.')
    marker = ('<marker id="sm-a" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
              '<path class="st-mk" d="M0,0.8 L10,5 L0,9.2 z"/></marker>')
    return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
            f'<defs>{marker}</defs>' + ''.join(ar + el + lb) + '</svg>')
