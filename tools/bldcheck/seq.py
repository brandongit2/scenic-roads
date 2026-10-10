"""Flicker in a frame sequence: python seq.py <dir> <id> [n]. Per mode (app, ref): the mean |ΔL*|
between consecutive frames after a σ 1 px blur of the light (a slow pan moves the picture a fraction
of a pixel a frame: aliasing shows as frame-to-frame change beyond that), its 95th percentile, and
the view's mean lightness per frame (a zoom sweep: steps at a hand-over)."""
import sys, os, numpy as np
sys.path.insert(0, os.path.dirname(__file__))
from cmp import lum, lstar, blur, load
d, vid = sys.argv[1], sys.argv[2]
n = int(sys.argv[3]) if len(sys.argv) > 3 else 12
for mode in ('app', 'ref'):
    frames = []
    for i in range(n):
        try:
            frames.append(load(d, vid, f'seq{i:02d}.{mode}.lin'))
        except FileNotFoundError:
            break
    if len(frames) < 2:
        continue
    h = min(f.shape[0] for f in frames); w = min(f.shape[1] for f in frames)
    L = [lstar(blur(lum(f[:h, :w]), 1.0))[16:-16, 16:-16] for f in frames]
    dif = [np.abs(L[i + 1] - L[i]) for i in range(len(L) - 1)]
    mean = [lstar(lum(f[16:h - 16, 16:w - 16]).mean()) for f in frames]
    print(f'{vid:24s} {mode}: frame-to-frame |ΔL*| {np.mean([x.mean() for x in dif]):.2f} (p95 {np.mean([np.percentile(x, 95) for x in dif]):.1f}, p99.9 {np.mean([np.percentile(x, 99.9) for x in dif]):.1f}); mean L* by frame {" ".join(f"{m:.1f}" for m in mean)}')
