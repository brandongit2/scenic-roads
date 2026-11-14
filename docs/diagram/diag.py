"""Drawing helpers for the two figures (storage.py, workers.py; page.py puts them together)."""
import re
from html import escape as E

from metrics import SANS, SANS_BOLD

W = 1484
# Each text class's font (as CSS draws it): (face, size); wrap() measures with them.
FONT = {'st-t': ('b', 14.5), 'st-l': ('s', 13), 'st-f': ('m', 12.5), 'st-lbl': ('b', 12)}
# Line spacing of a box's lines; where its first line sits under its title.
LINE, FIRST = 18.5, 49


def width(cls, s):
    """A text's width in px, from the fonts' advance widths (no kerning)."""
    face, size = FONT[cls]
    if face == 'm':
        em = 0.6 * len(s)
    else:
        t = SANS_BOLD if face == 'b' else SANS
        em = sum(t.get(c, 600) for c in s) / 1000
    return size * em


def wrap(cls, s, maxw):
    """s broken into lines no wider than maxw, at spaces, and in mono after a slash; a word wider than the room
    stays whole."""
    mono = FONT[cls][0] == 'm'
    words = re.findall(r'[^ /]*/|[^ /]+ ?|[^ ]* ?', s) if mono else re.findall(r'\S+ ?', s)
    lines, cur = [], ''
    for w in (w for w in words if w):
        if cur and width(cls, (cur + w).rstrip()) > maxw * 0.985:
            lines.append(cur.rstrip())
            cur = w
        else:
            cur += w
    if cur.strip():
        lines.append(cur.rstrip())
    return lines or ['']


def rpath(pts, r=7):
    """Orthogonal polyline with rounded corners."""
    d = f'M{pts[0][0]:.1f},{pts[0][1]:.1f}'
    for i in range(1, len(pts) - 1):
        (x0, y0), (x1, y1), (x2, y2) = pts[i - 1], pts[i], pts[i + 1]
        l1, l2 = abs(x1 - x0) + abs(y1 - y0), abs(x2 - x1) + abs(y2 - y1)
        rr = min(r, l1 / 2, l2 / 2)
        ux1, uy1 = (x1 - x0) / l1, (y1 - y0) / l1
        ux2, uy2 = (x2 - x1) / l2, (y2 - y1) / l2
        d += f' L{x1 - ux1 * rr:.1f},{y1 - uy1 * rr:.1f} Q{x1:.1f},{y1:.1f} {x1 + ux2 * rr:.1f},{y1 + uy2 * rr:.1f}'
    return d + f' L{pts[-1][0]:.1f},{pts[-1][1]:.1f}'


class Bx:
    def __init__(s, x, y, w, h):
        s.x, s.y, s.w, s.h = x, y, w, h

    r = property(lambda s: s.x + s.w)
    b = property(lambda s: s.y + s.h)


class Fig:
    """One SVG of boxes and arrows, drawn in this order: frames, arrows, boxes, labels.
    mp prefixes the marker's id (the figures share the page); check: data-max on texts (page.py --check)."""

    def __init__(self, mp, check=False):
        self.mp, self.check = mp, check
        self.frames, self.arrows, self.boxes, self.labels = [], [], [], []

    def tx(self, cls, x, y, s, maxw=None, anchor=None):
        a = f' text-anchor="{anchor}"' if anchor else ''
        m = f' data-max="{maxw:.0f}"' if self.check and maxw else ''
        return f'<text class="{cls}" x="{x:.1f}" y="{y:.1f}"{a}{m}>{E(s)}</text>'

    def measure(self, w, text='', rows=()):
        """The height box() gives these contents at this width."""
        n = len(wrap('st-l', text, w - 28)) if text else 0
        for f, t in rows:
            n += len(wrap('st-f', f, w - 28)) + len(wrap('st-l', t, w - 28))
        gaps = 7 * (len(rows) - (0 if text else 1)) if rows else 0
        return FIRST + LINE * (n - 1) + gaps + 14 if n else 40

    def box(self, x, y, w, title, text='', rows=(), cls='', h=None, frame=False):
        """A titled box: a paragraph (wrapped to the box), then rows, each a file name over what it holds.
        h: at least this tall; frame: drawn under the arrows, for a box holding others."""
        hh = max(h or 0, self.measure(w, text, rows))
        el = self.frames if frame else self.boxes
        el.append(f'<rect class="st-box{" " + cls if cls else ""}" x="{x}" y="{y}" width="{w}" height="{hh:.1f}" rx="9"/>')
        el.append(self.tx('st-t', x + 14, y + 25, title, w - 28))
        yy = y + FIRST
        for s in (wrap('st-l', text, w - 28) if text else []):
            el.append(self.tx('st-l', x + 14, yy, s, w - 28))
            yy += LINE
        for i, (f, t) in enumerate(rows):
            yy += 7 if i or text else 0
            for c, s in [('st-f', s) for s in wrap('st-f', f, w - 28)] + [('st-l', s) for s in wrap('st-l', t, w - 28)]:
                el.append(self.tx(c, x + 14, yy, s, w - 28))
                yy += LINE
        return Bx(x, y, w, hh)

    def arrow(self, *pts, label=None, at=None, anchor='start'):
        self.arrows.append(f'<path class="st-a" d="{rpath(pts)}" marker-end="url(#{self.mp}-a)"/>')
        if label:
            self.labels.append(self.tx('st-lbl', at[0], at[1], label, anchor=anchor))

    def svg(self, h, aria):
        marker = (f'<marker id="{self.mp}-a" viewBox="0 0 10 10" refX="9.5" refY="5" markerWidth="6.5" markerHeight="6.5" '
                  'orient="auto-start-reverse"><path class="st-mk" d="M0,0.8 L10,5 L0,9.2 z"/></marker>')
        return (f'<svg viewBox="0 0 {W} {h:.0f}" role="img" aria-label="{E(aria)}" xmlns="http://www.w3.org/2000/svg">'
                f'<defs>{marker}</defs>' + ''.join(self.frames + self.arrows + self.boxes + self.labels) + '</svg>')


CSS = '''
:root{
  --bg:#f5f5f2;--surface:#ffffff;--fg:#1b1e23;--muted:#5f6670;--faint:#8b919a;
  --lane:rgba(27,30,35,.035);--edge:rgba(27,30,35,.32);
  --sans:"IBM Plex Sans Condensed",ui-sans-serif,system-ui,sans-serif;
  --mono:"IBM Plex Mono",ui-monospace,SFMono-Regular,Menlo,monospace;
  color-scheme:light;
}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){
  --bg:#0d1015;--surface:#141920;--fg:#e3e6ea;--muted:#97a0aa;--faint:#6b737d;
  --lane:rgba(255,255,255,.028);--edge:rgba(255,255,255,.30);
  color-scheme:dark;
}}
:root[data-theme="dark"]{
  --bg:#0d1015;--surface:#141920;--fg:#e3e6ea;--muted:#97a0aa;--faint:#6b737d;
  --lane:rgba(255,255,255,.028);--edge:rgba(255,255,255,.30);
  color-scheme:dark;
}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--fg)}
body{font:15.5px/1.45 var(--sans);-webkit-font-smoothing:antialiased}
main{max-width:1520px;margin:0 auto;padding:22px 16px 28px}
h1{font:600 22.5px/1.2 var(--sans);margin:0 0 4px;letter-spacing:-.005em}
.sub{margin:0;color:var(--muted);font-size:15px;max-width:1080px}
header{margin-bottom:18px}
figure{margin:0 0 22px}
.scroll{overflow-x:auto;-webkit-overflow-scrolling:touch;border-radius:12px}
.scroll svg{display:block;width:100%;min-width:1180px;height:auto}
figcaption{margin-top:8px;color:var(--muted);font-size:14px;max-width:1080px}
figure h2{font:600 11.5px/1 var(--sans);letter-spacing:.08em;color:var(--faint);text-transform:uppercase;margin:0 0 8px}
svg .st-box{fill:var(--surface);stroke:var(--edge);stroke-width:1.2}
svg .st-box.nas{stroke:var(--fg);stroke-width:1.5}
svg .st-box.group{fill:var(--lane)}
svg .st-t{font:600 14.5px var(--sans);fill:var(--fg)}
svg .st-l{font:400 13px var(--sans);fill:var(--muted)}
svg .st-f{font:500 12.5px var(--mono);fill:var(--fg)}
svg .st-a{fill:none;stroke:var(--muted);stroke-width:1.4}
svg .st-mk{fill:var(--muted)}
svg .st-lbl{font:600 12px var(--sans);fill:var(--muted);paint-order:stroke;stroke:var(--bg);stroke-width:4px;stroke-linejoin:round}
'''
