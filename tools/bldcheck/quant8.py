"""The 8-bit coding trial for the 5-class field: python quant8.py <field dir> <out dir> [sample]
Each tile's 15 planes to 8 bits as they'd be stored (f as sqrt(f); w as log2(1 + w) / log2(4097);
each class's mean top H = v / f linearly within its class's range), then back to the u16 planes the
prototype reads (so the app draws exactly what 8-bit storage would give). And the stored size: the
15 bytes a texel as four RGBA images (the 16th channel zero), lossless WebP (cwebp -z 6), on every
`sample`-th tile (all by default). Prints sizes per level."""
import sys, os, gzip, subprocess, tempfile, random, numpy as np
from multiprocessing import Pool
from PIL import Image

SRC, DST = sys.argv[1], sys.argv[2]
SAMPLE = int(sys.argv[3]) if len(sys.argv) > 3 else 1
LO = [0.0, 8.0, 20.0, 40.0, 100.0]
HI = [8.0, 20.0, 40.0, 100.0, 1000.0]
N = 512 * 512

def code(a):
    f = a[0:5] / 65535.0
    w = 2 ** (a[5:10] / 65535.0 * 12) - 1
    v = a[10:15] / 64.0
    qf = np.round(np.sqrt(np.clip(f, 0, 1)) * 255)
    qw = np.round(np.log2(1 + w) / np.log2(4097) * 255)
    H = np.where(f > 0, v / np.maximum(f, 1e-12), 0)
    lo = np.array(LO)[:, None, None]; hi = np.array(HI)[:, None, None]
    qh = np.round(np.clip((H - lo) / (hi - lo), 0, 1) * 255)
    return qf.astype(np.uint8), qw.astype(np.uint8), qh.astype(np.uint8)

def decode(qf, qw, qh):
    f = (qf / 255.0) ** 2
    w = 2 ** (qw / 255.0 * np.log2(4097)) - 1
    lo = np.array(LO)[:, None, None]; hi = np.array(HI)[:, None, None]
    H = lo + qh / 255.0 * (hi - lo)
    v = f * H
    return np.concatenate([np.round(f * 65535), np.round(np.log2(1 + w) / 12 * 65535), np.clip(np.round(v * 64), 0, 65535)]).astype('<u2')

def job(rel):
    a = np.frombuffer(gzip.open(os.path.join(SRC, rel)).read(), '<u2').reshape(15, 512, 512).astype(np.float64)
    qf, qw, qh = code(a)
    out = os.path.join(DST, rel)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with gzip.open(out, 'wb', compresslevel=1) as g:
        g.write(decode(qf, qw, qh).tobytes())
    size = None
    if hash(rel) % SAMPLE == 0:
        planes = np.concatenate([qf, qw, qh, np.zeros((1, 512, 512), np.uint8)])
        size = 0
        with tempfile.TemporaryDirectory() as t:
            for k in range(4):
                p, wp = os.path.join(t, f'{k}.png'), os.path.join(t, f'{k}.webp')
                Image.fromarray(np.dstack(planes[k * 4:(k + 1) * 4]), 'RGBA').save(p)
                subprocess.run(['cwebp', '-quiet', '-lossless', '-z', '6', '-exact', p, '-o', wp], check=True)
                size += os.path.getsize(wp)
    return rel, os.path.getsize(os.path.join(SRC, rel)), size

if __name__ == '__main__':
    rels = [os.path.relpath(os.path.join(dp, f), SRC) for dp, _, fs in os.walk(SRC) for f in fs if f.endswith('.bin')]
    with Pool(10) as pool:
        res = pool.map(job, rels, chunksize=16)
    by = {}
    for rel, s16, sw in res:
        z = rel.split('/')[0]
        e = by.setdefault(z, [0, 0, 0, 0])
        e[0] += 1; e[1] += s16
        if sw is not None:
            e[2] += 1; e[3] += sw
    tot = 0
    for z in sorted(by, key=int):
        n, s16, ns, sw = by[z]
        est = sw / max(ns, 1) * n
        tot += est
        print(f'level {z}: {n} tiles, u16 gz {s16 / 1e6:.1f} MB, 8-bit WebP ~{est / 1e6:.1f} MB ({ns} encoded, max {"-"})')
    print(f'all: 8-bit WebP ~{tot / 1e6:.1f} MB')
