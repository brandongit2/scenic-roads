"""Where the data lives (docs/plan.md v7 §3): the NAS holds everything; the lead (the M4 today) alone merges the
journal into the build's records, and every member's jobs put their files there (workers.py); what was made by hand
goes into inputs/, translations/, descriptions/ and sources/ (the curated inputs: page.py); the app Macs run the
published app and copy what you download."""
from html import escape as E

from diag import W, Diagram, rpath, wrap

LINE = 18.5


def build(check=False):
    d = Diagram('s', check)
    tx = d.tx
    el, ar, lb = [], [], []

    def lines_of(items, w):
        """Each item wrapped to the box's width (a string is one paragraph)."""
        items = [items] if isinstance(items, str) else items
        return [ln for it in items for ln in wrap('st-l', it, w - 28)]

    def need(n):
        """A box's height for n lines under its title."""
        return 49 + LINE * (n - 1) + 14

    def box(x, y, w, h, title, lines=(), nas=False):
        el.append(f'<rect class="st-box{" nas" if nas else ""}" x="{x}" y="{y}" width="{w}" height="{h}" rx="9"/>')
        el.append(tx('st-t', x + 14, y + 25, title, w - 28))
        for i, s in enumerate(lines):
            el.append(tx('st-l', x + 14, y + 49 + LINE * i, s, w - 28))

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
    top = 16
    sx, sw = 16, 196
    nx, nw, fw = 270, 590, 172
    ax, aw = 928, 330
    bx = 1328
    src_l = lines_of(['OSM planet, twice a year', 'AWS terrain · GLO-30', 'DEMs · canopy · land cover', 'Overture · GHSL buildings',
                      'Wikidata · pageviews', 'registers (a snapshot)', 'timetables'], sw)
    rows_l = [(f, wrap('st-l', t, nw - fw - 14)) for f, t in rows]
    mac_l = lines_of(['the map is always on (a launcher)', 'copy what you download: the World, zoomed out; regions; views',
                      'read the rest from the NAS (timeouts, an offline banner)', 'Regions panel → recipes in inputs/',
                      'menu bar: the build, who leads', 'each helps build (below)'], aw)
    brw_l = lines_of(['the map: MapLibre and WebGL', 'the build page, on any device (below)'], W - 16 - bx)
    hh = max(need(len(src_l)), need(len(mac_l)), need(sum(len(t) for _, t in rows_l)), need(len(brw_l)))
    box(sx, top, sw, hh, 'Online sources', src_l)
    box(nx, top, nw, hh, 'NAS · the source of truth', nas=True)
    y = top + 49
    for f, ts in rows_l:
        el.append(tx('st-f', nx + 14, y, f, fw - 10))
        for i, t in enumerate(ts):
            el.append(tx('st-l', nx + fw, y + LINE * i, t, nw - fw - 14))
        y += LINE * len(ts)
    box(ax, top, aw, hh, 'Your Macs', mac_l)
    box(bx, top, W - 16 - bx, hh, 'Browser', brw_l)
    lw = 460
    lead_l = lines_of('two jobs at once, staged on its SSD; paused while asleep, away from the NAS or on battery below 30 %; nothing lost', lw)
    hand_l = lines_of('you, Claude agents (translators, writers, research), downloads a script can’t make', aw)
    by, bh = top + hh + 56, need(max(len(lead_l), len(hand_l)))
    box(nx, by, lw, bh, 'The lead (the M4 today) · and every member', lead_l)
    hx = ax
    box(hx, by, aw, bh, 'Made by hand (the curated inputs, below)', hand_l)

    mid = top + hh / 2
    arrow((sx + sw, top + 49), (nx, top + 49), label='the planet', at=((sx + sw + nx) / 2, top + 41), anchor='middle')
    arrow((sx + sw / 2, top + hh), (sx + sw / 2, by + bh / 2), (nx, by + bh / 2), label='downloads (kept on the NAS)', at=(sx + sw / 2 + 8, by + bh / 2 - 8))
    gx = (nx + nw + ax) / 2
    arrow((nx + nw, top + 49), (ax, top + 49), label='copies', at=(gx, top + 41), anchor='middle')
    arrow((nx + nw, top + 104), (ax, top + 104), label='on demand', at=(gx, top + 96), anchor='middle', dashed=True)
    arrow((ax, top + hh - 54), (nx + nw, top + hh - 54), label='recipes', at=(gx, top + hh - 62), anchor='middle', dashed=True)
    arrow((ax + aw, mid), (bx, mid), label='tiles · JSON', at=((ax + aw + bx) / 2, mid - 8), anchor='middle')
    arrow((nx + 200, top + hh), (nx + 200, by), label='inputs', at=(nx + 192, top + hh + 33), anchor='end')
    arrow((nx + 360, by), (nx + 360, top + hh), label='outputs', at=(nx + 368, top + hh + 33))
    xh = nx + nw - 50
    arrow((hx, by + bh / 2), (xh, by + bh / 2), (xh, top + hh), label='by hand', at=(xh + 8, top + hh + 33))

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
