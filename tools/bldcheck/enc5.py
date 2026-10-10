"""Encodings of the 5-class field, on a sample of tiles of a level: python enc5.py <field dir> <level> [n]
 a: four RGBA images (15 planes, as quant8.py)
 b: one RGB image (f, w, H) per class present in the tile
 c: as b, the classes' f, w, H planes side by side in one wide image (fewer headers)
 d: as b without H at the finest levels (H taken as the class's mid)"""
import sys, os, gzip, random, subprocess, tempfile, numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(__file__))
from quant8 import code
root, z = sys.argv[1], sys.argv[2]
n = int(sys.argv[3]) if len(sys.argv) > 3 else 150
files = [os.path.join(dp, f) for dp, _, fs in os.walk(os.path.join(root, z)) for f in fs]
random.seed(2)
files = random.sample(files, min(n, len(files)))

def webp(arr, mode):
    with tempfile.TemporaryDirectory() as t:
        p, w = os.path.join(t, 'a.png'), os.path.join(t, 'a.webp')
        Image.fromarray(arr, mode).save(p)
        subprocess.run(['cwebp', '-quiet', '-lossless', '-z', '6', '-exact', p, '-o', w], check=True)
        return os.path.getsize(w)

tot = {'a': 0, 'b': 0, 'c': 0, 'd': 0}
present = np.zeros(5)
for f in files:
    a = np.frombuffer(gzip.open(f).read(), '<u2').reshape(15, 512, 512).astype(np.float64)
    qf, qw, qh = code(a)
    planes = np.concatenate([qf, qw, qh, np.zeros((1, 512, 512), np.uint8)])
    tot['a'] += sum(webp(np.dstack(planes[k * 4:(k + 1) * 4]), 'RGBA') for k in range(4))
    cls = [c for c in range(5) if qf[c].any() or qw[c].any()]
    for c in cls:
        present[c] += 1
    tot['b'] += sum(webp(np.dstack([qf[c], qw[c], qh[c]]), 'RGB') for c in cls) + 1
    if cls:
        tot['c'] += webp(np.hstack([np.dstack([qf[c], qw[c], qh[c]]) for c in cls]), 'RGB')
        tot['d'] += webp(np.hstack([np.dstack([qf[c], qw[c], qw[c] * 0]) for c in cls]), 'RGB')
k = len(files)
print(f'level {z}: {k} tiles; KB a tile: ' + ', '.join(f'{m} {v / k / 1024:.1f}' for m, v in tot.items()) + '; classes present in ' + ' '.join(f'{p / k * 100:.0f}%' for p in present))
