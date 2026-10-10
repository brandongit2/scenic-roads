"""Compares shots: python cmp.py <dir> <id> modeA modeB [diff.png]. Lightness (L*) after blurring
the light (linear luminance) by a Gaussian: sigma 1 device px (what a 2x screen shows the eye) and 8
(the tone of a district)."""
import sys, os, numpy as np
from PIL import Image

def lum(rgb):
    c = rgb.astype(np.float64) / 255
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return lin @ np.array([0.2126, 0.7152, 0.0722])

def lstar(y):
    return np.where(y > 216 / 24389, 116 * np.cbrt(y) - 16, y * 24389 / 27)

def blur(a, s=1.0):
    if s <= 0:
        return a
    r = int(3 * s + 0.5)
    k = np.exp(-np.arange(-r, r + 1) ** 2 / (2 * s * s)); k /= k.sum()
    a = np.apply_along_axis(lambda v: np.convolve(np.pad(v, r, mode='edge'), k, 'valid'), 0, a)
    return np.apply_along_axis(lambda v: np.convolve(np.pad(v, r, mode='edge'), k, 'valid'), 1, a)

def stats(a, b, margin=12):
    ya, yb = lum(a), lum(b)
    out = {}
    for s in (1.0, 8.0):
        d = lstar(blur(ya, s)) - lstar(blur(yb, s))
        d = d[margin:-margin, margin:-margin]
        out[f'bias{int(s)}'] = d.mean()
        out[f'mad{int(s)}'] = np.abs(d).mean()
        out[f'p95_{int(s)}'] = np.percentile(np.abs(d), 95)
        if s == 1.0:
            out['seen3'] = (np.abs(d) > 3).mean()
    out['L_A'] = lstar(ya.mean()); out['L_B'] = lstar(yb.mean())
    return out

def diffimg(a, b, out, s=4.0, scale=6.0):
    d = lstar(blur(lum(a), s)) - lstar(blur(lum(b), s))
    k = np.clip(np.abs(d) / scale, 0, 1)
    base = np.full(d.shape, 235.0)
    r = np.where(d > 0, base, base * (1 - k)); g = base * (1 - k); bl = np.where(d < 0, base, base * (1 - k))
    Image.fromarray(np.dstack([r, g, bl]).astype(np.uint8)).save(out)

def load(d, vid, m):
    return np.asarray(Image.open(os.path.join(d, f'{vid}.{m}.png')).convert('RGB'))

if __name__ == '__main__':
    d, vid, ma, mb = sys.argv[1:5]
    ims = [load(d, vid, ma), load(d, vid, mb)]
    h = min(i.shape[0] for i in ims); w = min(i.shape[1] for i in ims)
    ims = [i[:h, :w] for i in ims]
    s = stats(ims[0], ims[1])
    print(f'{vid:16s} {ma:>10s} vs {mb:<10s}', ' '.join(f'{k} {v:.2f}' for k, v in s.items()))
    if len(sys.argv) > 5:
        diffimg(ims[0], ims[1], sys.argv[5])

def lab(rgb_lin):
    M = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    xyz = rgb_lin @ M.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)

def delta_e(a, b, s=8.0, margin=16):
    lin = lambda x: np.where(x / 255 <= 0.04045, x / 255 / 12.92, ((x / 255 + 0.055) / 1.055) ** 2.4)
    A = np.stack([blur(lin(a[..., c].astype(float)), s) for c in range(3)], -1)
    B = np.stack([blur(lin(b[..., c].astype(float)), s) for c in range(3)], -1)
    d = np.linalg.norm(lab(A) - lab(B), axis=-1)[margin:-margin, margin:-margin]
    return d.mean(), np.percentile(d, 95)
