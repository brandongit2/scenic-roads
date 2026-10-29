# The build page world map's country outlines: makes web/work/countries.json (task #835), from Natural
# Earth's 1:50m admin-0 countries (public domain), as the NAS keeps them in the Natural Earth vector
# package: sources/basemap/natural_earth_vector.sqlite.zip (5.1.2), table ne_50m_admin_0_countries.
# Every polygon's outer ring (holes left out), in Web Mercator 0-1000 each way (y down, as
# lakes.json), flat [x0, y0, ...], 1 decimal, in the table's order; simplified (Douglas-Peucker
# 0.2), rings left with fewer than 4 points dropped.
#
#   python3 -I tools/worldmap/countries.py [natural_earth_vector.sqlite.zip] [out.json]
#
# Defaults: the NAS's zip (read only; its 850 MB sqlite is unpacked to a temporary folder, removed
# at the end) and web/work/countries.json. The page serves the file from the pipeline binary
# (crates/pipeline/src/coord/mod.rs include_bytes), so rebuild after a change.
import struct, sys, math, json, os, sqlite3, tempfile, zipfile

NAS = '/Volumes/personal/projects/scenic-roads/sources/basemap/natural_earth_vector.sqlite.zip'
zpath = sys.argv[1] if len(sys.argv) > 1 else NAS
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'web', 'work', 'countries.json')

def outer_rings(b):
    """The first (outer) ring of each polygon of a WKB Polygon or MultiPolygon."""
    def rd(o):
        bo = '<' if b[o] == 1 else '>'
        t = struct.unpack(bo + 'I', b[o + 1:o + 5])[0] % 1000
        o += 5
        if t == 3:
            n = struct.unpack(bo + 'I', b[o:o + 4])[0]
            o += 4
            rings = []
            for _ in range(n):
                m = struct.unpack(bo + 'I', b[o:o + 4])[0]
                o += 4
                pts = struct.unpack(bo + '%dd' % (2 * m), b[o:o + 16 * m])
                o += 16 * m
                rings.append([(pts[2 * i], pts[2 * i + 1]) for i in range(m)])
            return rings[:1], o
        n = struct.unpack(bo + 'I', b[o:o + 4])[0]
        o += 4
        polys = []
        for _ in range(n):
            p, o = rd(o)
            polys += p
        return polys, o
    return rd(0)[0]

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

rings = []
with tempfile.TemporaryDirectory() as tmp:
    z = zipfile.ZipFile(zpath)  # read only: the zip is never written
    member = next(n for n in z.namelist() if n.endswith('natural_earth_vector.sqlite'))
    db = os.path.join(tmp, 'ne.sqlite')
    with z.open(member) as src, open(db, 'wb') as dst:
        while chunk := src.read(1 << 24): dst.write(chunk)
    con = sqlite3.connect('file:' + db + '?mode=ro', uri=True)
    for (g,) in con.execute('select GEOMETRY from ne_50m_admin_0_countries order by ogc_fid'):
        for r in outer_rings(g):
            m = dp([merc(*p) for p in r], 0.2)
            if len(m) < 4: continue
            rings.append([round(v, 1) for p in m for v in p])
    con.close()
with open(out, 'w') as f: json.dump(rings, f, separators=(',', ':'))
print(len(rings), 'rings', sum(len(r) for r in rings) // 2, 'points')
