"""The 5-class field around a point: python probe5.py <field dir> <z> <lon> <lat> [r] -> class 4 (>=100 m) f, w, height per texel, and the area."""
import sys, gzip, math, numpy as np
d, z, lon, lat = sys.argv[1], int(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4])
r = int(sys.argv[5]) if len(sys.argv) > 5 else 3
n = 2 ** z
x = (lon + 180) / 360 * n
y = (1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n
tx, ty = int(x), int(y)
px, py = int((x - tx) * 512), int((y - ty) * 512)
a = np.frombuffer(gzip.open(f'{d}/{z}/{tx}/{ty}.bin').read(), '<u2').reshape(15, 512, 512).astype(float)
side = 40075016.686 * math.cos(math.radians(lat)) / n / 512
for c in range(5):
    f = a[c] / 65535; w = 2 ** (a[5 + c] / 65535 * 12) - 1; v = a[10 + c] / 64
    win = (slice(py - r, py + r + 1), slice(px - r, px + r + 1))
    fa = f[win].sum() * side * side
    hm = (v[win].sum() / f[win].sum()) if f[win].sum() > 0 else 0
    print(f'class {c}: roof area {fa:8.0f} m2, wall area {w[win].sum() * side * side:9.0f} m2, mean top {hm:5.0f} m (texel {side:.1f} m)')
