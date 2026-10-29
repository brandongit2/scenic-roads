# The build page world map's large lakes (web/work/lakes.json, task #831), from Natural Earth's
# 1:10m lakes (public domain), as the NAS keeps them: sources/fetched/natural-earth-lakes-10m.
# Rings in Web Mercator 0-1000 each way (y down, as countries.json), flat [x0, y0, ...], 1
# decimal; only lakes of scalerank <= 3, simplified (Douglas-Peucker 0.15), specks dropped.
#
#   python3 -I tools/worldmap/lakes.py [ne_10m_lakes.zip] [out.json]
#
# Defaults: the NAS's zip (read only) and web/work/lakes.json. The page serves the file from the
# pipeline binary (crates/pipeline/src/coord/mod.rs include_bytes), so rebuild after a change.
import struct, sys, math, json, os, zipfile

NAS = '/Volumes/personal/projects/scenic-roads/sources/fetched/natural-earth-lakes-10m/ne_10m_lakes.zip'
zpath = sys.argv[1] if len(sys.argv) > 1 else NAS
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'web', 'work', 'lakes.json')
z = zipfile.ZipFile(zpath)  # read only: the zip is never written or extracted to disk
member = lambda ext: next(n for n in z.namelist() if n.endswith('ne_10m_lakes' + ext))

def dbf(path):
    b = z.read(member(path))
    n, hl, rl = struct.unpack('<IHH', b[4:12])
    fields, o = [], 32
    while b[o] != 0x0D:
        name = b[o:o + 11].split(b'\0')[0].decode()
        fields.append((name, b[o + 16]))
        o += 32
    rows = []
    for i in range(n):
        r, p, row = b[hl + i * rl:hl + (i + 1) * rl], 1, {}
        for name, ln in fields:
            row[name] = r[p:p + ln].decode('utf-8', 'replace').strip()
            p += ln
        rows.append(row)
    return rows

def shp(path):
    b = z.read(member(path))
    o, out = 100, []
    while o < len(b):
        _, ln = struct.unpack('>II', b[o:o + 8])
        rec = b[o + 8:o + 8 + ln * 2]
        o += 8 + ln * 2
        if struct.unpack('<i', rec[:4])[0] != 5:
            out.append([]); continue
        np_, npt = struct.unpack('<ii', rec[36:44])
        parts = list(struct.unpack(f'<{np_}i', rec[44:44 + 4 * np_]))
        pts = struct.unpack(f'<{2 * npt}d', rec[44 + 4 * np_:44 + 4 * np_ + 16 * npt])
        rings = []
        for k, s in enumerate(parts):
            e = parts[k + 1] if k + 1 < np_ else npt
            rings.append([(pts[2 * i], pts[2 * i + 1]) for i in range(s, e)])
        out.append(rings)
    return out

def merc(lon, lat):
    lat = max(-85.05, min(85.05, lat))
    return ((lon + 180) / 360 * 1000, (1 - math.log(math.tan(math.pi / 4 + math.radians(lat) / 2)) / math.pi) / 2 * 1000)

def dp(pts, eps):
    if len(pts) < 3: return pts
    keep, stack = [0, len(pts) - 1], [(0, len(pts) - 1)]
    while stack:
        a, b = stack.pop()
        (ax, ay), (bx, by) = pts[a], pts[b]
        dx, dy, best, bi = bx - ax, by - ay, -1, -1
        d2 = dx * dx + dy * dy
        for i in range(a + 1, b):
            px, py = pts[i]
            d = abs(dy * (px - ax) - dx * (py - ay)) / math.sqrt(d2) if d2 else math.hypot(px - ax, py - ay)
            if d > best: best, bi = d, i
        if best > eps:
            keep.append(bi); stack += [(a, bi), (bi, b)]
    return [pts[i] for i in sorted(keep)]

rows, geoms = dbf('.dbf'), shp('.shp')
rings = []
for row, g in zip(rows, geoms):
    try: rank = int(float(row.get('scalerank') or 99))
    except ValueError: rank = 99
    if rank > 3: continue
    for r in g:
        m = dp([merc(*p) for p in r], 0.15)
        if len(m) < 4: continue
        xs, ys = [p[0] for p in m], [p[1] for p in m]
        if (max(xs) - min(xs)) * (max(ys) - min(ys)) < 0.3: continue
        rings.append([round(v, 1) for p in m for v in p])
json.dump(rings, open(out, 'w'), separators=(',', ':'))
print(len(rings), 'rings', sum(len(r) for r in rings) // 2, 'points')
