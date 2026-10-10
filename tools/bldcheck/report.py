"""The comparison's numbers and contact sheets: python report.py <shots dir> <out dir> [views.json]
Per view: the prototype (app) and today's map (cur) against full detail shrunk in linear light (ref),
or, where full detail is too many tiles, against the app as at 3 zooms closer, shrunk (chain)."""
import sys, os, json, numpy as np
from PIL import Image, ImageDraw, ImageFont
sys.path.insert(0, os.path.dirname(__file__))
from cmp import stats, lum, lstar, blur

D, OUT = sys.argv[1], sys.argv[2]
views = json.load(open(sys.argv[3])) if len(sys.argv) > 3 else json.load(open(os.path.join(os.path.dirname(D.rstrip('/')), 'views.json')))
os.makedirs(OUT, exist_ok=True)
try:
    font = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 15)
    small = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 12)
except Exception:
    font = small = ImageFont.load_default()

def load(vid, m):
    p = os.path.join(D, f'{vid}.{m}.png')
    return np.asarray(Image.open(p).convert('RGB')) if os.path.exists(p) else None

def diffimg(a, b, s=1.0, scale=8.0):
    d = lstar(blur(lum(a), s)) - lstar(blur(lum(b), s))
    k = np.clip(np.abs(d) / scale, 0, 1)
    base = np.full(d.shape, 40.0)
    r = np.where(d > 0, base + (255 - base) * k, base); g = base; bl = np.where(d < 0, base + (255 - base) * k, base)
    return np.dstack([r, g, bl]).astype(np.uint8)

rows = []
places = []
for v in views:
    p = v['id'].rsplit('-', 1)[0]
    if p not in places:
        places.append(p)
    app, cur = load(v['id'], 'app.lin'), load(v['id'], 'cur.lin')
    ref, kind = load(v['id'], 'ref.lin'), 'ref'
    if ref is None:
        ref, kind = load(v['id'], 'chain.lin'), 'chain'
    if app is None or ref is None:
        continue
    h = min(app.shape[0], ref.shape[0]); w = min(app.shape[1], ref.shape[1])
    sa = stats(app[:h, :w], ref[:h, :w], margin=16)
    sc = stats(cur[:h, :w], ref[:h, :w], margin=16)
    rows.append((v, kind, sa, sc))
    print(f"{v['id']:14s} vs {kind:5s}  app: L {sa['L_A']:.1f}/{sa['L_B']:.1f}  σ1 mad {sa['mad1']:.2f} p95 {sa['p95_1']:.1f} seen>3 {sa['seen3']*100:.0f}%  σ8 mad {sa['mad8']:.2f}   today: L {sc['L_A']:.1f}  σ8 mad {sc['mad8']:.2f}")
json.dump([{'id': v['id'], 'vs': kind, 'app': sa, 'today': sc} for v, kind, sa, sc in rows], open(os.path.join(OUT, 'metrics.json'), 'w'), indent=1, default=float)

# Contact sheets: per place, a row per zoom: the reference, the prototype, today's map, the difference.
CW, CH = 420, 300
for p in places:
    pv = [r for r in rows if r[0]['id'].rsplit('-', 1)[0] == p]
    if not pv:
        continue
    W = 90 + 4 * (CW + 6)
    H = 40 + len(pv) * (CH + 24)
    sheet = Image.new('RGB', (W, H), (28, 30, 34))
    dr = ImageDraw.Draw(sheet)
    for i, t in enumerate(['full detail, shrunk', 'prototype (field)', 'today', 'prototype − reference (L*, σ1)']):
        dr.text((90 + i * (CW + 6) + 4, 12), t, fill=(220, 220, 220), font=font)
    for j, (v, kind, sa, sc) in enumerate(pv):
        y0 = 40 + j * (CH + 24)
        imgs = [load(v['id'], f'{"ref" if kind == "ref" else "chain"}.lin'), load(v['id'], 'app.lin'), load(v['id'], 'cur.lin')]
        h = min(i.shape[0] for i in imgs); w = min(i.shape[1] for i in imgs)
        cy, cx = h // 2, w // 2
        crop = lambda a: a[cy - CH // 2:cy + CH // 2, cx - CW // 2:cx + CW // 2]
        cells = [crop(a[:h, :w]) for a in imgs] + [crop(diffimg(imgs[1][:h, :w], imgs[0][:h, :w]))]
        for i, c in enumerate(cells):
            sheet.paste(Image.fromarray(c), (90 + i * (CW + 6), y0))
        dr.text((8, y0 + 4), f"z{v['z']}", fill=(240, 240, 240), font=font)
        if v.get('pitch'):
            dr.text((8, y0 + 24), f"{v['pitch']}°", fill=(200, 200, 200), font=small)
        dr.text((8, y0 + 44), 'vs ' + ('full' if kind == 'ref' else 'z+3'), fill=(160, 160, 160), font=small)
        dr.text((90, y0 + CH + 4), f"σ1 |ΔL*| {sa['mad1']:.1f} (p95 {sa['p95_1']:.1f}), σ8 {sa['mad8']:.1f}; mean L* {sa['L_A']:.1f} vs {sa['L_B']:.1f}", fill=(170, 170, 170), font=small)
        dr.text((90 + 2 * (CW + 6), y0 + CH + 4), f"today σ8 |ΔL*| {sc['mad8']:.1f}; mean L* {sc['L_A']:.1f}", fill=(170, 170, 170), font=small)
    sheet.save(os.path.join(OUT, f'sheet-{p}.png'))
    print('sheet', p)
