"""Rows of views: a crop around a screen point, scaled up, in several modes, labelled.
python cnsheet.py <dir> <out> <cx> <cy> <w> <h> <scale> <modes,comma> id..."""
import sys, os
from PIL import Image, ImageDraw, ImageFont
d, out = sys.argv[1], sys.argv[2]
cx, cy, w, h, sc = map(int, sys.argv[3:8])
modes = sys.argv[8].split(',')
ids = sys.argv[9:]
font = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 14)
W, H = w * sc, h * sc
sheet = Image.new('RGB', (120 + len(modes) * (W + 6), 24 + len(ids) * (H + 6)), (28, 30, 34))
dr = ImageDraw.Draw(sheet)
names = {'ref.lin': 'full detail, shrunk', 'app.lin': 'prototype', 'cur.lin': 'today', 'chain.lin': 'prototype at z+3, shrunk'}
for i, m in enumerate(modes):
    dr.text((120 + i * (W + 6) + 4, 4), names.get(m, m), fill=(220, 220, 220), font=font)
for j, vid in enumerate(ids):
    y = 24 + j * (H + 6)
    dr.text((6, y + 4), vid, fill=(220, 220, 220), font=font)
    for i, m in enumerate(modes):
        p = os.path.join(d, f'{vid}.{m}.png')
        if not os.path.exists(p):
            continue
        im = Image.open(p).convert('RGB').crop((cx - w // 2, cy - h // 2, cx + w // 2, cy + h // 2)).resize((W, H), Image.NEAREST)
        sheet.paste(im, (120 + i * (W + 6), y))
sheet.save(out)
