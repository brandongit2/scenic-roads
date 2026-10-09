"""Drawing helpers for the pipeline diagrams (proposed.py, storage.py, workers.py; page.py puts them together)."""
import re
from html import escape as E

from metrics import SANS, SANS_BOLD

W = 1484
COLS = {'src': (16, 180), 'd1': (224, 196), 'd2': (448, 196), 'd3': (672, 196), 'd4': (896, 196),
        'srv': (1114, 154), 'brw': (1288, 180)}
CLASSES = ['base', 'place', 'terr', 'net', 'scen', 'land', 'bldg', 'osm', 'mix']
TAB = {'area': 'PER AREA', 'pack': 'PER Z3 PACK', 'global': 'WORLDWIDE', 'task': 'TASK'}
# Before a step's language: the M1's helper may take its jobs (agent::claims::SHARED).
SHARED = '⇄'
# How reproducible a curated input is today: its method written down, partly, not at all, or its maker gone.
REPRO = {'doc': 'method written down', 'part': 'partly written down', 'none': 'undocumented', 'gone': 'maker gone, frozen'}

# Each text class's font (as CSS draws it): (face, size, letter-spacing in em); wrap() measures with them.
FONT = {'tt': ('b', 14), 'lt': ('b', 13.5), 'tool': ('b', 10.5, .04), 'sub': ('s', 12.5), 'file': ('m', 12), 'fmt': ('m', 11),
        'keep': ('m', 11), 'route': ('m', 11), 'pnote': ('s', 11.5), 'lbl': ('b', 12), 'who': ('b', 12), 'rpt': ('m', 11),
        'tab-t': ('b', 9, .08), 'lane-t': ('b', 11, .09), 'head': ('b', 11.5, .09), 'head-n': ('s', 12),
        'st-t': ('b', 14.5), 'st-l': ('s', 13), 'st-f': ('m', 12.5), 'st-lbl': ('b', 12)}
# Line spacing: a box's sub-lines and file names; formats, kept folders and routes; route notes.
LH, LH_FMT, LH_NOTE = 15.5, 13.5, 13.5


def width(cls, s):
    """A text's width in px, from the fonts' advance widths (no kerning)."""
    face, size, *ls = FONT[cls]
    if face == 'm':
        em = sum(1 if ord(c) > 0x2e80 else 0.6 for c in s)
    else:
        t = SANS_BOLD if face == 'b' else SANS
        em = sum(t.get(c, 600) for c in s) / 1000
    return size * (em + (ls[0] if ls else 0) * len(s))


def wrap(cls, s, maxw, first=None):
    """s broken into lines no wider than maxw (the first no wider than first), at spaces, and in mono after a slash.
    A list is one paragraph (its parts joined); a word wider than the room stays whole."""
    if isinstance(s, (list, tuple)):
        s = ' '.join(s)
    mono = FONT[cls][0] == 'm'
    words = re.findall(r'[^ /]*/|[^ /]+ ?|[^ ]* ?', s) if mono else re.findall(r'\S+ ?', s)
    words = [w for w in words if w]
    lines, cur = [], ''
    for w in words:
        room = (first if first is not None and not lines else maxw) * 0.985
        if cur and width(cls, (cur + w).rstrip()) > room:
            lines.append(cur.rstrip())
            cur = w
        else:
            cur += w
    if cur.strip():
        lines.append(cur.rstrip())
    return lines or ['']


def repro_mark(x, cy, level, r=4.2):
    """The reproducibility marker: a full, half or empty disc, or a crossed one (maker gone)."""
    if level == 'doc':
        return f'<circle class="rp full" cx="{x:.1f}" cy="{cy:.1f}" r="{r}"/>'
    if level == 'part':
        return (f'<circle class="rp" cx="{x:.1f}" cy="{cy:.1f}" r="{r}"/>'
                f'<path class="rp-half" d="M{x:.1f},{cy - r:.1f} A{r},{r} 0 0 0 {x:.1f},{cy + r:.1f} Z"/>')
    if level == 'gone':
        q = r * 0.62
        return (f'<circle class="rp" cx="{x:.1f}" cy="{cy:.1f}" r="{r}"/>'
                f'<path class="rp-x" d="M{x - q:.1f},{cy - q:.1f} L{x + q:.1f},{cy + q:.1f} M{x + q:.1f},{cy - q:.1f} L{x - q:.1f},{cy + q:.1f}"/>')
    return f'<circle class="rp" cx="{x:.1f}" cy="{cy:.1f}" r="{r}"/>'


class Bx:
    def __init__(s, x, y, w, h):
        s.x, s.y, s.w, s.h = x, y, w, h

    l = property(lambda s: s.x)
    r = property(lambda s: s.x + s.w)
    t = property(lambda s: s.y)
    b = property(lambda s: s.y + s.h)
    my = property(lambda s: s.y + s.h / 2)


def rpath(pts, r=7):
    """Orthogonal polyline with rounded corners (repeated points dropped)."""
    pts = [p for i, p in enumerate(pts) if i == 0 or abs(p[0] - pts[i - 1][0]) + abs(p[1] - pts[i - 1][1]) > 0.01]
    d = f'M{pts[0][0]:.1f},{pts[0][1]:.1f}'
    for i in range(1, len(pts) - 1):
        (x0, y0), (x1, y1), (x2, y2) = pts[i - 1], pts[i], pts[i + 1]
        l1, l2 = abs(x1 - x0) + abs(y1 - y0), abs(x2 - x1) + abs(y2 - y1)
        rr = min(r, l1 / 2, l2 / 2)
        ux1, uy1 = (x1 - x0) / l1, (y1 - y0) / l1
        ux2, uy2 = (x2 - x1) / l2, (y2 - y1) / l2
        d += f' L{x1 - ux1 * rr:.1f},{y1 - uy1 * rr:.1f} Q{x1:.1f},{y1:.1f} {x1 + ux2 * rr:.1f},{y1 + uy2 * rr:.1f}'
    return d + f' L{pts[-1][0]:.1f},{pts[-1][1]:.1f}'


class Diagram:
    """One SVG: lane bands, then arrows, then the connectors between rows (with halos), boxes, labels.
    mp prefixes the marker ids (several diagrams share the page)."""

    def __init__(self, mp, check=False):
        self.mp, self.check = mp, check
        self.bands, self.arrows, self.cross, self.boxes, self.labels, self.extra = [], [], [], [], [], []

    def tx(self, cls, x, y, s, maxw=None, anchor=None):
        a = f' text-anchor="{anchor}"' if anchor else ''
        m = f' data-max="{maxw:.0f}"' if self.check and maxw else ''
        return f'<text class="{cls}" x="{x:.1f}" y="{y:.1f}"{a}{m}>{E(s)}</text>'

    def card(self, k, col, y, title, tool, subs, groups, kept=None, minh=0, scope=None, later=False, shared=False, task=False):
        """A build step (tinted header: name, language, what it does) over the files it writes
        (body: names, then format and size); scope: a tab saying how it's divided; later: planned,
        not built yet (a dashed outline); shared: the M1's helper may take its jobs (⇄ before the
        language); task: a second tab, the step is part of an area's last steps, which any worker
        may run as a task (a device's page too). Texts wrap to the card's width."""
        x, w = COLS[col]
        tx = self.tx
        if shared:
            tool = f'{SHARED} {tool}'
        ty = y + 20
        tl = wrap('tt', title, w - 18, first=w - 26 - width('tool', tool))
        sl = wrap('sub', subs, w - 18)
        ty1 = ty + 17 * (len(tl) - 1)
        hh = ty1 + LH * len(sl) + 10 - y
        body, yy = [], y + hh + 3
        for files, fmt in groups:
            for f in files:
                for ln in wrap('file', f, w - 18):
                    yy += LH
                    body.append(tx('file', x + 9, yy, ln, w - 18))
            for ln in (wrap('fmt', fmt, w - 18) if fmt else []):
                yy += LH_FMT
                body.append(tx('fmt', x + 9, yy, ln, w - 18))
        yy += 9
        if kept:
            body.append(f'<rect class="kept" x="{x + 6}" y="{yy:.1f}" width="{w - 12}" height="20" rx="3"/>')
            body.append(tx('keep', x + 12, yy + 14, kept, w - 24))
            yy += 26
        h = max(yy - y, minh)
        r = 5
        el = [f'<g class="c-{k}">']
        x1 = x + w - 8
        for kind in ([scope] if scope else []) + (['task'] if task else []):
            t = TAB[kind]
            tw = width('tab-t', t) + 14
            x0, ty0 = x1 - tw, y - 15
            el.append(f'<path class="tab {kind}" d="M{x0:.1f},{y} V{ty0 + 3} Q{x0:.1f},{ty0} {x0 + 3:.1f},{ty0} H{x1 - 3:.1f} Q{x1:.1f},{ty0} {x1:.1f},{ty0 + 3} V{y} Z"/>')
            el.append(tx('tab-t', x0 + tw / 2, y - 4.3, t, anchor='middle'))
            x1 = x0 - 4
        el += [f'<rect class="card" x="{x}" y="{y}" width="{w}" height="{h:.1f}" rx="{r}"/>',
               f'<path class="card-hd" d="M{x},{y + hh:.1f} V{y + r} Q{x},{y} {x + r},{y} H{x + w - r} Q{x + w},{y} {x + w},{y + r} V{y + hh:.1f} Z"/>',
               f'<line class="card-sep" x1="{x}" y1="{y + hh:.1f}" x2="{x + w}" y2="{y + hh:.1f}"/>',
               f'<rect class="card-ol{" later" if later else ""}" x="{x}" y="{y}" width="{w}" height="{h:.1f}" rx="{r}"/>',
               tx('tool', x + w - 9, ty - 0.5, tool, anchor='end')]
        for i, ln in enumerate(tl):
            el.append(tx('tt', x + 9, ty + 17 * i, ln, w - 26 - width('tool', tool) if i == 0 else w - 18))
        for i, s in enumerate(sl):
            el.append(tx('sub', x + 9, ty1 + LH * (i + 1), s, w - 18))
        self.boxes.extend(el + body + ['</g>'])
        return Bx(x, y, w, h)

    def src(self, k, y, title, subs, kept=(), minh=0):
        """A source; kept: where its download is kept between runs."""
        x, w = COLS['src']
        tx = self.tx
        ty = y + 20
        tl = wrap('tt', title, w - 20)
        el = [tx('tt', x + 10, ty + 17 * i, s, w - 20) for i, s in enumerate(tl)]
        ty += 17 * (len(tl) - 1)
        sl = wrap('sub', subs, w - 20)
        for i, s in enumerate(sl):
            el.append(tx('sub', x + 10, ty + LH * (i + 1), s, w - 20))
        yy = ty + LH * len(sl) + 9
        if kept:
            kl = [ln for s in kept for ln in wrap('keep', s, w - 24)]
            kh = 6 + LH_FMT * len(kl)
            el.append(f'<rect class="kept" x="{x + 6}" y="{yy:.1f}" width="{w - 12}" height="{kh:.1f}" rx="3"/>')
            for i, s in enumerate(kl):
                el.append(tx('keep', x + 12, yy + LH_FMT * (i + 1) - 1, s, w - 24))
            yy += kh + 6
        h = max(yy - y, minh)
        self.boxes.append(f'<g class="c-{k}"><rect class="src" x="{x}" y="{y}" width="{w}" height="{h:.1f}" rx="11"/>' + ''.join(el) + '</g>')
        return Bx(x, y, w, h)

    def cur(self, k, y, title, subs, who, repro, col='src', minh=0, later=False, w=None):
        """A curated input: made by hand (by the owner, a Claude agent, a hand download or a hand-run step), not by a
        job. A sheet with a folded corner; who: who or what makes it (in the class's colour); repro: how reproducible
        it is today (REPRO's keys, or (key, words)), its marker and words at the foot."""
        level, words = repro if isinstance(repro, tuple) else (repro, REPRO[repro])
        x, ww = COLS[col]
        w = w or ww
        tx = self.tx
        ty = y + 19
        tl = wrap('tt', title, w - 20, first=w - 26)
        el = [tx('tt', x + 10, ty + 17 * i, s, w - 26 if i == 0 else w - 20) for i, s in enumerate(tl)]
        yy = ty + 17 * (len(tl) - 1)
        for s in wrap('sub', subs, w - 20):
            yy += LH
            el.append(tx('sub', x + 10, yy, s, w - 20))
        for s in (wrap('who', who, w - 20) if who else []):
            yy += LH
            el.append(tx('who', x + 10, yy, s, w - 20))
        for i, s in enumerate(wrap('rpt', words, w - 37)):
            yy += 17 if i == 0 else LH_FMT
            if i == 0:
                el.append(repro_mark(x + 15.5, yy - 4, level, r=4.8))
            el.append(tx('rpt', x + 27, yy, s, w - 37))
        h = max(yy + 10 - y, minh)
        f = 10
        d = (f'M{x + 3},{y} H{x + w - f} L{x + w},{y + f} V{y + h - 3:.1f} Q{x + w},{y + h:.1f} {x + w - 3},{y + h:.1f} '
             f'H{x + 3} Q{x},{y + h:.1f} {x},{y + h - 3:.1f} V{y + 3} Q{x},{y} {x + 3},{y} Z')
        fold = f'M{x + w - f},{y} V{y + f} H{x + w}'
        self.boxes.append(f'<g class="c-{k}"><path class="cur{" later" if later else ""}" d="{d}"/><path class="cur-fold" d="{fold}"/>'
                          + ''.join(el) + '</g>')
        return Bx(x, y, w, h)

    def pill(self, k, cy, lines, note=None, later=False):
        """Server routes, centred on cy; note: a line on how they're served; later: planned."""
        x, w = COLS['srv']
        rl = [ln for s in lines for ln in wrap('route', s, w - 18)]
        nl = wrap('pnote', note, w - 18) if note else []
        h = 10 + LH_FMT * len(rl) + LH_NOTE * len(nl)
        y = cy - h / 2
        el = [f'<rect class="pill{" later" if later else ""}" x="{x}" y="{y:.1f}" width="{w}" height="{h:.1f}" rx="8"/>']
        for i, s in enumerate(rl):
            el.append(self.tx('route', x + 9, y + 3.5 + LH_FMT * (i + 1), s, w - 18))
        for i, s in enumerate(nl):
            el.append(self.tx('pnote', x + 9, y + 3.5 + LH_FMT * len(rl) + LH_NOTE * (i + 1), s, w - 18))
        self.boxes.append(f'<g class="c-{k}">' + ''.join(el) + '</g>')
        return Bx(x, y, w, h)

    def layer(self, k, y, title, subs, computed=False, cy=None, later=False):
        """A map layer in the browser (dotted: computed there; dashed: planned); cy: centred there."""
        x, w = COLS['brw']
        tl = wrap('lt', title, w - 20)
        sl = wrap('sub', subs, w - 20)
        h = 19 + 16.5 * (len(tl) - 1) + LH * len(sl) + 10
        if cy is not None:
            y = cy - h / 2
        ty = y + 19
        cls = 'layer' + (' computed' if computed else '') + (' later' if later else '')
        el = [f'<rect class="{cls}" x="{x}" y="{y:.1f}" width="{w}" height="{h:.1f}" rx="6"/>']
        el += [self.tx('lt', x + 10, ty + 16.5 * i, s, w - 20) for i, s in enumerate(tl)]
        ty += 16.5 * (len(tl) - 1)
        for i, s in enumerate(sl):
            el.append(self.tx('sub', x + 10, ty + LH * (i + 1), s, w - 20))
        self.boxes.append(f'<g class="c-{k}">' + ''.join(el) + '</g>')
        return Bx(x, y, w, h)

    def arrow(self, k, *pts, label=None, at=None, anchor='start', cross=False, dashed=False):
        d = rpath(pts)
        p = f'<path class="a c-{k}{" dashed" if dashed else ""}" d="{d}" marker-end="url(#{self.mp}m-{k})"/>'
        if cross:
            self.cross.extend([f'<path class="halo" d="{d}"/>', p])
        else:
            self.arrows.append(p)
        if label:
            self.labels.append(f'<g class="c-{k}">' + self.tx('lbl', at[0], at[1], label, anchor=anchor) + '</g>')

    def to_layer(self, k, p, b, dx=12, dashed=False):
        """Pill → map layer: straight when level, else a dogleg just before the layer."""
        if abs(p.my - b.my) < 1:
            self.arrow(k, (p.r, p.my), (b.l, b.my), dashed=dashed)
        else:
            self.arrow(k, (p.r, p.my), (b.l - dx, p.my), (b.l - dx, b.my), (b.l, b.my), dashed=dashed)

    def lane(self, title, y0, y1):
        self.bands.append(f'<rect class="lane" x="6" y="{y0:.1f}" width="{W - 12}" height="{y1 - y0:.1f}" rx="10"/>')
        self.bands.append(self.tx('lane-t', 18, y0 + 16, title))

    def svg(self, h, aria, heads=()):
        """heads: (x, title, note) column headings."""
        head = ''
        for x, t, note in heads:
            head += self.tx('head', x, 30, t)
            if note:
                head += self.tx('head-n', x, 46, note)
        markers = ''.join(f'<marker id="{self.mp}m-{c}" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
                          f'<path class="mk c-{c}" d="M0,0.8 L10,5 L0,9.2 z"/></marker>' for c in CLASSES)
        return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
                f'<defs>{markers}</defs>{head}' + ''.join(self.bands + self.arrows + self.cross + self.boxes + self.labels + self.extra) + '</svg>')


CSS = '''
:root{
  --bg:#f5f5f2;--surface:#ffffff;--fg:#1b1e23;--muted:#5f6670;--faint:#8b919a;
  --lane:rgba(27,30,35,.035);--rule:rgba(27,30,35,.10);--edge:rgba(27,30,35,.32);
  --c-base:#2a78d2;--c-place:#7650d0;--c-terr:#9c7430;--c-net:#de5a1c;--c-scen:#c93a86;--c-land:#2a8f4d;--c-bldg:#0e8796;--c-osm:#69717d;--c-mix:#2c3138;
  --sans:"IBM Plex Sans Condensed",ui-sans-serif,system-ui,sans-serif;
  --mono:"IBM Plex Mono",ui-monospace,SFMono-Regular,Menlo,monospace;
  color-scheme:light;
}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){
  --bg:#0d1015;--surface:#141920;--fg:#e3e6ea;--muted:#97a0aa;--faint:#6b737d;
  --lane:rgba(255,255,255,.028);--rule:rgba(255,255,255,.09);--edge:rgba(255,255,255,.30);
  --c-base:#5ea2f4;--c-place:#a58bf7;--c-terr:#d3b170;--c-net:#f47a42;--c-scen:#ec6cae;--c-land:#56c47f;--c-bldg:#45c8d5;--c-osm:#9fa7b2;--c-mix:#d9dde3;
  color-scheme:dark;
}}
:root[data-theme="dark"]{
  --bg:#0d1015;--surface:#141920;--fg:#e3e6ea;--muted:#97a0aa;--faint:#6b737d;
  --lane:rgba(255,255,255,.028);--rule:rgba(255,255,255,.09);--edge:rgba(255,255,255,.30);
  --c-base:#5ea2f4;--c-place:#a58bf7;--c-terr:#d3b170;--c-net:#f47a42;--c-scen:#ec6cae;--c-land:#56c47f;--c-bldg:#45c8d5;--c-osm:#9fa7b2;--c-mix:#d9dde3;
  color-scheme:dark;
}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--fg)}
body{font:15.5px/1.45 var(--sans);-webkit-font-smoothing:antialiased}
main{max-width:1520px;margin:0 auto;padding:22px 16px 28px}
h1{font:600 22.5px/1.2 var(--sans);margin:0 0 4px;letter-spacing:-.005em}
.sub{margin:0;color:var(--muted);font-size:15px;max-width:1080px}
header{margin-bottom:14px;display:flex;flex-wrap:wrap;gap:10px 24px;align-items:flex-end;justify-content:space-between}
.legend{display:flex;flex-wrap:wrap;gap:8px 26px;align-items:flex-start;margin:0 0 14px;padding:10px 12px;border:1px solid var(--rule);border-radius:10px;background:var(--surface)}
.legend h2{font:600 11.5px/1 var(--sans);letter-spacing:.08em;color:var(--faint);margin:0 0 7px;text-transform:uppercase}
.legend ul{list-style:none;margin:0;padding:0;display:grid;gap:5px 14px;font-size:14px}
.legend .cols2{grid-template-columns:auto auto}
.legend li{display:flex;align-items:center;gap:7px;white-space:nowrap}
.legend svg{flex:none;display:block}
.dot{width:11px;height:11px;border-radius:3px;background:var(--k);flex:none}
.legend code{font:500 13px var(--mono);color:var(--fg)}
.legend .g{color:var(--muted)}
.legend>div{min-width:0}
figure{margin:0 0 18px}
.scroll{overflow-x:auto;-webkit-overflow-scrolling:touch;border-radius:12px}
.scroll svg{display:block;width:100%;min-width:1180px;height:auto}
figcaption{margin-top:8px;color:var(--muted);font-size:14px}
figure h3{font:600 11.5px/1 var(--sans);letter-spacing:.08em;color:var(--faint);text-transform:uppercase;margin:0 0 8px}
@media (max-width:760px){.legend .cols2{grid-template-columns:1fr}.legend li{white-space:normal;align-items:flex-start}.legend li>svg,.legend .dot{margin-top:2px}}
.c-base{--k:var(--c-base)}.c-place{--k:var(--c-place)}.c-terr{--k:var(--c-terr)}.c-net{--k:var(--c-net)}
.c-scen{--k:var(--c-scen)}.c-land{--k:var(--c-land)}.c-bldg{--k:var(--c-bldg)}.c-osm{--k:var(--c-osm)}.c-mix{--k:var(--c-mix)}
svg .lane{fill:var(--lane)}
svg .lane-t,svg .head{font:600 11px var(--sans);letter-spacing:.09em;fill:var(--faint)}
svg .head{font-size:11.5px}
svg .head-n{font:400 12px var(--sans);fill:var(--faint)}
svg .card{fill:var(--surface)}
svg .card-hd{fill:color-mix(in srgb,var(--k) 17%,var(--surface))}
svg .card-sep{stroke:color-mix(in srgb,var(--k) 45%,var(--surface));stroke-width:1}
svg .card-ol{fill:none;stroke:var(--k);stroke-width:1.2}
svg .card-ol.later{stroke-dasharray:5 3.5}
svg .tab{fill:color-mix(in srgb,var(--k) 17%,var(--surface));stroke:var(--k);stroke-width:1}
svg .tab-t{font:600 9px var(--sans);letter-spacing:.08em;fill:var(--muted)}
svg .tab.pack{fill:color-mix(in srgb,var(--k) 55%,var(--surface))}
svg .tab.pack+.tab-t{fill:var(--fg)}
svg .tab.global{fill:var(--k)}
svg .tab.global+.tab-t{fill:var(--surface)}
svg .tab.task{fill:var(--surface)}
svg .tab.task+.tab-t{fill:var(--k)}
svg .src{fill:color-mix(in srgb,var(--k) 7%,var(--surface));stroke:var(--k);stroke-width:1.2}
svg .kept{fill:none;stroke:var(--k);stroke-width:1;stroke-dasharray:3.5 2.5}
svg .pill{fill:var(--surface);stroke:var(--k);stroke-width:1}
svg .layer{fill:color-mix(in srgb,var(--k) 9%,var(--surface));stroke:var(--k);stroke-width:1.2}
svg .layer.computed{stroke-dasharray:1.2 3;stroke-linecap:round;stroke-width:1.6}
svg .pill.later,svg .layer.later{stroke-dasharray:5 3.5}
svg .tt{font:600 14px var(--sans);fill:var(--fg)}
svg .lt{font:600 13.5px var(--sans);fill:var(--fg)}
svg .tool{font:600 10.5px var(--sans);letter-spacing:.04em;fill:var(--k)}
svg .sub{font:400 12.5px var(--sans);fill:var(--muted)}
svg .file{font:500 12px var(--mono);fill:var(--fg)}
svg .fmt,svg .keep,svg .route{font:400 11px var(--mono);fill:var(--muted)}
svg .route{fill:var(--fg)}
svg .pnote{font:400 11.5px var(--sans);fill:var(--muted)}
svg .a{fill:none;stroke:var(--k);stroke-width:1.4;stroke-linejoin:round}
svg .a.dashed{stroke-dasharray:5 4}
svg .halo{fill:none;stroke:var(--bg);stroke-width:7}
svg .mk{fill:var(--k)}
svg .lbl{font:600 12px var(--sans);fill:var(--k);paint-order:stroke;stroke:var(--bg);stroke-width:4px;stroke-linejoin:round}
svg .cur{fill:color-mix(in srgb,var(--k) 4%,var(--surface));stroke:var(--k);stroke-width:1.2;stroke-linejoin:round}
svg .cur.later{stroke-dasharray:5 3.5}
svg .cur-fold{fill:color-mix(in srgb,var(--k) 30%,var(--surface));stroke:var(--k);stroke-width:1;stroke-linejoin:round}
svg .who{font:600 12px var(--sans);fill:var(--k)}
svg .rpt{font:400 11px var(--mono);fill:var(--muted)}
svg .rp{fill:var(--surface);stroke:var(--fg);stroke-width:1.1}
svg .rp.full,svg .rp-half{fill:var(--fg)}
svg .rp-x{stroke:var(--fg);stroke-width:1.1;stroke-linecap:round}
svg .st-box{fill:var(--surface);stroke:var(--edge);stroke-width:1.2}
svg .st-box.nas{stroke:var(--fg);stroke-width:1.5}
svg .st-box.later{stroke-dasharray:5 3.5}
svg .st-box.group{fill:var(--lane)}
svg .st-t{font:600 14.5px var(--sans);fill:var(--fg)}
svg .st-l{font:400 13px var(--sans);fill:var(--muted)}
svg .st-f{font:500 12.5px var(--mono);fill:var(--fg)}
svg .st-a{fill:none;stroke:var(--muted);stroke-width:1.4}
svg .st-a.dashed{stroke-dasharray:5 4}
svg .st-mk{fill:var(--muted)}
svg .st-lbl{font:600 12px var(--sans);fill:var(--muted);paint-order:stroke;stroke:var(--bg);stroke-width:4px;stroke-linejoin:round}
'''
