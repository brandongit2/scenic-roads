"""python cmpall.py <dir> <views.json> [modeA modeB]: the comparison numbers of every view."""
import sys, os, json
sys.path.insert(0, os.path.dirname(__file__))
from cmp import stats, load
d, vf = sys.argv[1], sys.argv[2]
ma, mb = (sys.argv[3], sys.argv[4]) if len(sys.argv) > 4 else ('app.lin', 'ref.lin')
out = {}
for v in json.load(open(vf)):
    try:
        a, b = load(d, v['id'], ma), load(d, v['id'], mb)
    except FileNotFoundError:
        continue
    h, w = min(a.shape[0], b.shape[0]), min(a.shape[1], b.shape[1])
    s = stats(a[:h, :w], b[:h, :w], margin=16)
    out[v['id']] = s
    print(f"{v['id']:22s} L {s['L_A']:5.1f} vs {s['L_B']:5.1f}  σ1 |Δ| {s['mad1']:4.2f} p95 {s['p95_1']:5.1f}  σ8 |Δ| {s['mad8']:4.2f} bias {s['bias8']:+5.2f}")
json.dump(out, open(os.path.join(d, f'cmp-{ma}-{mb}.json'), 'w'), indent=1, default=float)
if len(sys.argv) > 5 and sys.argv[5] == 'de':
    from cmp import delta_e
    for v in json.load(open(vf)):
        try:
            a, b = load(d, v['id'], ma), load(d, v['id'], mb)
        except FileNotFoundError:
            continue
        h, w = min(a.shape[0], b.shape[0]), min(a.shape[1], b.shape[1])
        m1, p1 = delta_e(a[:h, :w], b[:h, :w], 1.0)
        m8, p8 = delta_e(a[:h, :w], b[:h, :w], 8.0)
        print(f"{v['id']:22s} ΔE σ1 {m1:5.2f} (p95 {p1:5.1f})  σ8 {m8:5.2f} (p95 {p8:5.1f})")
