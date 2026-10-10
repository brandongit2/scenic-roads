"""Side-by-side crops: python crop.py <dir> <id> <x> <y> <w> <h> <scale> <out> mode..."""
import sys, os
from PIL import Image, ImageDraw
d, vid, x, y, w, h, sc, out = sys.argv[1], sys.argv[2], *map(int, sys.argv[3:8]), sys.argv[8]
modes = sys.argv[9:]
tiles = []
for m in modes:
    im = Image.open(os.path.join(d, f'{vid}.{m}.png')).convert('RGB').crop((x, y, x + w, y + h)).resize((w * sc, h * sc), Image.NEAREST)
    tiles.append((m, im))
W = sum(t.width for _, t in tiles) + 6 * (len(tiles) - 1)
o = Image.new('RGB', (W, h * sc + 18), (60, 60, 60))
dr = ImageDraw.Draw(o)
cx = 0
for m, t in tiles:
    o.paste(t, (cx, 18))
    dr.text((cx + 4, 3), m, fill=(230, 230, 230))
    cx += t.width + 6
o.save(out)
