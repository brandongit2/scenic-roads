"""Tilted views: the prototype and today's map side by side, whole frames halved. python tiltsheet.py <dir> <out> id..."""
import sys, os
from PIL import Image, ImageDraw, ImageFont
d, out, ids = sys.argv[1], sys.argv[2], sys.argv[3:]
font = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 15)
W, H = 500, 360
sheet = Image.new('RGB', (2 * W + 6, len(ids) * (H + 22) + 22), (28, 30, 34))
dr = ImageDraw.Draw(sheet)
dr.text((4, 3), 'prototype (field)', fill=(220, 220, 220), font=font)
dr.text((W + 10, 3), 'today', fill=(220, 220, 220), font=font)
for j, vid in enumerate(ids):
    y = 22 + j * (H + 22)
    for i, m in enumerate(['app.lin', 'cur.lin']):
        im = Image.open(os.path.join(d, f'{vid}.{m}.png')).convert('RGB').resize((W, H), Image.LANCZOS)
        sheet.paste(im, (i * (W + 6), y))
    dr.text((4, y + H + 2), vid, fill=(200, 200, 200), font=font)
sheet.save(out)
