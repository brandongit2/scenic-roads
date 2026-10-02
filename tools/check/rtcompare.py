"""Compare road tiles of a legacy archive (RDTILES1) with the new packs (RDPACK01) under one tile.

    python3 tools/check/rtcompare.py <legacy roads.tiles> <pack> [<pack> …]

For each tile in the packs: whether the legacy archive has it, and the line / vertex counts of
both (decoded from RT v6 / v7), so differences in geometry show apart from ids and road lengths.
"""
import gzip, struct, sys
from collections import Counter


def index_rdtiles(path):
    f = open(path, 'rb')
    head = f.read(28)
    assert head[:8] == b'RDTILES1', head[:8]
    off, n = struct.unpack('<QQ', head[8:24])
    f.seek(off)
    out = {}
    for i in range(n):
        key, o, l, raw = struct.unpack('<QQII', f.read(24))
        out[key] = (o, l)
    return f, out


def index_rdpack(path):
    f = open(path, 'rb')
    head = f.read(36)
    assert head[:8] == b'RDPACK01', head[:8]
    off, n = struct.unpack('<QQ', head[16:32])
    f.seek(off)
    out = {}
    for i in range(n):
        key, o, l, raw, h = struct.unpack('<QQIIQ', f.read(32))
        out[key] = (o, l)
    return f, out


def varint(b, i):
    v = s = 0
    while True:
        c = b[i]; i += 1
        v |= (c & 0x7f) << s; s += 7
        if c < 0x80:
            return v, i


def rt_counts(blob):
    b = gzip.decompress(blob)
    assert b[:2] == b'RT', b[:4]
    i = 4
    nl, i = varint(b, i)
    nv, i = varint(b, i)
    styles = b[i:i + nl]
    return nl, nv, Counter(s & 0x0f for s in styles)


def zxy(key):
    return key >> 58, (key >> 29) & ((1 << 29) - 1), key & ((1 << 29) - 1)


legacy_f, legacy = index_rdtiles(sys.argv[1])
diff = same = missing = extra = 0
lines_old = lines_new = verts_old = verts_new = 0
worst = []
for p in sys.argv[2:]:
    f, idx = index_rdpack(p)
    for key, (o, l) in sorted(idx.items()):
        f.seek(o); nb = f.read(l)
        n_l, n_v, n_c = rt_counts(nb)
        lines_new += n_l; verts_new += n_v
        if key not in legacy:
            extra += 1
            continue
        lo, ll = legacy[key]
        legacy_f.seek(lo); ob = legacy_f.read(ll)
        o_l, o_v, o_c = rt_counts(ob)
        lines_old += o_l; verts_old += o_v
        if (o_l, o_v) == (n_l, n_v):
            same += 1
        else:
            diff += 1
            worst.append((abs(o_v - n_v), zxy(key), (o_l, o_v), (n_l, n_v)))
print(f"tiles: {same} same counts, {diff} different, {extra} new only")
print(f"lines {lines_old} → {lines_new}, vertices {verts_old} → {verts_new}")
for w in sorted(worst, reverse=True)[:12]:
    print('  ', w)
