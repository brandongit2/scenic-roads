#!/usr/bin/env python3
"""Sample reference colour ramps into web/src/data/ramps.json: 33 stops each, oriented and trimmed
for a dark map (sequential ramps run dark -> light with the darkest end lifted to L* >= 25).

Sources: matplotlib (inferno, cividis, cubehelix; ColorBrewer's ramps, Apache 2.0, Cynthia Brewer),
seaborn (mako, rocket), Fabio Crameri's Scientific colour maps (cmcrameri, MIT), cmocean (ice, MIT)
and colorcet (fire, CC BY 4.0, Peter Kovesi). Not project dependencies, so run it in a throwaway env:

usage: uv run --no-project --with matplotlib --with seaborn --with cmcrameri --with cmocean \
         --with colorcet python dem/ramps.py web/src/data/ramps.json
"""
import json, sys
import numpy as np
import matplotlib as mpl
import seaborn  # registers mako, rocket
import cmcrameri.cm as cmc
import cmocean
import colorcet

def lstar(rgb):
    c = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    y = 0.2126 * c[..., 0] + 0.7152 * c[..., 1] + 0.0722 * c[..., 2]
    return np.where(y > 216 / 24389, 116 * np.cbrt(y) - 16, y * 24389 / 27)

def cmap(src):
    kind, name = src.split(':')
    return {'mpl': lambda n: mpl.colormaps[n], 'cmc': lambda n: getattr(cmc, n), 'cmo': lambda n: getattr(cmocean.cm, n),
            'cc': lambda n: colorcet.cm[n]}[kind](name)

MIN_L, MAX_L = 25.0, 97.0  # dark map: lift the darkest end, keep off pure white
# key, label, group, source, mode: 'seq' (orient dark->light, trim), 'keep', 'rev' (flip, no trim),
# 'div' / 'div-rev' (diverging, ends lifted to L* >= 30)
RAMPS = [
    ('inferno', 'Inferno', 'Perceptual', 'mpl:inferno', 'seq'),
    ('cividis', 'Cividis', 'Perceptual', 'mpl:cividis', 'seq'),
    ('mako', 'Mako', 'Perceptual', 'mpl:mako', 'seq'),
    ('rocket', 'Rocket', 'Perceptual', 'mpl:rocket', 'seq'),
    ('cubehelix', 'Cubehelix', 'Perceptual', 'mpl:cubehelix', 'seq'),
    ('batlow', 'Batlow', 'Scientific', 'cmc:batlow', 'seq'),
    ('hawaii', 'Hawaii', 'Scientific', 'cmc:hawaii', 'seq'),
    ('lajolla', 'La Jolla', 'Scientific', 'cmc:lajolla', 'seq'),
    ('oslo', 'Oslo', 'Scientific', 'cmc:oslo', 'seq'),
    ('bamako', 'Bamako', 'Scientific', 'cmc:bamako', 'seq'),
    ('tokyo', 'Tokyo', 'Scientific', 'cmc:tokyo', 'seq'),
    ('spectral', 'Spectral', 'Rainbow', 'mpl:Spectral', 'rev'),
    ('rdylgn', 'Red–yellow–green', 'Rainbow', 'mpl:RdYlGn', 'keep'),
    ('ylorrd', 'Yellow–orange–red', 'Multi-hue', 'mpl:YlOrRd', 'seq'),
    ('ylgnbu', 'Yellow–green–blue', 'Multi-hue', 'mpl:YlGnBu', 'seq'),
    ('orrd', 'Orange–red', 'Multi-hue', 'mpl:OrRd', 'seq'),
    ('pubugn', 'Purple–blue–green', 'Multi-hue', 'mpl:PuBuGn', 'seq'),
    ('blues', 'Blues', 'Single hue', 'mpl:Blues', 'seq'),
    ('greens-cb', 'Greens', 'Single hue', 'mpl:Greens', 'seq'),
    ('oranges', 'Oranges', 'Single hue', 'mpl:Oranges', 'seq'),
    ('purples', 'Purples', 'Single hue', 'mpl:Purples', 'seq'),
    ('ice', 'Ice', 'Single hue', 'cmo:ice', 'seq'),
    ('fire', 'Fire', 'Single hue', 'cc:fire', 'seq'),
    ('rdbu', 'Red–blue', 'Diverging', 'mpl:RdBu', 'div-rev'),
    ('brbg', 'Brown–teal', 'Diverging', 'mpl:BrBG', 'div'),
    ('piyg', 'Pink–green', 'Diverging', 'mpl:PiYG', 'div'),
    ('coolwarm', 'Cool–warm', 'Diverging', 'mpl:coolwarm', 'keep'),
    ('vik', 'Vik', 'Diverging', 'cmc:vik', 'div'),
    ('berlin', 'Berlin (dark middle)', 'Diverging', 'cmc:berlin', 'keep'),
]
out = []
N = 33
fine = np.linspace(0, 1, 1001)
for key, label, group, src, mode in RAMPS:
    cm = cmap(src)
    rgb = cm(fine)[:, :3]
    if mode in ('rev', 'div-rev') or (mode == 'seq' and lstar(rgb[0]) > lstar(rgb[-1])):
        rgb = rgb[::-1]
    a, b = 0.0, 1.0
    if mode == 'seq':
        L = lstar(rgb)
        ok = np.nonzero(L >= MIN_L)[0]
        a = fine[ok[0]] if len(ok) else 0.0
        hi = np.nonzero(L <= MAX_L)[0]
        b = fine[hi[-1]] if len(hi) else 1.0
    if mode.startswith('div'):
        # Both ends at L* >= 30, trimmed symmetrically so the middle stays in the middle.
        L = lstar(rgb)
        s = next((fine[i] for i in range(500) if L[i] >= 30 and L[1000 - i] >= 30), 0.0)
        a, b = s, 1 - s
    ts = np.linspace(a, b, N)
    idx = np.clip(np.round(ts * 1000).astype(int), 0, 1000)
    stops = ['#%02x%02x%02x' % tuple(int(round(v * 255)) for v in rgb[i]) for i in idx]
    out.append({'key': key, 'label': label, 'group': group, 'stops': stops})
    print(f'{key:10} {src:16} trim {a:.2f}-{b:.2f}  L* {lstar(rgb[idx[0]]):.0f} -> {lstar(rgb[idx[-1]]):.0f}', file=sys.stderr)
json.dump(out, open(sys.argv[1], 'w'), separators=(',', ':'))
