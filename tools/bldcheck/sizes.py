"""Every field tile of a z6 tile encoded as it would be served: 3 x u8 (f linear, w sqrt to 16, mean
roof height sqrt to 400 m), lossless WebP (cwebp -z 6); also f alone. Totals per level.
python sizes.py <field dir>"""
import sys, os, gzip, subprocess, tempfile, numpy as np
from multiprocessing import Pool
from PIL import Image

root = sys.argv[1]

def enc(p):
    a = np.frombuffer(gzip.open(p).read(), '<u2').reshape(3, 512, 512).astype(np.float32)
    f, w, v = a[0] / 65535, a[1] / 2048, a[2] / 128
    h = np.where(f > 0, v / np.maximum(f, 1e-9), 0)
    q = lambda x: np.clip(np.round(x), 0, 255).astype(np.uint8)
    A, B, C = q(f * 255), q(np.sqrt(np.clip(w, 0, 16) / 16) * 255), q(np.sqrt(np.clip(h, 0, 400) / 400) * 255)
    out = []
    with tempfile.TemporaryDirectory() as t:
        for name, img in (('3ch', np.dstack([A, B, C])), ('f', np.dstack([A, A, A]))):
            pp, wp = os.path.join(t, 'a.png'), os.path.join(t, 'a.webp')
            Image.fromarray(img, 'RGB').save(pp)
            subprocess.run(['cwebp', '-quiet', '-lossless', '-z', '6', '-exact', pp, '-o', wp], check=True)
            out.append(os.path.getsize(wp))
    return os.path.getsize(p), out[0], out[1]

if __name__ == '__main__':
    tot = {}
    for z in sorted((d for d in os.listdir(root) if d.isdigit()), key=int):
        files = [os.path.join(dp, f) for dp, _, fs in os.walk(os.path.join(root, z)) for f in fs]
        with Pool(12) as pool:
            r = pool.map(enc, files, chunksize=8)
        s = np.array(r).sum(0)
        tot[z] = s
        print(f'z{z}: {len(files)} tiles, u16gz {s[0]/1e6:.1f} MB, webp 3ch {s[1]/1e6:.1f} MB (max {max(x[1] for x in r)/1e3:.0f} KB), f only {s[2]/1e6:.1f} MB', flush=True)
    a = np.array(list(tot.values())).sum(0)
    print(f'all: u16gz {a[0]/1e6:.1f} MB, webp 3ch {a[1]/1e6:.1f} MB, f only {a[2]/1e6:.1f} MB; without z13: webp {(a[1]-tot["13"][1])/1e6:.1f} MB')
